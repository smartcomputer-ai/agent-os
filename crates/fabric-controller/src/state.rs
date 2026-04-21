use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use fabric_protocol::{
    ControllerExecRequest, ControllerSessionStatus, ControllerSessionSummary, ExecEvent,
    ExecEventKind, ExecId, FabricHostProvider, FabricSessionSignalKind, FabricSessionTarget,
    FabricSessionTargetKind, HostHeartbeatRequest, HostId, HostInventoryResponse,
    HostRegisterRequest, HostStatus, HostSummary, RequestId, SessionId, SessionStatus,
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FabricControllerError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("{code}: {message}")]
    Conflict { code: String, message: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsupported target: {0}")]
    UnsupportedTarget(String),
    #[error("unsupported lifecycle: {0}")]
    UnsupportedLifecycle(String),
    #[error("no healthy host: {0}")]
    NoHealthyHost(String),
    #[error("host error: {0}")]
    HostError(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("time error: {0}")]
    Time(String),
}

#[derive(Debug, Clone)]
pub struct NewControllerSession {
    pub session_id: SessionId,
    pub target: FabricSessionTarget,
    pub host_id: HostId,
    pub host_session_id: SessionId,
    pub status: ControllerSessionStatus,
    pub workdir: Option<String>,
    pub supported_signals: Vec<FabricSessionSignalKind>,
    pub labels: BTreeMap<String, String>,
    pub expires_at_ns: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct NewControllerExec {
    pub exec_id: ExecId,
    pub scope: String,
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub host_id: HostId,
    pub request: ControllerExecRequest,
}

#[derive(Debug, Clone)]
pub enum IdempotencyStart {
    Acquired,
    Replay(String),
}

#[derive(Clone)]
pub struct FabricControllerState {
    db_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl FabricControllerState {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, FabricControllerError> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                FabricControllerError::Time(format!(
                    "create controller database directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }

        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&conn)?;

        Ok(Self {
            db_path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn upsert_registered_host(
        &self,
        request: &HostRegisterRequest,
    ) -> Result<HostSummary, FabricControllerError> {
        if request.host_id.0.trim().is_empty() {
            return Err(FabricControllerError::BadRequest(
                "host_id must not be empty".to_owned(),
            ));
        }
        if request.endpoint.trim().is_empty() {
            return Err(FabricControllerError::BadRequest(
                "endpoint must not be empty".to_owned(),
            ));
        }

        let now = now_ns_i64()?;
        let providers_json = serde_json::to_string(&request.providers)?;
        let labels_json = serde_json::to_string(&request.labels)?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            INSERT INTO hosts (
              host_id, endpoint, status, providers_json, labels_json,
              last_heartbeat_ns, created_at_ns, updated_at_ns
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
            ON CONFLICT(host_id) DO UPDATE SET
              endpoint = excluded.endpoint,
              status = excluded.status,
              providers_json = excluded.providers_json,
              labels_json = excluded.labels_json,
              last_heartbeat_ns = excluded.last_heartbeat_ns,
              updated_at_ns = excluded.updated_at_ns
            "#,
            params![
                request.host_id.0,
                request.endpoint,
                host_status_name(HostStatus::Healthy),
                providers_json,
                labels_json,
                now,
            ],
        )?;
        drop(conn);

        self.host(&request.host_id)?
            .ok_or_else(|| FabricControllerError::NotFound(request.host_id.0.clone()))
    }

    pub fn record_heartbeat(
        &self,
        request: &HostHeartbeatRequest,
    ) -> Result<HostSummary, FabricControllerError> {
        if request.host_id.0.trim().is_empty() {
            return Err(FabricControllerError::BadRequest(
                "host_id must not be empty".to_owned(),
            ));
        }

        let now = now_ns_i64()?;
        let providers_json = serde_json::to_string(&request.providers)?;
        let labels_json = serde_json::to_string(&request.labels)?;
        let inventory_json = match &request.inventory {
            Some(inventory) => Some(serde_json::to_string(inventory)?),
            None => None,
        };

        let conn = self.lock_conn()?;
        let existing_endpoint: Option<String> = conn
            .query_row(
                "SELECT endpoint FROM hosts WHERE host_id = ?1",
                params![request.host_id.0],
                |row| row.get(0),
            )
            .optional()?;
        let endpoint = request
            .endpoint
            .as_deref()
            .or(existing_endpoint.as_deref())
            .ok_or_else(|| {
                FabricControllerError::BadRequest(
                    "heartbeat for unknown host must include endpoint".to_owned(),
                )
            })?;

        conn.execute(
            r#"
            INSERT INTO hosts (
              host_id, endpoint, status, providers_json, labels_json,
              last_heartbeat_ns, created_at_ns, updated_at_ns
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
            ON CONFLICT(host_id) DO UPDATE SET
              endpoint = excluded.endpoint,
              status = excluded.status,
              providers_json = excluded.providers_json,
              labels_json = excluded.labels_json,
              last_heartbeat_ns = excluded.last_heartbeat_ns,
              updated_at_ns = excluded.updated_at_ns
            "#,
            params![
                request.host_id.0,
                endpoint,
                host_status_name(HostStatus::Healthy),
                providers_json,
                labels_json,
                now,
            ],
        )?;

        if let Some(inventory) = &request.inventory {
            replace_inventory(
                &conn,
                inventory,
                inventory_json.as_deref().unwrap_or(""),
                now,
            )?;
        }
        drop(conn);

        if let Some(inventory) = &request.inventory {
            self.reconcile_inventory(inventory)?;
        }

        self.host(&request.host_id)?
            .ok_or_else(|| FabricControllerError::NotFound(request.host_id.0.clone()))
    }

    pub fn list_hosts(&self) -> Result<Vec<HostSummary>, FabricControllerError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT host_id, endpoint, status, providers_json, labels_json,
                   last_heartbeat_ns, created_at_ns, updated_at_ns
            FROM hosts
            ORDER BY host_id
            "#,
        )?;
        let hosts = stmt
            .query_map([], host_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(hosts)
    }

    pub fn host(&self, host_id: &HostId) -> Result<Option<HostSummary>, FabricControllerError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            r#"
            SELECT host_id, endpoint, status, providers_json, labels_json,
                   last_heartbeat_ns, created_at_ns, updated_at_ns
            FROM hosts
            WHERE host_id = ?1
            "#,
            params![host_id.0],
            host_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn inventory(
        &self,
        host_id: &HostId,
    ) -> Result<Option<HostInventoryResponse>, FabricControllerError> {
        let conn = self.lock_conn()?;
        let inventory_json: Option<String> = conn
            .query_row(
                r#"
                SELECT inventory_json
                FROM host_inventory
                WHERE host_id = ?1
                ORDER BY observed_at_ns DESC
                LIMIT 1
                "#,
                params![host_id.0],
                |row| row.get(0),
            )
            .optional()?;

        inventory_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }

    pub fn begin_idempotency(
        &self,
        scope: &str,
        request_id: &RequestId,
        operation: &str,
        request_hash: &str,
        resource_kind: Option<&str>,
        resource_id: Option<&str>,
        expires_at_ns: Option<u128>,
    ) -> Result<IdempotencyStart, FabricControllerError> {
        let now = now_ns_i64()?;
        let expires_at_ns = expires_at_ns.map(u128_to_i64).transpose()?;
        let conn = self.lock_conn()?;
        let existing: Option<(String, String, Option<String>, Option<String>)> = conn
            .query_row(
                r#"
                SELECT request_hash, status, response_json, error_json
                FROM idempotency
                WHERE scope = ?1 AND request_id = ?2
                "#,
                params![scope, request_id.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        if let Some((existing_hash, status, response_json, error_json)) = existing {
            if existing_hash != request_hash {
                return Err(FabricControllerError::Conflict {
                    code: "idempotency_key_conflict".to_owned(),
                    message: "request_id was already used with a different request body".to_owned(),
                });
            }

            return match (status.as_str(), response_json, error_json) {
                ("completed", Some(response_json), _) => {
                    Ok(IdempotencyStart::Replay(response_json))
                }
                ("failed", _, Some(error_json)) => Err(FabricControllerError::Conflict {
                    code: "idempotent_request_failed".to_owned(),
                    message: error_json,
                }),
                _ => Err(FabricControllerError::Conflict {
                    code: "request_in_flight".to_owned(),
                    message: "request_id is already in flight".to_owned(),
                }),
            };
        }

        conn.execute(
            r#"
            INSERT INTO idempotency (
              scope, request_id, operation, request_hash, status,
              resource_kind, resource_id, created_at_ns, updated_at_ns, expires_at_ns
            )
            VALUES (?1, ?2, ?3, ?4, 'in_flight', ?5, ?6, ?7, ?7, ?8)
            "#,
            params![
                scope,
                request_id.0,
                operation,
                request_hash,
                resource_kind,
                resource_id,
                now,
                expires_at_ns,
            ],
        )?;

        Ok(IdempotencyStart::Acquired)
    }

    pub fn complete_idempotency(
        &self,
        scope: &str,
        request_id: &RequestId,
        response_json: &str,
    ) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            UPDATE idempotency
            SET status = 'completed',
                response_json = ?3,
                error_json = NULL,
                updated_at_ns = ?4
            WHERE scope = ?1 AND request_id = ?2
            "#,
            params![scope, request_id.0, response_json, now],
        )?;
        Ok(())
    }

    pub fn fail_idempotency(
        &self,
        scope: &str,
        request_id: &RequestId,
        error_json: &str,
    ) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            UPDATE idempotency
            SET status = 'failed',
                response_json = NULL,
                error_json = ?3,
                updated_at_ns = ?4
            WHERE scope = ?1 AND request_id = ?2
            "#,
            params![scope, request_id.0, error_json, now],
        )?;
        Ok(())
    }

    pub fn reconcile_inventory(
        &self,
        inventory: &HostInventoryResponse,
    ) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let conn = self.lock_conn()?;
        for session in &inventory.sessions {
            conn.execute(
                r#"
                UPDATE sessions
                SET status = ?3,
                    workdir = COALESCE(?4, workdir),
                    updated_at_ns = ?5
                WHERE host_id = ?1
                  AND host_session_id = ?2
                  AND status NOT IN ('closed', 'error', 'lost')
                "#,
                params![
                    inventory.host_id.0,
                    session.session_id.0,
                    controller_status_name(controller_status_from_host_status(session.status)),
                    session.workdir,
                    now,
                ],
            )?;
        }
        Ok(())
    }

    pub fn insert_session(
        &self,
        session: &NewControllerSession,
    ) -> Result<ControllerSessionSummary, FabricControllerError> {
        let now = now_ns_i64()?;
        let expires_at_ns = session.expires_at_ns.map(u128_to_i64).transpose()?;
        let target_json = serde_json::to_string(&session.target)?;
        let supported_signals_json = serde_json::to_string(&session.supported_signals)?;
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO sessions (
              session_id, target_kind, target_json, host_id, host_session_id,
              status, workdir, supported_signals_json, created_at_ns, updated_at_ns,
              expires_at_ns
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)
            "#,
            params![
                session.session_id.0,
                target_kind_name(session.target.kind()),
                target_json,
                session.host_id.0,
                session.host_session_id.0,
                controller_status_name(session.status),
                session.workdir,
                supported_signals_json,
                now,
                expires_at_ns,
            ],
        )?;
        replace_session_labels_tx(&tx, &session.session_id, &session.labels)?;
        tx.commit()?;
        drop(conn);

        self.session(&session.session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(session.session_id.0.clone()))
    }

