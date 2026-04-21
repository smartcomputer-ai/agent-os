use std::{
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};

use fabric_client::FabricHostClient;
use fabric_protocol::{
    ControllerExecRequest, ControllerInfoResponse, ControllerSessionListResponse,
    ControllerSessionOpenRequest, ControllerSessionOpenResponse, ControllerSessionStatus,
    ControllerSessionSummary, ControllerSignalSessionRequest, ExecEvent, ExecEventKind, ExecId,
    ExecRequest, FabricSessionSignal, FabricSessionTarget, FsApplyPatchRequest,
    FsApplyPatchResponse, FsEditFileRequest, FsEditFileResponse, FsExistsResponse,
    FsFileReadResponse, FsFileWriteRequest, FsGlobRequest, FsGlobResponse, FsGrepRequest,
    FsGrepResponse, FsListDirResponse, FsMkdirRequest, FsPathQuery, FsRemoveRequest,
    FsRemoveResponse, FsStatResponse, FsWriteResponse, HostHeartbeatRequest, HostId,
    HostInventoryResponse, HostListResponse, HostRegisterRequest, HostRegisterResponse, HostStatus,
    HostSummary, RequestId, SessionId, SessionLabelsPatchRequest, SessionLabelsResponse,
    SessionOpenRequest, SessionSignal, SessionStatus, SignalSessionRequest,
};
use futures_core::Stream;
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::{
    FabricControllerConfig, FabricControllerError, FabricControllerState,
    scheduler::schedule_sandbox,
    state::{IdempotencyStart, NewControllerExec, NewControllerSession},
};

pub type ControllerExecEventStream =
    Pin<Box<dyn Stream<Item = Result<ExecEvent, FabricControllerError>> + Send + 'static>>;

#[derive(Clone)]
pub struct FabricControllerService {
    config: FabricControllerConfig,
    state: FabricControllerState,
}

impl FabricControllerService {
    pub fn new(config: FabricControllerConfig, state: FabricControllerState) -> Self {
        Self { config, state }
    }

    pub fn config(&self) -> &FabricControllerConfig {
        &self.config
    }

    pub fn controller_info(&self) -> ControllerInfoResponse {
        ControllerInfoResponse {
            controller_version: env!("CARGO_PKG_VERSION").to_owned(),
            db_path: self.state.db_path().to_string_lossy().into_owned(),
            heartbeat_timeout_ns: self.config.host_heartbeat_timeout_ns,
            default_session_ttl_ns: self.config.default_session_ttl_ns,
            auth_mode: if self.config.allow_unauthenticated_loopback {
                "unauthenticated_loopback".to_owned()
            } else {
                "token_required".to_owned()
            },
        }
    }

    pub fn register_host(
        &self,
        request: HostRegisterRequest,
    ) -> Result<HostRegisterResponse, FabricControllerError> {
        let host = self.state.upsert_registered_host(&request)?;
        info!(
            host_id = %host.host_id.0,
            endpoint = %host.endpoint,
            provider_count = host.providers.len(),
            label_count = host.labels.len(),
            "fabric host registered"
        );
        Ok(HostRegisterResponse {
            host_id: host.host_id,
            status: HostStatus::Healthy,
            heartbeat_interval_ns: self.config.host_heartbeat_interval_ns,
            controller_time_ns: host.updated_at_ns,
        })
    }

    pub fn heartbeat_host(
        &self,
        host_id: HostId,
        request: HostHeartbeatRequest,
    ) -> Result<HostRegisterResponse, FabricControllerError> {
        if request.host_id != host_id {
            return Err(FabricControllerError::BadRequest(format!(
                "heartbeat host_id '{}' does not match route host_id '{}'",
                request.host_id.0, host_id.0
            )));
        }

        let host = self.state.record_heartbeat(&request)?;
        debug!(
            host_id = %host.host_id.0,
            endpoint = %host.endpoint,
            provider_count = host.providers.len(),
            inventory_sessions = request.inventory.as_ref().map_or(0, |inventory| inventory.sessions.len()),
            "fabric host heartbeat"
        );
        Ok(HostRegisterResponse {
            host_id: host.host_id,
            status: HostStatus::Healthy,
            heartbeat_interval_ns: self.config.host_heartbeat_interval_ns,
            controller_time_ns: host.updated_at_ns,
        })
    }

