use aos_cbor::Hash;
use aos_node::{
    CreateUniverseRequest, CreateWorldSeedRequest, ForkWorldRequest, PersistConflict, PersistError,
    PutSecretVersionRequest, SecretAuditRecord, SecretBindingRecord, SecretBindingSourceKind,
    SecretBindingStatus, SecretStore, SecretVersionRecord, SecretVersionStatus,
    UniverseAdminStatus, UniverseCreateResult, UniverseId, UniverseRecord, UniverseStore,
    WorldAdminLifecycle, WorldAdminStore, WorldCreateResult, WorldForkResult, WorldId,
    WorldLineage, WorldSeed, default_world_handle, normalize_handle,
    rewrite_snapshot_for_fork_policy, validate_create_world_seed_request,
    validate_fork_world_request,
};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::SqliteNodeStore;
use super::util::{
    decode, encode, ensure_universe_for_world, ensure_universe_handle_available,
    ensure_world_handle_available, get_universe_row, get_world_row, insert_world_from_seed,
    resolve_snapshot_selector,
};

impl UniverseStore for SqliteNodeStore {
    fn create_universe(
        &self,
        request: CreateUniverseRequest,
    ) -> Result<UniverseCreateResult, PersistError> {
        let record = self.get_universe(self.local_universe_id())?;
        if let Some(universe_id) = request.universe_id
            && universe_id != record.universe_id
        {
            return Err(PersistConflict::UniverseExists {
                universe_id: record.universe_id,
            }
            .into());
        }
        if let Some(handle) = request.handle {
            let handle = normalize_handle(&handle)?;
            if handle != record.meta.handle {
                return Err(PersistConflict::UniverseHandleExists {
                    handle,
                    universe_id: record.universe_id,
                }
                .into());
            }
        }
        Ok(UniverseCreateResult { record })
    }

    fn delete_universe(
        &self,
        universe: UniverseId,
        deleted_at_ns: u64,
    ) -> Result<UniverseRecord, PersistError> {
        let _ = deleted_at_ns;
        self.ensure_local_universe(universe)?;
        Err(PersistError::validation(
            "aos-node-local cannot delete its singleton universe",
        ))
    }

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            get_universe_row(conn, universe)?
                .ok_or_else(|| PersistError::not_found(format!("universe {universe}")))
        })
    }

    fn get_universe_by_handle(&self, handle: &str) -> Result<UniverseRecord, PersistError> {
        let handle = normalize_handle(handle)?;
        self.read(|conn| {
            let record = get_universe_row(conn, self.local_universe_id())?
                .ok_or_else(|| PersistError::not_found(format!("universe handle '{handle}'")))?;
            if record.meta.handle == handle {
                Ok(record)
            } else {
                Err(PersistError::not_found(format!(
                    "universe handle '{handle}'"
                )))
            }
        })
    }

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseRecord>, PersistError> {
        if limit == 0 || after.is_some_and(|after| after >= self.local_universe_id()) {
            return Ok(Vec::new());
        }
        Ok(vec![self.get_universe(self.local_universe_id())?])
    }

    fn set_universe_handle(
        &self,
        universe: UniverseId,
        handle: String,
    ) -> Result<UniverseRecord, PersistError> {
        self.write(|tx| {
            self.ensure_local_universe(universe)?;
            let handle = normalize_handle(&handle)?;
            let Some(mut record) = get_universe_row(tx, universe)? else {
                return Err(PersistError::not_found(format!("universe {universe}")));
            };
            if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                return Err(PersistConflict::UniverseAdminBlocked {
                    universe_id: universe,
                    status: record.admin.status,
                    action: "set_universe_handle".into(),
                }
                .into());
            }
            if record.meta.handle == handle {
                return Ok(record);
            }
            ensure_universe_handle_available(tx, universe, &handle)?;
            record.meta.handle = handle.clone();
            tx.execute(
                "update local_meta set universe_handle = ?1 where singleton = 1",
                params![handle],
            )
            .map_err(|err| PersistError::backend(format!("update universe handle: {err}")))?;
            Ok(record)
        })
    }
}