    pub fn update_session_opened(
        &self,
        session_id: &SessionId,
        status: ControllerSessionStatus,
        workdir: &str,
    ) -> Result<ControllerSessionSummary, FabricControllerError> {
        let now = now_ns_i64()?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            UPDATE sessions
            SET status = ?2, workdir = ?3, updated_at_ns = ?4
            WHERE session_id = ?1
            "#,
            params![session_id.0, controller_status_name(status), workdir, now],
        )?;
        drop(conn);

        self.session(session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(session_id.0.clone()))
    }

    pub fn update_session_status(
        &self,
        session_id: &SessionId,
        status: ControllerSessionStatus,
    ) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE sessions SET status = ?2, updated_at_ns = ?3 WHERE session_id = ?1",
            params![session_id.0, controller_status_name(status), now],
        )?;
        Ok(())
    }

    pub fn update_session_signaled(
        &self,
        session_id: &SessionId,
        status: ControllerSessionStatus,
    ) -> Result<ControllerSessionSummary, FabricControllerError> {
        let now = now_ns_i64()?;
        let closed_at_ns = matches!(status, ControllerSessionStatus::Closed).then_some(now);
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            UPDATE sessions
            SET status = ?2,
                updated_at_ns = ?3,
                closed_at_ns = COALESCE(?4, closed_at_ns)
            WHERE session_id = ?1
            "#,
            params![
                session_id.0,
                controller_status_name(status),
                now,
                closed_at_ns,
            ],
        )?;
        drop(conn);

        self.session(session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(session_id.0.clone()))
    }

    pub fn session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ControllerSessionSummary>, FabricControllerError> {
        let conn = self.lock_conn()?;
        let session = conn
            .query_row(
                session_select_sql("AND s.session_id = ?1").as_str(),
                params![session_id.0],
                session_from_row,
            )
            .optional()?;
        drop(conn);

        session
            .map(|mut session| {
                session.labels = self.session_labels(&session.session_id)?;
                Ok(session)
            })
            .transpose()
    }

    pub fn list_sessions(
        &self,
        label_filters: &[(String, String)],
    ) -> Result<Vec<ControllerSessionSummary>, FabricControllerError> {
        let mut where_clause = String::new();
        let mut values = Vec::new();
        for (index, (key, value)) in label_filters.iter().enumerate() {
            where_clause.push_str(&format!(
                " AND EXISTS (
                    SELECT 1 FROM session_labels sl{index}
                    WHERE sl{index}.session_id = s.session_id
                      AND sl{index}.key = ?
                      AND sl{index}.value = ?
                )"
            ));
            values.push(Value::Text(key.clone()));
            values.push(Value::Text(value.clone()));
        }

        let sql = session_select_sql(&where_clause);
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let sessions = stmt
            .query_map(params_from_iter(values.iter()), session_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        drop(conn);

        sessions
            .into_iter()
            .map(|mut session| {
                session.labels = self.session_labels(&session.session_id)?;
                Ok(session)
            })
            .collect()
    }

    pub fn patch_session_labels(
        &self,
        session_id: &SessionId,
        set: &BTreeMap<String, String>,
        remove: &[String],
    ) -> Result<BTreeMap<String, String>, FabricControllerError> {
        let mut conn = self.lock_conn()?;
        let exists = conn
            .query_row(
                "SELECT 1 FROM sessions WHERE session_id = ?1",
                params![session_id.0],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(FabricControllerError::NotFound(format!(
                "session {}",
                session_id.0
            )));
        }

        let tx = conn.transaction()?;
        for key in remove {
            tx.execute(
                "DELETE FROM session_labels WHERE session_id = ?1 AND key = ?2",
                params![session_id.0, key],
            )?;
        }
        for (key, value) in set {
            validate_label(key, value)?;
            tx.execute(
                r#"
                INSERT INTO session_labels (session_id, key, value)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(session_id, key) DO UPDATE SET value = excluded.value
                "#,
                params![session_id.0, key, value],
            )?;
        }
        tx.commit()?;
        drop(conn);

        self.session_labels(session_id)
    }

    pub fn insert_exec(&self, exec: &NewControllerExec) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let request_json = serde_json::to_string(&exec.request)?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            INSERT INTO execs (
              exec_id, scope, request_id, session_id, host_id, status,
              request_json, created_at_ns, updated_at_ns
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?7)
            "#,
            params![
                exec.exec_id.0,
                exec.scope,
                exec.request_id.0,
                exec.session_id.0,
                exec.host_id.0,
                request_json,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn append_exec_event(
        &self,
        exec_id: &ExecId,
        event: &ExecEvent,
    ) -> Result<(), FabricControllerError> {
        let now = now_ns_i64()?;
        let event_json = serde_json::to_string(event)?;
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
            INSERT OR REPLACE INTO exec_events (
              exec_id, seq, event_json, created_at_ns
            )
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![exec_id.0, event.seq, event_json, now],
        )?;

        if matches!(event.kind, ExecEventKind::Exit | ExecEventKind::Error) {
            conn.execute(
                r#"
                UPDATE execs
                SET status = ?2,
                    host_exec_id = COALESCE(host_exec_id, ?3),
                    exit_code = ?4,
                    error_message = ?5,
                    completed_at_ns = ?6,
                    updated_at_ns = ?6
                WHERE exec_id = ?1
                "#,
                params![
                    exec_id.0,
                    exec_status_name(event.kind),
                    event.exec_id.0,
                    event.exit_code,
                    event.message.clone().or_else(|| event
                        .data
                        .as_ref()
                        .and_then(|data| data.as_text())
                        .map(str::to_owned)),
                    now,
                ],
            )?;
        } else {
            conn.execute(
                r#"
                UPDATE execs
                SET host_exec_id = COALESCE(host_exec_id, ?2),
                    started_at_ns = CASE
                      WHEN ?3 = 'started' AND started_at_ns IS NULL THEN ?4
                      ELSE started_at_ns
                    END,
                    updated_at_ns = ?4
                WHERE exec_id = ?1
                "#,
                params![
                    exec_id.0,
                    event.exec_id.0,
                    exec_event_kind_name(event.kind),
                    now
                ],
            )?;
        }

        Ok(())
    }

    fn session_labels(
        &self,
        session_id: &SessionId,
    ) -> Result<BTreeMap<String, String>, FabricControllerError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT key, value FROM session_labels WHERE session_id = ?1 ORDER BY key")?;
        let labels = stmt
            .query_map(params![session_id.0], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<BTreeMap<String, String>, _>>()?;
        Ok(labels)
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, FabricControllerError> {
        self.conn
            .lock()
            .map_err(|_| FabricControllerError::Database(rusqlite::Error::InvalidQuery))
    }
}

fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS hosts (
          host_id TEXT PRIMARY KEY,
          endpoint TEXT NOT NULL,
          status TEXT NOT NULL,
          providers_json TEXT NOT NULL,
          labels_json TEXT NOT NULL,
          last_heartbeat_ns INTEGER,
          created_at_ns INTEGER NOT NULL,
          updated_at_ns INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
          session_id TEXT PRIMARY KEY,
          target_kind TEXT NOT NULL,
          target_json TEXT NOT NULL,
          host_id TEXT NOT NULL,
          host_session_id TEXT NOT NULL,
          status TEXT NOT NULL,
          workdir TEXT,
          supported_signals_json TEXT NOT NULL,
          created_at_ns INTEGER NOT NULL,
          updated_at_ns INTEGER NOT NULL,
          expires_at_ns INTEGER,
          closed_at_ns INTEGER,
          FOREIGN KEY(host_id) REFERENCES hosts(host_id)
        );
        CREATE INDEX IF NOT EXISTS sessions_host_id_idx ON sessions(host_id);
        CREATE INDEX IF NOT EXISTS sessions_status_idx ON sessions(status);

        CREATE TABLE IF NOT EXISTS session_labels (
          session_id TEXT NOT NULL,
          key TEXT NOT NULL,
          value TEXT NOT NULL,
          PRIMARY KEY(session_id, key),
          FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS session_labels_key_value_idx
          ON session_labels(key, value, session_id);
        CREATE INDEX IF NOT EXISTS session_labels_session_idx
          ON session_labels(session_id);

        CREATE TABLE IF NOT EXISTS execs (
          exec_id TEXT PRIMARY KEY,
          scope TEXT NOT NULL,
          request_id TEXT NOT NULL,
          session_id TEXT NOT NULL,
          host_id TEXT NOT NULL,
          status TEXT NOT NULL,
          request_json TEXT NOT NULL,
          host_exec_id TEXT,
          exit_code INTEGER,
          error_message TEXT,
          started_at_ns INTEGER,
          completed_at_ns INTEGER,
          created_at_ns INTEGER NOT NULL,
          updated_at_ns INTEGER NOT NULL,
          FOREIGN KEY(session_id) REFERENCES sessions(session_id),
          FOREIGN KEY(host_id) REFERENCES hosts(host_id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS execs_scope_request_id_idx
          ON execs(scope, request_id);
        CREATE INDEX IF NOT EXISTS execs_session_id_idx ON execs(session_id);
        CREATE INDEX IF NOT EXISTS execs_status_idx ON execs(status);

        CREATE TABLE IF NOT EXISTS exec_events (
          exec_id TEXT NOT NULL,
          seq INTEGER NOT NULL,
          event_json TEXT NOT NULL,
          created_at_ns INTEGER NOT NULL,
          PRIMARY KEY(exec_id, seq),
          FOREIGN KEY(exec_id) REFERENCES execs(exec_id)
        );

        CREATE TABLE IF NOT EXISTS idempotency (
          scope TEXT NOT NULL,
          request_id TEXT NOT NULL,
          operation TEXT NOT NULL,
          request_hash TEXT NOT NULL,
          status TEXT NOT NULL,
          resource_kind TEXT,
          resource_id TEXT,
          response_json TEXT,
          error_json TEXT,
          created_at_ns INTEGER NOT NULL,
          updated_at_ns INTEGER NOT NULL,
          expires_at_ns INTEGER,
          PRIMARY KEY(scope, request_id)
        );
        CREATE INDEX IF NOT EXISTS idempotency_expires_at_idx
          ON idempotency(expires_at_ns);

        CREATE TABLE IF NOT EXISTS host_inventory (
          host_id TEXT NOT NULL,
          session_id TEXT NOT NULL,
          inventory_json TEXT NOT NULL,
          observed_at_ns INTEGER NOT NULL,
          PRIMARY KEY(host_id, session_id),
          FOREIGN KEY(host_id) REFERENCES hosts(host_id)
        );
        "#,
    )?;

    if !table_has_column(conn, "sessions", "supported_signals_json")? {
        conn.execute(
            r#"
            ALTER TABLE sessions
            ADD COLUMN supported_signals_json TEXT NOT NULL DEFAULT '["quiesce","resume","close"]'
            "#,
            [],
        )?;
    }

    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn replace_inventory(
    conn: &Connection,
    inventory: &HostInventoryResponse,
    inventory_json: &str,
    observed_at_ns: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM host_inventory WHERE host_id = ?1",
        params![inventory.host_id.0],
    )?;

    for session in &inventory.sessions {
        conn.execute(
            r#"
            INSERT INTO host_inventory (
              host_id, session_id, inventory_json, observed_at_ns
            )
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                inventory.host_id.0,
                session.session_id.0,
                inventory_json,
                observed_at_ns,
            ],
        )?;
    }

    if inventory.sessions.is_empty() {
        conn.execute(
            r#"
            INSERT INTO host_inventory (
              host_id, session_id, inventory_json, observed_at_ns
            )
            VALUES (?1, '', ?2, ?3)
            "#,
            params![inventory.host_id.0, inventory_json, observed_at_ns],
        )?;
    }

    Ok(())
}