    pub fn list_hosts(&self) -> Result<HostListResponse, FabricControllerError> {
        Ok(HostListResponse {
            hosts: self.state.list_hosts()?,
        })
    }

    pub fn host(&self, host_id: &HostId) -> Result<HostSummary, FabricControllerError> {
        self.state
            .host(host_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("host {}", host_id.0)))
    }

    pub fn host_inventory(
        &self,
        host_id: &HostId,
    ) -> Result<HostInventoryResponse, FabricControllerError> {
        self.state
            .inventory(host_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("host inventory {}", host_id.0)))
    }

    pub async fn open_session(
        &self,
        request: ControllerSessionOpenRequest,
    ) -> Result<ControllerSessionOpenResponse, FabricControllerError> {
        let FabricSessionTarget::Sandbox(target) = request.target.clone() else {
            return Err(FabricControllerError::UnsupportedTarget(
                "attached_host targets are not implemented in P2 yet".to_owned(),
            ));
        };
        if target.image.trim().is_empty() {
            return Err(FabricControllerError::BadRequest(
                "sandbox image must not be empty".to_owned(),
            ));
        }

        let scope = "dev";
        let request_id = request.request_id.clone();
        let request_hash = request_hash("session_open", &request)?;
        let session_id = match &request_id {
            Some(request_id) => deterministic_session_id(scope, request_id),
            None => SessionId(format!("sess-{}", uuid::Uuid::new_v4())),
        };

        if let Some(request_id) = &request_id {
            match self.state.begin_idempotency(
                scope,
                request_id,
                "session_open",
                &request_hash,
                Some("session"),
                Some(&session_id.0),
                None,
            )? {
                IdempotencyStart::Acquired => {}
                IdempotencyStart::Replay(response_json) => {
                    info!(
                        request_id = %request_id.0,
                        session_id = %session_id.0,
                        "replaying idempotent fabric session open"
                    );
                    return serde_json::from_str(&response_json).map_err(Into::into);
                }
            }
        }

        let result = self
            .open_session_uncached(request, target, session_id.clone())
            .await;
        if let Some(request_id) = &request_id {
            match &result {
                Ok(response) => {
                    self.state.complete_idempotency(
                        scope,
                        request_id,
                        &serde_json::to_string(response)?,
                    )?;
                }
                Err(error) => {
                    warn!(
                        request_id = %request_id.0,
                        session_id = %session_id.0,
                        error = %error,
                        "fabric session open failed"
                    );
                    let _ = self.state.fail_idempotency(
                        scope,
                        request_id,
                        &serde_json::to_string(&fabric_protocol::ErrorResponse {
                            code: "session_open_failed".to_owned(),
                            message: error.to_string(),
                        })?,
                    );
                }
            }
        }

        result
    }

    async fn open_session_uncached(
        &self,
        request: ControllerSessionOpenRequest,
        target: fabric_protocol::FabricSandboxTarget,
        session_id: SessionId,
    ) -> Result<ControllerSessionOpenResponse, FabricControllerError> {
        let hosts = self.state.list_hosts()?;
        let scheduled = schedule_sandbox(&hosts, &target, self.config.host_heartbeat_timeout_ns)?;
        info!(
            session_id = %session_id.0,
            host_id = %scheduled.host.host_id.0,
            endpoint = %scheduled.host.endpoint,
            image = %target.image,
            runtime_class = ?target.runtime_class,
            "fabric session scheduled"
        );
        let supported_signals = vec![
            fabric_protocol::FabricSessionSignalKind::Quiesce,
            fabric_protocol::FabricSessionSignalKind::Resume,
            fabric_protocol::FabricSessionSignalKind::Close,
        ];
        let expires_at_ns = expires_at_ns(request.ttl_ns, self.config.default_session_ttl_ns)?;

        let creating = self.state.insert_session(&NewControllerSession {
            session_id: session_id.clone(),
            target: request.target.clone(),
            host_id: scheduled.host.host_id.clone(),
            host_session_id: session_id.clone(),
            status: ControllerSessionStatus::Creating,
            workdir: target.workdir.clone(),
            supported_signals: supported_signals.clone(),
            labels: request.labels.clone(),
            expires_at_ns,
        })?;

        let host_request = SessionOpenRequest {
            session_id: Some(session_id.clone()),
            image: target.image,
            runtime_class: target.runtime_class,
            workdir: target.workdir,
            env: target.env,
            network_mode: target.network_mode,
            mounts: target.mounts,
            resources: target.resources,
            ttl_secs: ttl_ns_to_secs(request.ttl_ns.or(self.config.default_session_ttl_ns)),
            labels: request.labels,
        };

        let host_response = FabricHostClient::new(scheduled.host.endpoint.clone())
            .open_session(&host_request)
            .await
            .map_err(|error| {
                FabricControllerError::HostError(format!(
                    "open session on host '{}': {error}",
                    scheduled.host.host_id.0
                ))
            });

        let host_response = match host_response {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    session_id = %session_id.0,
                    host_id = %scheduled.host.host_id.0,
                    error = %error,
                    "fabric host failed to open session"
                );
                let _ = self
                    .state
                    .update_session_status(&session_id, ControllerSessionStatus::Error);
                return Err(error);
            }
        };

        let opened = self.state.update_session_opened(
            &session_id,
            controller_status_from_host(host_response.status),
            &host_response.workdir,
        )?;
        info!(
            session_id = %opened.session_id.0,
            host_id = %opened.host_id.0,
            host_session_id = %opened.host_session_id.0,
            status = ?opened.status,
            workdir = ?opened.workdir,
            "fabric session opened"
        );

        Ok(open_response_from_summary(
            opened,
            creating.supported_signals,
        ))
    }

    pub fn session(
        &self,
        session_id: &SessionId,
    ) -> Result<ControllerSessionSummary, FabricControllerError> {
        self.state
            .session(session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("session {}", session_id.0)))
    }

    pub fn list_sessions(
        &self,
        label_filters: &[(String, String)],
    ) -> Result<ControllerSessionListResponse, FabricControllerError> {
        Ok(ControllerSessionListResponse {
            sessions: self.state.list_sessions(label_filters)?,
        })
    }

    pub fn patch_session_labels(
        &self,
        session_id: &SessionId,
        request: SessionLabelsPatchRequest,
    ) -> Result<SessionLabelsResponse, FabricControllerError> {
        Ok(SessionLabelsResponse {
            session_id: session_id.clone(),
            labels: self
                .state
                .patch_session_labels(session_id, &request.set, &request.remove)?,
        })
    }

    pub async fn exec_session_stream(
        &self,
        session_id: SessionId,
        request: ControllerExecRequest,
    ) -> Result<ControllerExecEventStream, FabricControllerError> {
        if request.argv.is_empty() {
            return Err(FabricControllerError::BadRequest(
                "exec argv must not be empty".to_owned(),
            ));
        }

        let session = self
            .state
            .session(&session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("session {}", session_id.0)))?;
        let host = self.state.host(&session.host_id)?.ok_or_else(|| {
            FabricControllerError::NotFound(format!("host {}", session.host_id.0))
        })?;
        let host_request = ExecRequest {
            session_id: session.host_session_id.clone(),
            argv: request.argv.clone(),
            cwd: request.cwd.clone(),
            env: request.env_patch.clone(),
            stdin: request.stdin.clone().map(Into::into),
            timeout_secs: timeout_ns_to_secs(request.timeout_ns),
        };

        let Some(request_id) = request.request_id.clone() else {
            info!(
                session_id = %session_id.0,
                host_id = %host.host_id.0,
                argv = ?request.argv,
                "starting non-idempotent fabric exec"
            );
            let host_stream = FabricHostClient::new(host.endpoint.clone())
                .exec_session_stream(&host_request)
                .await
                .map_err(|error| {
                    FabricControllerError::HostError(format!(
                        "exec session on host '{}': {error}",
                        host.host_id.0
                    ))
                })?;
            return Ok(Box::pin(host_stream.map(|event| {
                event.map_err(|error| FabricControllerError::HostError(error.to_string()))
            })));
        };

        let scope = "dev";
        let exec_id = deterministic_exec_id(scope, &request_id);
        let request_hash = request_hash("session_exec", &(session_id.clone(), &request))?;
        match self.state.begin_idempotency(
            scope,
            &request_id,
            "session_exec",
            &request_hash,
            Some("exec"),
            Some(&exec_id.0),
            None,
        )? {
            IdempotencyStart::Acquired => {}
            IdempotencyStart::Replay(response_json) => {
                let events: Vec<ExecEvent> = serde_json::from_str(&response_json)?;
                info!(
                    session_id = %session_id.0,
                    host_id = %host.host_id.0,
                    exec_id = %exec_id.0,
                    request_id = %request_id.0,
                    event_count = events.len(),
                    "replaying idempotent fabric exec"
                );
                return Ok(Box::pin(stream::iter(events.into_iter().map(Ok))));
            }
        }

        info!(
            session_id = %session_id.0,
            host_id = %host.host_id.0,
            exec_id = %exec_id.0,
            request_id = %request_id.0,
            argv = ?request.argv,
            "starting idempotent fabric exec"
        );
        self.state.insert_exec(&NewControllerExec {
            exec_id: exec_id.clone(),
            scope: scope.to_owned(),
            request_id: request_id.clone(),
            session_id: session_id.clone(),
            host_id: session.host_id.clone(),
            request: request.clone(),
        })?;

        let host_stream = FabricHostClient::new(host.endpoint.clone())
            .exec_session_stream(&host_request)
            .await
            .map_err(|error| {
                FabricControllerError::HostError(format!(
                    "exec session on host '{}': {error}",
                    host.host_id.0
                ))
            })?;

        Ok(Box::pin(stream::unfold(
            (
                host_stream,
                self.state.clone(),
                scope.to_owned(),
                request_id,
                exec_id,
                Vec::<ExecEvent>::new(),
                false,
            ),
            |(mut host_stream, state, scope, request_id, exec_id, mut events, done)| async move {
                if done {
                    return None;
                }

                match host_stream.next().await {
                    Some(Ok(event)) => {
                        let append_result = state.append_exec_event(&exec_id, &event);
                        if let Err(error) = append_result {
                            return Some((
                                Err(error),
                                (host_stream, state, scope, request_id, exec_id, events, true),
                            ));
                        }

                        let terminal =
                            matches!(event.kind, ExecEventKind::Exit | ExecEventKind::Error);
                        events.push(event.clone());
                        if terminal {
                            match event.kind {
                                ExecEventKind::Exit => info!(
                                    exec_id = %exec_id.0,
                                    host_exec_id = %event.exec_id.0,
                                    exit_code = ?event.exit_code,
                                    event_count = events.len(),
                                    "fabric exec exited"
                                ),
                                ExecEventKind::Error => warn!(
                                    exec_id = %exec_id.0,
                                    host_exec_id = %event.exec_id.0,
                                    message = ?event.message,
                                    event_count = events.len(),
                                    "fabric exec errored"
                                ),
                                _ => {}
                            }
                            match serde_json::to_string(&events) {
                                Ok(response_json) => {
                                    let _ = state.complete_idempotency(
                                        &scope,
                                        &request_id,
                                        &response_json,
                                    );
                                }
                                Err(error) => {
                                    let _ = state.fail_idempotency(
                                        &scope,
                                        &request_id,
                                        &error.to_string(),
                                    );
                                }
                            }
                        }

                        Some((
                            Ok(event),
                            (
                                host_stream,
                                state,
                                scope,
                                request_id,
                                exec_id,
                                events,
                                terminal,
                            ),
                        ))
                    }
                    Some(Err(error)) => {
                        let controller_error = FabricControllerError::HostError(error.to_string());
                        warn!(
                            exec_id = %exec_id.0,
                            error = %controller_error,
                            "fabric exec stream failed"
                        );
                        let _ = state.fail_idempotency(
                            &scope,
                            &request_id,
                            &controller_error.to_string(),
                        );
                        Some((
                            Err(controller_error),
                            (host_stream, state, scope, request_id, exec_id, events, true),
                        ))
                    }
                    None => None,
                }
            },
        )))
    }

    pub async fn signal_session(
        &self,
        session_id: &SessionId,
        request: ControllerSignalSessionRequest,
    ) -> Result<ControllerSessionSummary, FabricControllerError> {
        let session = self
            .state
            .session(session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("session {}", session_id.0)))?;
        let (host_action, capability_name) =
            host_signal_from_controller_signal(&request.signal, &session.supported_signals)?;

        let host = self.state.host(&session.host_id)?.ok_or_else(|| {
            FabricControllerError::NotFound(format!("host {}", session.host_id.0))
        })?;
        info!(
            session_id = %session_id.0,
            host_id = %session.host_id.0,
            signal = capability_name,
            "signaling fabric session"
        );
        let host_response = FabricHostClient::new(host.endpoint)
            .signal_session(
                &session.host_session_id,
                &SignalSessionRequest {
                    action: host_action,
                },
            )
            .await
            .map_err(|error| {
                warn!(
                    session_id = %session_id.0,
                    host_id = %session.host_id.0,
                    signal = capability_name,
                    error = %error,
                    "fabric session signal failed"
                );
                FabricControllerError::HostError(format!(
                    "signal {capability_name} on host '{}': {error}",
                    session.host_id.0
                ))
            })?;

        let updated = self.state.update_session_signaled(
            session_id,
            controller_status_from_host(host_response.status),
        )?;
        info!(
            session_id = %updated.session_id.0,
            host_id = %updated.host_id.0,
            signal = capability_name,
            status = ?updated.status,
            "fabric session signaled"
        );
        Ok(updated)
    }

    pub async fn read_file(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsFileReadResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .read_file(&host_session_id, query)
            .await
            .map_err(host_client_error)
    }

    pub async fn write_file(
        &self,
        session_id: &SessionId,
        request: &FsFileWriteRequest,
    ) -> Result<FsWriteResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .write_file(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn edit_file(
        &self,
        session_id: &SessionId,
        request: &FsEditFileRequest,
    ) -> Result<FsEditFileResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .edit_file(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn apply_patch(
        &self,
        session_id: &SessionId,
        request: &FsApplyPatchRequest,
    ) -> Result<FsApplyPatchResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .apply_patch(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn mkdir(
        &self,
        session_id: &SessionId,
        request: &FsMkdirRequest,
    ) -> Result<FsStatResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .mkdir(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn remove(
        &self,
        session_id: &SessionId,
        request: &FsRemoveRequest,
    ) -> Result<FsRemoveResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .remove(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn exists(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsExistsResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .exists(&host_session_id, query)
            .await
            .map_err(host_client_error)
    }

    pub async fn stat(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsStatResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .stat(&host_session_id, query)
            .await
            .map_err(host_client_error)
    }

    pub async fn list_dir(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsListDirResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .list_dir(&host_session_id, query)
            .await
            .map_err(host_client_error)
    }

    pub async fn grep(
        &self,
        session_id: &SessionId,
        request: &FsGrepRequest,
    ) -> Result<FsGrepResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .grep(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    pub async fn glob(
        &self,
        session_id: &SessionId,
        request: &FsGlobRequest,
    ) -> Result<FsGlobResponse, FabricControllerError> {
        let (host_session_id, client) = self.host_client_for_session(session_id)?;
        client
            .glob(&host_session_id, request)
            .await
            .map_err(host_client_error)
    }

    fn host_client_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(SessionId, FabricHostClient), FabricControllerError> {
        let session = self
            .state
            .session(session_id)?
            .ok_or_else(|| FabricControllerError::NotFound(format!("session {}", session_id.0)))?;
        let host = self.state.host(&session.host_id)?.ok_or_else(|| {
            FabricControllerError::NotFound(format!("host {}", session.host_id.0))
        })?;

        Ok((
            session.host_session_id,
            FabricHostClient::new(host.endpoint),
        ))
    }
}

fn request_hash<T: serde::Serialize>(
    operation: &str,
    request: &T,
) -> Result<String, FabricControllerError> {
    let data = serde_json::to_vec(&(operation, request))?;
    Ok(hex::encode(Sha256::digest(data)))
}

fn deterministic_session_id(scope: &str, request_id: &RequestId) -> SessionId {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(request_id.0.as_bytes());
    let digest = hex::encode(hasher.finalize());
    SessionId(format!("sess-{}", &digest[..16]))
}

fn deterministic_exec_id(scope: &str, request_id: &RequestId) -> ExecId {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(request_id.0.as_bytes());
    let digest = hex::encode(hasher.finalize());
    ExecId(format!("exec-{}", &digest[..16]))
}

fn expires_at_ns(
    ttl_ns: Option<u128>,
    default_ttl_ns: Option<u128>,
) -> Result<Option<u128>, FabricControllerError> {
    let Some(ttl_ns) = ttl_ns.or(default_ttl_ns) else {
        return Ok(None);
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| FabricControllerError::Time(error.to_string()))?
        .as_nanos();
    Ok(Some(now.saturating_add(ttl_ns)))
}

fn ttl_ns_to_secs(ttl_ns: Option<u128>) -> Option<u64> {
    ttl_ns.and_then(|ttl_ns| {
        let secs = ttl_ns.div_ceil(1_000_000_000);
        u64::try_from(secs).ok()
    })
}

fn timeout_ns_to_secs(timeout_ns: Option<u128>) -> Option<u64> {
    ttl_ns_to_secs(timeout_ns)
}

fn controller_status_from_host(status: SessionStatus) -> ControllerSessionStatus {
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

fn host_signal_from_controller_signal(
    signal: &FabricSessionSignal,
    supported_signals: &[fabric_protocol::FabricSessionSignalKind],
) -> Result<(SessionSignal, &'static str), FabricControllerError> {
    match signal {
        FabricSessionSignal::Quiesce(_)
            if supported_signals.contains(&fabric_protocol::FabricSessionSignalKind::Quiesce) =>
        {
            Ok((SessionSignal::Quiesce, "quiesce"))
        }
        FabricSessionSignal::Quiesce(_) => Err(unsupported_lifecycle("quiesce")),
        FabricSessionSignal::Resume(_)
            if supported_signals.contains(&fabric_protocol::FabricSessionSignalKind::Resume) =>
        {
            Ok((SessionSignal::Resume, "resume"))
        }
        FabricSessionSignal::Resume(_) => Err(unsupported_lifecycle("resume")),
        FabricSessionSignal::Close(_)
            if supported_signals.contains(&fabric_protocol::FabricSessionSignalKind::Close) =>
        {
            Ok((SessionSignal::Close, "close"))
        }
        FabricSessionSignal::Close(_) => Err(unsupported_lifecycle("close")),
        FabricSessionSignal::TerminateRuntime(_)
            if supported_signals
                .contains(&fabric_protocol::FabricSessionSignalKind::TerminateRuntime) =>
        {
            Ok((SessionSignal::Terminate, "terminate_runtime"))
        }
        FabricSessionSignal::TerminateRuntime(_) => Err(unsupported_lifecycle("terminate_runtime")),
    }
}

fn unsupported_lifecycle(signal: &'static str) -> FabricControllerError {
    FabricControllerError::UnsupportedLifecycle(format!("session does not support {signal}"))
}

fn open_response_from_summary(
    summary: ControllerSessionSummary,
    supported_signals: Vec<fabric_protocol::FabricSessionSignalKind>,
) -> ControllerSessionOpenResponse {
    ControllerSessionOpenResponse {
        session_id: summary.session_id,
        status: summary.status,
        target_kind: summary.target_kind,
        host_id: summary.host_id,
        host_session_id: summary.host_session_id,
        workdir: summary.workdir.unwrap_or_else(|| "/workspace".to_owned()),
        supported_signals,
        created_at_ns: summary.created_at_ns,
        expires_at_ns: summary.expires_at_ns,
    }
}

fn host_client_error(error: fabric_client::FabricClientError) -> FabricControllerError {
    FabricControllerError::HostError(error.to_string())
}