impl WorldAdminStore for SqliteNodeStore {
    fn world_create_from_seed(
        &self,
        universe: UniverseId,
        request: CreateWorldSeedRequest,
    ) -> Result<WorldCreateResult, PersistError> {
        validate_create_world_seed_request(&request)?;
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            ensure_universe_for_world(tx, universe)?;
            let world_id = request
                .world_id
                .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
            let handle = match request.handle {
                Some(handle) => normalize_handle(&handle)?,
                None => default_world_handle(world_id),
            };
            ensure_world_handle_available(tx, universe, world_id, &handle)?;
            let record = insert_world_from_seed(
                tx,
                &self.cas,
                universe,
                world_id,
                &request.seed,
                handle.clone(),
                request.placement_pin.clone(),
                request.created_at_ns,
                match &request.seed.imported_from {
                    Some(imported_from) => WorldLineage::Import {
                        created_at_ns: request.created_at_ns,
                        source: imported_from.source.clone(),
                        external_world_id: imported_from.external_world_id.clone(),
                        external_snapshot_ref: imported_from.external_snapshot_ref.clone(),
                    },
                    None => WorldLineage::Genesis {
                        created_at_ns: request.created_at_ns,
                    },
                },
            )?;
            let mut record = record;
            record.meta.placement_pin = request.placement_pin;
            Ok(WorldCreateResult { record })
        })
    }

    fn world_prepare_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: aos_node::WorldId,
        manifest_hash: Hash,
        handle: String,
        placement_pin: Option<String>,
        created_at_ns: u64,
        lineage: WorldLineage,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            ensure_universe_for_world(tx, universe)?;
            if !self.cas.has(manifest_hash) {
                return Err(PersistError::not_found(format!(
                    "manifest {} in universe {}",
                    manifest_hash.to_hex(),
                    universe
                )));
            }
            let handle = normalize_handle(&handle)?;
            ensure_world_handle_available(tx, universe, world, &handle)?;
            tx.execute(
                "insert into local_worlds (
                    world_id, handle, manifest_hash, active_baseline_height, placement_pin,
                    created_at_ns, lineage, admin, journal_head, inbox_cursor, next_inbox_seq,
                    notify_counter, pending_effects_count, next_timer_due_at_ns
                ) values (?1, ?2, ?3, null, ?4, ?5, ?6, ?7, 0, null, 0, 0, 0, null)",
                params![
                    world.to_string(),
                    handle,
                    manifest_hash.to_hex(),
                    placement_pin,
                    created_at_ns,
                    Some(encode(&lineage)?),
                    encode(&WorldAdminLifecycle::default())?,
                ],
            )
            .map_err(map_sql_world_insert_conflict(world))?;
            tx.execute(
                "insert into local_world_handles (handle, world_id) values (?1, ?2)",
                params![handle, world.to_string()],
            )
            .map_err(map_sql_world_insert_conflict(world))?;
            Ok(())
        })
    }

    fn world_drop_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: aos_node::WorldId,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let row = match get_world_row(tx, universe, world) {
                Ok(row) => row,
                Err(PersistError::NotFound(_)) => return Ok(()),
                Err(err) => return Err(err),
            };
            let snapshot_count: i64 = tx
                .query_row(
                    "select count(*) from local_snapshots where world_id = ?1",
                    params![world.to_string()],
                    |row| row.get(0),
                )
                .map_err(|err| PersistError::backend(format!("count snapshots: {err}")))?;
            if row.meta.active_baseline_height.is_some() || snapshot_count > 0 {
                return Ok(());
            }
            tx.execute(
                "delete from local_world_handles where world_id = ?1",
                params![world.to_string()],
            )
            .map_err(|err| PersistError::backend(format!("drop bootstrap world handle: {err}")))?;
            tx.execute(
                "delete from local_worlds where world_id = ?1",
                params![world.to_string()],
            )
            .map_err(|err| {
                PersistError::backend(format!("drop manifest bootstrap world: {err}"))
            })?;
            Ok(())
        })
    }

    fn world_fork(
        &self,
        universe: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<WorldForkResult, PersistError> {
        validate_fork_world_request(&request)?;
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let baseline = resolve_snapshot_selector(
                tx,
                universe,
                request.src_world_id,
                &request.src_snapshot,
            )?;
            let snapshot_hash = Hash::from_hex_str(&baseline.snapshot_ref)
                .map_err(|err| PersistError::validation(format!("invalid snapshot_ref: {err}")))?;
            let snapshot_bytes = self.cas.get(snapshot_hash)?;
            let snapshot_ref = match rewrite_snapshot_for_fork_policy(
                &snapshot_bytes,
                &request.pending_effect_policy,
            )? {
                Some(bytes) => self.cas.put_verified(&bytes)?.to_hex(),
                None => baseline.snapshot_ref.clone(),
            };
            let src_row = get_world_row(tx, universe, request.src_world_id)?;
            let world_id = request
                .new_world_id
                .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
            let handle = match request.handle {
                Some(handle) => normalize_handle(&handle)?,
                None => default_world_handle(world_id),
            };
            ensure_world_handle_available(tx, universe, world_id, &handle)?;
            let mut seed = WorldSeed {
                baseline: baseline.clone(),
                seed_kind: aos_node::SeedKind::Import,
                imported_from: Some(aos_node::ImportedSeedSource {
                    source: "fork".into(),
                    external_world_id: Some(request.src_world_id.to_string()),
                    external_snapshot_ref: Some(baseline.snapshot_ref.clone()),
                }),
            };
            seed.baseline.snapshot_ref = snapshot_ref.clone();
            let record = insert_world_from_seed(
                tx,
                &self.cas,
                universe,
                world_id,
                &seed,
                handle,
                request
                    .placement_pin
                    .clone()
                    .or(src_row.meta.placement_pin.clone()),
                request.forked_at_ns,
                WorldLineage::Fork {
                    forked_at_ns: request.forked_at_ns,
                    src_universe_id: universe,
                    src_world_id: request.src_world_id,
                    src_snapshot_ref: baseline.snapshot_ref,
                    src_height: baseline.height,
                },
            )?;
            Ok(WorldCreateResult { record })
        })
    }

    fn set_world_handle(
        &self,
        universe: UniverseId,
        world: aos_node::WorldId,
        handle: String,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let handle = normalize_handle(&handle)?;
            let row = get_world_row(tx, universe, world)?;
            if row.meta.admin.status.blocks_world_operations() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: row.meta.admin.status,
                    action: "set_world_handle".into(),
                }
                .into());
            }
            ensure_world_handle_available(tx, universe, world, &handle)?;
            tx.execute(
                "insert into local_world_handles (handle, world_id) values (?1, ?2)
                 on conflict(world_id) do update set handle = excluded.handle",
                params![handle.clone(), world.to_string()],
            )
            .map_err(map_sql_world_handle_conflict(
                universe,
                world,
                handle.clone(),
            ))?;
            tx.execute(
                "update local_worlds set handle = ?2 where world_id = ?1",
                params![world.to_string(), handle],
            )
            .map_err(|err| PersistError::backend(format!("set world handle: {err}")))?;
            Ok(())
        })
    }
}