fn replace_session_labels_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &SessionId,
    labels: &BTreeMap<String, String>,
) -> Result<(), FabricControllerError> {
    tx.execute(
        "DELETE FROM session_labels WHERE session_id = ?1",
        params![session_id.0],
    )?;
    for (key, value) in labels {
        validate_label(key, value)?;
        tx.execute(
            r#"
            INSERT INTO session_labels (session_id, key, value)
            VALUES (?1, ?2, ?3)
            "#,
            params![session_id.0, key, value],
        )?;
    }
    Ok(())
}

fn validate_label(key: &str, value: &str) -> Result<(), FabricControllerError> {
    if key.trim().is_empty() {
        return Err(FabricControllerError::BadRequest(
            "label key must not be empty".to_owned(),
        ));
    }
    if value.is_empty() {
        return Err(FabricControllerError::BadRequest(format!(
            "label '{key}' value must not be empty"
        )));
    }
    Ok(())
}

fn session_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT s.session_id, s.status, s.target_kind, s.host_id,
               s.host_session_id, s.workdir, s.supported_signals_json,
               s.created_at_ns, s.updated_at_ns, s.expires_at_ns,
               s.closed_at_ns
        FROM sessions s
        WHERE 1 = 1
        {where_clause}
        ORDER BY s.created_at_ns, s.session_id
        "#
    )
}