impl SecretStore for SqliteNodeStore {
    fn put_secret_binding(
        &self,
        universe: UniverseId,
        mut record: SecretBindingRecord,
    ) -> Result<SecretBindingRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            ensure_universe_for_world(tx, universe)?;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "select record from local_secret_bindings where binding_id = ?1",
                    params![record.binding_id.clone()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| PersistError::backend(format!("query secret binding: {err}")))?;
            if let Some(existing) = existing {
                let existing: SecretBindingRecord = decode(&existing)?;
                record.created_at_ns = existing.created_at_ns;
                if record.latest_version.is_none() {
                    record.latest_version = existing.latest_version;
                }
            }
            if record.updated_at_ns == 0 {
                record.updated_at_ns = record.created_at_ns;
            }
            tx.execute(
                "insert into local_secret_bindings (binding_id, record) values (?1, ?2)
                 on conflict(binding_id) do update set record = excluded.record",
                params![record.binding_id.clone(), encode(&record)?],
            )
            .map_err(|err| PersistError::backend(format!("upsert secret binding: {err}")))?;
            Ok(record)
        })
    }

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            conn.query_row(
                "select record from local_secret_bindings where binding_id = ?1",
                params![binding_id],
                |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                },
            )
            .optional()
            .map_err(|err| PersistError::backend(format!("load secret binding: {err}")))
        })
    }

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let mut stmt = conn
                .prepare("select record from local_secret_bindings order by binding_id limit ?1")
                .map_err(|err| {
                    PersistError::backend(format!("prepare list secret bindings: {err}"))
                })?;
            let rows = stmt
                .query_map(params![limit], |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                })
                .map_err(|err| {
                    PersistError::backend(format!("query list secret bindings: {err}"))
                })?;
            rows.map(|row| {
                row.map_err(|err| PersistError::backend(format!("read secret binding row: {err}")))
            })
            .collect()
        })
    }

    fn disable_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        updated_at_ns: u64,
    ) -> Result<SecretBindingRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let bytes: Vec<u8> = tx
                .query_row(
                    "select record from local_secret_bindings where binding_id = ?1",
                    params![binding_id],
                    |row| row.get(0),
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PersistError::not_found(format!("secret binding '{binding_id}'"))
                    }
                    other => {
                        PersistError::backend(format!("load secret binding for disable: {other}"))
                    }
                })?;
            let mut record: SecretBindingRecord = decode(&bytes)?;
            record.status = SecretBindingStatus::Disabled;
            record.updated_at_ns = updated_at_ns;
            tx.execute(
                "update local_secret_bindings set record = ?2 where binding_id = ?1",
                params![binding_id, encode(&record)?],
            )
            .map_err(|err| PersistError::backend(format!("disable secret binding: {err}")))?;
            Ok(record)
        })
    }

    fn put_secret_version(
        &self,
        universe: UniverseId,
        request: PutSecretVersionRequest,
    ) -> Result<SecretVersionRecord, PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            let binding_bytes: Vec<u8> = tx
                .query_row(
                    "select record from local_secret_bindings where binding_id = ?1",
                    params![request.binding_id.clone()],
                    |row| row.get(0),
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PersistError::not_found(format!("secret binding '{}'", request.binding_id))
                    }
                    other => PersistError::backend(format!("load secret binding for version: {other}")),
                })?;
            let mut binding: SecretBindingRecord = decode(&binding_bytes)?;
            if !matches!(binding.status, SecretBindingStatus::Active) {
                return Err(PersistError::validation(format!(
                    "secret binding '{}' is disabled",
                    request.binding_id
                )));
            }
            if !matches!(binding.source_kind, SecretBindingSourceKind::NodeSecretStore) {
                return Err(PersistError::validation(format!(
                    "secret binding '{}' is not node_secret_store",
                    request.binding_id
                )));
            }
            if let Some(previous) = binding.latest_version {
                let bytes: Vec<u8> = tx
                    .query_row(
                        "select record from local_secret_versions where binding_id = ?1 and version = ?2",
                        params![request.binding_id.clone(), previous],
                        |row| row.get(0),
                    )
                    .map_err(|err| PersistError::backend(format!("load previous secret version: {err}")))?;
                let mut prev_record: SecretVersionRecord = decode(&bytes)?;
                prev_record.status = SecretVersionStatus::Superseded;
                tx.execute(
                    "update local_secret_versions set record = ?3 where binding_id = ?1 and version = ?2",
                    params![request.binding_id.clone(), previous, encode(&prev_record)?],
                )
                .map_err(|err| PersistError::backend(format!("update previous secret version: {err}")))?;
            }
            let version = binding.latest_version.unwrap_or(0) + 1;
            let record = SecretVersionRecord {
                binding_id: request.binding_id.clone(),
                version,
                digest: request.digest,
                ciphertext: request.ciphertext,
                dek_wrapped: request.dek_wrapped,
                nonce: request.nonce,
                enc_alg: request.enc_alg,
                kek_id: request.kek_id,
                created_at_ns: request.created_at_ns,
                created_by: request.created_by,
                status: SecretVersionStatus::Active,
            };
            tx.execute(
                "insert into local_secret_versions (binding_id, version, record) values (?1, ?2, ?3)",
                params![record.binding_id.clone(), record.version, encode(&record)?],
            )
            .map_err(|err| PersistError::backend(format!("insert secret version: {err}")))?;
            binding.latest_version = Some(version);
            binding.updated_at_ns = record.created_at_ns;
            tx.execute(
                "update local_secret_bindings set record = ?2 where binding_id = ?1",
                params![record.binding_id.clone(), encode(&binding)?],
            )
            .map_err(|err| PersistError::backend(format!("update secret binding latest version: {err}")))?;
            Ok(record)
        })
    }

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        self.read(|conn| {
            conn.query_row(
                "select record from local_secret_versions where binding_id = ?1 and version = ?2",
                params![binding_id, version],
                |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                },
            )
            .optional()
            .map_err(|err| PersistError::backend(format!("load secret version: {err}")))
        })
    }

    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, PersistError> {
        self.ensure_local_universe(universe)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let mut stmt = conn
                .prepare("select record from local_secret_versions where binding_id = ?1 order by version asc limit ?2")
                .map_err(|err| PersistError::backend(format!("prepare list secret versions: {err}")))?;
            let rows = stmt
                .query_map(params![binding_id, limit], |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    decode(&bytes).map_err(super::util::to_sql_error)
                })
                .map_err(|err| PersistError::backend(format!("query list secret versions: {err}")))?;
            rows.map(|row| row.map_err(|err| PersistError::backend(format!("read secret version row: {err}"))))
                .collect()
        })
    }

    fn append_secret_audit(
        &self,
        universe: UniverseId,
        record: SecretAuditRecord,
    ) -> Result<(), PersistError> {
        self.ensure_local_universe(universe)?;
        self.write(|tx| {
            tx.execute(
                "insert into local_secret_audit (ts_ns, binding_id, version_key, record) values (?1, ?2, ?3, ?4)",
                params![
                    record.ts_ns,
                    record.binding_id.clone(),
                    record.version.unwrap_or(0),
                    encode(&record)?
                ],
            )
            .map_err(|err| PersistError::backend(format!("append secret audit: {err}")))?;
            Ok(())
        })
    }
}

fn map_sql_world_insert_conflict(
    world: aos_node::WorldId,
) -> impl FnOnce(rusqlite::Error) -> PersistError {
    move |err| {
        let message = err.to_string();
        if message.contains("local_worlds.world_id") {
            PersistConflict::WorldExists { world_id: world }.into()
        } else if message.contains("local_worlds.handle") {
            PersistError::backend(format!("world handle conflict: {message}"))
        } else {
            PersistError::backend(format!("insert world: {message}"))
        }
    }
}

fn map_sql_world_handle_conflict(
    universe: UniverseId,
    world: WorldId,
    handle: String,
) -> impl FnOnce(rusqlite::Error) -> PersistError {
    move |err| {
        let message = err.to_string();
        if message.contains("local_world_handles.handle") {
            PersistConflict::WorldHandleExists {
                universe_id: universe,
                handle: handle.clone(),
                world_id: world,
            }
            .into()
        } else {
            PersistError::backend(format!("set world handle: {message}"))
        }
    }
}