fn session_from_row(row: &rusqlite::Row<'_>) -> Result<ControllerSessionSummary, rusqlite::Error> {
    let supported_signals_json: String = row.get(6)?;
    let created_at_ns: i64 = row.get(7)?;
    let updated_at_ns: i64 = row.get(8)?;
    let expires_at_ns: Option<i64> = row.get(9)?;
    let closed_at_ns: Option<i64> = row.get(10)?;

    Ok(ControllerSessionSummary {
        session_id: SessionId(row.get(0)?),
        status: parse_controller_status(row.get::<_, String>(1)?.as_str())
            .map_err(json_sql_error)?,
        target_kind: parse_target_kind(row.get::<_, String>(2)?.as_str())
            .map_err(json_sql_error)?,
        host_id: HostId(row.get(3)?),
        host_session_id: SessionId(row.get(4)?),
        workdir: row.get(5)?,
        supported_signals: serde_json::from_str(&supported_signals_json).map_err(json_sql_error)?,
        labels: BTreeMap::new(),
        created_at_ns: i64_to_u128(created_at_ns).map_err(json_sql_error)?,
        updated_at_ns: i64_to_u128(updated_at_ns).map_err(json_sql_error)?,
        expires_at_ns: expires_at_ns
            .map(i64_to_u128)
            .transpose()
            .map_err(json_sql_error)?,
        closed_at_ns: closed_at_ns
            .map(i64_to_u128)
            .transpose()
            .map_err(json_sql_error)?,
    })
}

fn host_from_row(row: &rusqlite::Row<'_>) -> Result<HostSummary, rusqlite::Error> {
    let providers_json: String = row.get(3)?;
    let labels_json: String = row.get(4)?;
    let last_heartbeat_ns: Option<i64> = row.get(5)?;
    let created_at_ns: i64 = row.get(6)?;
    let updated_at_ns: i64 = row.get(7)?;

    let providers: Vec<FabricHostProvider> =
        serde_json::from_str(&providers_json).map_err(json_sql_error)?;
    let labels = serde_json::from_str(&labels_json).map_err(json_sql_error)?;

    Ok(HostSummary {
        host_id: HostId(row.get(0)?),
        endpoint: row.get(1)?,
        status: parse_host_status(row.get::<_, String>(2)?.as_str()).map_err(json_sql_error)?,
        providers,
        labels,
        last_heartbeat_ns: last_heartbeat_ns
            .map(i64_to_u128)
            .transpose()
            .map_err(json_sql_error)?,
        created_at_ns: i64_to_u128(created_at_ns).map_err(json_sql_error)?,
        updated_at_ns: i64_to_u128(updated_at_ns).map_err(json_sql_error)?,
    })
}

fn now_ns_i64() -> Result<i64, FabricControllerError> {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| FabricControllerError::Time(error.to_string()))?
        .as_nanos();
    i64::try_from(ns).map_err(|_| FabricControllerError::Time("timestamp overflow".to_owned()))
}

fn u128_to_i64(value: u128) -> Result<i64, FabricControllerError> {
    i64::try_from(value).map_err(|_| FabricControllerError::Time("timestamp overflow".to_owned()))
}

fn i64_to_u128(value: i64) -> Result<u128, String> {
    u128::try_from(value).map_err(|_| format!("negative timestamp: {value}"))
}

fn host_status_name(status: HostStatus) -> &'static str {
    match status {
        HostStatus::Healthy => "healthy",
        HostStatus::Unhealthy => "unhealthy",
    }
}

fn controller_status_name(status: ControllerSessionStatus) -> &'static str {
    match status {
        ControllerSessionStatus::Creating => "creating",
        ControllerSessionStatus::Ready => "ready",
        ControllerSessionStatus::Quiesced => "quiesced",
        ControllerSessionStatus::Closing => "closing",
        ControllerSessionStatus::Closed => "closed",
        ControllerSessionStatus::Error => "error",
        ControllerSessionStatus::Lost => "lost",
        ControllerSessionStatus::HostUnreachable => "host_unreachable",
        ControllerSessionStatus::OrphanedHostSession => "orphaned_host_session",
    }
}

fn parse_controller_status(value: &str) -> Result<ControllerSessionStatus, String> {
    match value {
        "creating" => Ok(ControllerSessionStatus::Creating),
        "ready" => Ok(ControllerSessionStatus::Ready),
        "quiesced" => Ok(ControllerSessionStatus::Quiesced),
        "closing" => Ok(ControllerSessionStatus::Closing),
        "closed" => Ok(ControllerSessionStatus::Closed),
        "error" => Ok(ControllerSessionStatus::Error),
        "lost" => Ok(ControllerSessionStatus::Lost),
        "host_unreachable" => Ok(ControllerSessionStatus::HostUnreachable),
        "orphaned_host_session" => Ok(ControllerSessionStatus::OrphanedHostSession),
        _ => Err(format!("unknown controller session status: {value}")),
    }
}

fn controller_status_from_host_status(status: SessionStatus) -> ControllerSessionStatus {
    match status {
        SessionStatus::Creating => ControllerSessionStatus::Creating,
        SessionStatus::Ready => ControllerSessionStatus::Ready,
        SessionStatus::Quiesced => ControllerSessionStatus::Quiesced,
        SessionStatus::Closing => ControllerSessionStatus::Closing,
        SessionStatus::Closed => ControllerSessionStatus::Closed,
        SessionStatus::OrphanedWorkspace => ControllerSessionStatus::Lost,
        SessionStatus::Error => ControllerSessionStatus::Error,
    }
}

fn exec_status_name(kind: ExecEventKind) -> &'static str {
    match kind {
        ExecEventKind::Exit => "completed",
        ExecEventKind::Error => "error",
        ExecEventKind::Started | ExecEventKind::Stdout | ExecEventKind::Stderr => "running",
    }
}

fn exec_event_kind_name(kind: ExecEventKind) -> &'static str {
    match kind {
        ExecEventKind::Started => "started",
        ExecEventKind::Stdout => "stdout",
        ExecEventKind::Stderr => "stderr",
        ExecEventKind::Exit => "exit",
        ExecEventKind::Error => "error",
    }
}

fn target_kind_name(kind: FabricSessionTargetKind) -> &'static str {
    match kind {
        FabricSessionTargetKind::Sandbox => "sandbox",
        FabricSessionTargetKind::AttachedHost => "attached_host",
    }
}

fn parse_target_kind(value: &str) -> Result<FabricSessionTargetKind, String> {
    match value {
        "sandbox" => Ok(FabricSessionTargetKind::Sandbox),
        "attached_host" => Ok(FabricSessionTargetKind::AttachedHost),
        _ => Err(format!("unknown session target kind: {value}")),
    }
}

fn parse_host_status(value: &str) -> Result<HostStatus, String> {
    match value {
        "healthy" => Ok(HostStatus::Healthy),
        "unhealthy" => Ok(HostStatus::Unhealthy),
        _ => Err(format!("unknown host status: {value}")),
    }
}

fn json_sql_error(error: impl ToString) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    )))
}
