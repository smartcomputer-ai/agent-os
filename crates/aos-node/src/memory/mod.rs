use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aos_cbor::Hash;
use uuid::Uuid;

use crate::fork_snapshot::rewrite_snapshot_for_fork_policy;
use crate::memory_cas::MemoryCasStore;
use crate::projection::{materialization_from_snapshot, state_blobs_from_snapshot};
use crate::segment::{
    decode_segment_entries, encode_segment_entries, segment_checksum,
    validate_segment_export_request,
};
use crate::{
    CasStore, CborPayload, CellStateProjectionRecord, CommandIngress, CommandRecord, CommandStore,
    CreateUniverseRequest, CreateWorldSeedRequest, DeliveredStatus, DispatchStatus,
    EffectDedupeRecord, EffectDispatchItem, EffectInFlightItem, ForkWorldRequest,
    HeadProjectionRecord, HostedCoordinationStore, HostedEffectQueueStore, HostedPortalStore,
    HostedTimerQueueStore, InboxItem, InboxSeq, JournalHeight, NodeCatalog, NodeWorldRuntimeInfo,
    PersistConflict, PersistCorruption, PersistError, PersistenceConfig, PortalDedupeRecord,
    PortalSendResult, PortalSendStatus, ProjectionStore, PutSecretVersionRequest,
    QueryProjectionDelta, QueryProjectionMaterialization, QueueSeq, ReadyHint, ReadyState,
    ReceiptIngress, SecretAuditRecord, SecretBindingRecord, SecretBindingSourceKind,
    SecretBindingStatus, SecretStore, SecretVersionRecord, SecretVersionStatus,
    SegmentExportRequest, SegmentExportResult, SegmentIndexRecord, ShardId, SnapshotCommitRequest,
    SnapshotCommitResult, SnapshotRecord, SnapshotSelector, TimerClaim, TimerDedupeRecord,
    TimerDueItem, UniverseAdminLifecycle, UniverseAdminStatus, UniverseCreateResult, UniverseId,
    UniverseMeta, UniverseRecord, UniverseStore, WorkerHeartbeat,
    WorkspaceRegistryProjectionRecord, WorldAdminLifecycle, WorldAdminStatus, WorldAdminStore,
    WorldCreateResult, WorldForkResult, WorldId, WorldIngressStore, WorldLease, WorldLineage,
    WorldMeta, WorldRecord, WorldRuntimeInfo, WorldSeed, WorldStore, can_upgrade_snapshot_record,
    default_universe_handle, default_world_handle, ensure_monotonic_snapshot_records,
    gc_bucket_for, maintenance_due, normalize_handle, sample_world_meta,
    validate_baseline_promotion_record, validate_create_world_seed_request,
    validate_fork_world_request, validate_query_projection_delta,
    validate_query_projection_materialization, validate_snapshot_commit_request,
    validate_snapshot_record,
};

mod admin;
mod catalog;
mod hosted;
mod state;
mod support;
#[cfg(test)]
mod tests;
mod world;

pub use self::state::MemoryWorldPersistence;
use self::state::{MemoryState, MemoryTimerInFlightItem, StoredCommandRecord, WorldState};

fn matches_universe_filter(filter: Option<&[UniverseId]>, universe: UniverseId) -> bool {
    filter
        .map(|universes| universes.contains(&universe))
        .unwrap_or(true)
}

impl MemoryWorldPersistence {
    fn snapshot_maintenance_config(&self) -> crate::SnapshotMaintenanceConfig {
        self.config.snapshot_maintenance
    }

    fn heartbeat_worker(&self, heartbeat: WorkerHeartbeat) -> Result<(), PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        guard.workers.insert(heartbeat.worker_id.clone(), heartbeat);
        Ok(())
    }

    fn list_active_workers(
        &self,
        now_ns: u64,
        limit: u32,
    ) -> Result<Vec<WorkerHeartbeat>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut workers: Vec<_> = guard
            .workers
            .values()
            .filter(|heartbeat| heartbeat.expires_at_ns >= now_ns)
            .cloned()
            .collect();
        workers.sort_by(|left, right| left.worker_id.cmp(&right.worker_id));
        workers.truncate(limit as usize);
        Ok(workers)
    }

    fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = guard.worlds.get(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        Ok(Self::world_runtime_info_from_state(
            world,
            world_state,
            now_ns,
        ))
    }

    fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        let handle = normalize_handle(handle)?;
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world = guard
            .world_handles
            .get(&universe)
            .and_then(|handles| handles.get(&handle))
            .ok_or_else(|| {
                PersistError::not_found(format!("world handle '{handle}' in universe {universe}"))
            })?;
        let world_state = guard.worlds.get(&(universe, *world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        if matches!(world_state.meta.admin.status, WorldAdminStatus::Deleted) {
            return Err(PersistError::not_found(format!(
                "world handle '{handle}' in universe {universe}"
            )));
        }
        Ok(Self::world_runtime_info_from_state(
            *world,
            world_state,
            now_ns,
        ))
    }

    fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut worlds: Vec<_> = guard
            .worlds
            .iter()
            .filter(|((world_universe, _), _)| *world_universe == universe)
            .filter(|((_, world_id), _)| after.is_none_or(|cursor| *world_id > cursor))
            .map(|((_, world_id), world_state)| {
                Self::world_runtime_info_from_state(*world_id, world_state, now_ns)
            })
            .collect();
        worlds.sort_by_key(|info| info.world_id);
        worlds.truncate(limit as usize);
        Ok(worlds)
    }

    fn list_ready_worlds(
        &self,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut worlds = Vec::new();
        for ((_, _, universe, world_id), _) in &guard.ready_hints {
            if worlds.len() >= limit as usize {
                break;
            }
            if !matches_universe_filter(universe_filter, *universe) {
                continue;
            }
            let Some(world_state) = guard.worlds.get(&(*universe, *world_id)) else {
                continue;
            };
            if !world_state.meta.admin.status.allows_new_leases() {
                continue;
            }
            worlds.push(NodeWorldRuntimeInfo {
                universe_id: *universe,
                info: Self::world_runtime_info_from_state(*world_id, world_state, now_ns),
            });
        }
        Ok(worlds)
    }

    fn head_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state.head_projection.clone())
        })
    }

    fn cell_state_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<Option<CellStateProjectionRecord>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state
                .cell_state_projections
                .get(workflow)
                .and_then(|cells| cells.get(key_hash))
                .cloned())
        })
    }

    fn list_cell_state_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        after_key_hash: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<Vec<CellStateProjectionRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_world(universe, world, |world_state| {
            let cells = world_state
                .cell_state_projections
                .get(workflow)
                .into_iter()
                .flat_map(|cells| cells.iter())
                .filter(|(key_hash, _)| {
                    after_key_hash
                        .as_ref()
                        .is_none_or(|after| key_hash.as_slice() > after.as_slice())
                })
                .map(|(_, record)| record.clone())
                .take(limit as usize)
                .collect();
            Ok(cells)
        })
    }

    fn workspace_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state.workspace_projections.get(workspace).cloned())
        })
    }

    fn list_workspace_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_workspace: Option<String>,
        limit: u32,
    ) -> Result<Vec<WorkspaceRegistryProjectionRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_world(universe, world, |world_state| {
            let workspaces = world_state
                .workspace_projections
                .iter()
                .filter(|(workspace, _)| {
                    after_workspace
                        .as_ref()
                        .is_none_or(|after| workspace.as_str() > after.as_str())
                })
                .map(|(_, record)| record.clone())
                .take(limit as usize)
                .collect();
            Ok(workspaces)
        })
    }

    fn bootstrap_query_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        validate_query_projection_materialization(&materialization)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = state.worlds.get_mut(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        Self::apply_query_projection_materialization(world_state, materialization);
        Ok(())
    }

    fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        self.with_world(universe, world, |world_state| {
            let allowed = match &item {
                InboxItem::Control(_) => world_state.meta.admin.status.accepts_command_ingress(),
                _ => world_state.meta.admin.status.accepts_direct_ingress(),
            };
            if !allowed {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: world_state.meta.admin.status,
                    action: "enqueue_ingress".into(),
                }
                .into());
            }
            Ok(())
        })?;
        self.inbox_enqueue(universe, world, item)
    }

    fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError> {
        self.with_world(universe, world, |world_state| {
            Ok(world_state
                .command_records
                .get(command_id)
                .map(|stored| stored.record.clone()))
        })
    }

    fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: crate::CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        let request_hash = aos_cbor::Hash::of_cbor(&(
            ingress.command.as_str(),
            ingress.actor.as_deref(),
            &ingress.payload,
        ))
        .map_err(|err| PersistError::backend(err.to_string()))?
        .to_hex();
        let initial_record = self.normalize_command_record(universe, initial_record)?;
        let result = self.with_world_mut(universe, world, |world_state| {
            let allowed = world_state.meta.admin.status.accepts_command_ingress();
            if !allowed {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: world_state.meta.admin.status,
                    action: "submit_command".into(),
                }
                .into());
            }
            if let Some(existing) = world_state.command_records.get(&ingress.command_id) {
                if existing.request_hash != request_hash {
                    return Err(PersistConflict::CommandRequestMismatch {
                        command_id: ingress.command_id.clone(),
                    }
                    .into());
                }
                return Ok(existing.record.clone());
            }

            let item = self.normalize_inbox_item(universe, InboxItem::Control(ingress.clone()))?;
            let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
            world_state.next_inbox_seq = world_state.next_inbox_seq.saturating_add(1);
            world_state.inbox_entries.insert(seq, item);
            world_state.notify_counter = world_state.notify_counter.saturating_add(1);
            world_state.command_records.insert(
                ingress.command_id.clone(),
                StoredCommandRecord {
                    record: initial_record.clone(),
                    request_hash,
                },
            );
            Ok(initial_record)
        });
        if result.is_ok() {
            let mut guard = self
                .state
                .lock()
                .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
            Self::sync_ready_state(&mut guard, universe, world, 0, self.config);
        }
        result
    }

    fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        let record = self.normalize_command_record(universe, record)?;
        self.with_world_mut(universe, world, |world_state| {
            let Some(existing) = world_state.command_records.get_mut(&record.command_id) else {
                return Err(PersistError::not_found(format!(
                    "command {}",
                    record.command_id
                )));
            };
            existing.record = record;
            Ok(())
        })
    }

    fn update_command_record_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &crate::WorldLease,
        now_ns: u64,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        let record = self.normalize_command_record(universe, record)?;
        self.with_world_mut(universe, world, |world_state| {
            let current_lease =
                world_state
                    .lease
                    .clone()
                    .ok_or_else(|| PersistConflict::LeaseMismatch {
                        expected_worker_id: lease.holder_worker_id.clone(),
                        expected_epoch: lease.epoch,
                        actual_worker_id: None,
                        actual_epoch: None,
                    })?;
            if current_lease.holder_worker_id != lease.holder_worker_id
                || current_lease.epoch != lease.epoch
                || current_lease.expires_at_ns <= now_ns
            {
                return Err(PersistConflict::LeaseMismatch {
                    expected_worker_id: lease.holder_worker_id.clone(),
                    expected_epoch: lease.epoch,
                    actual_worker_id: Some(current_lease.holder_worker_id),
                    actual_epoch: Some(current_lease.epoch),
                }
                .into());
            }
            let Some(existing) = world_state.command_records.get_mut(&record.command_id) else {
                return Err(PersistError::not_found(format!(
                    "command {}",
                    record.command_id
                )));
            };
            existing.record = record;
            Ok(())
        })
    }

    fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError> {
        self.with_world_mut(universe, world, |world_state| {
            if world_state.meta.admin.status.blocks_world_operations() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: world_state.meta.admin.status,
                    action: "set_world_placement_pin".into(),
                }
                .into());
            }
            world_state.meta.placement_pin = placement_pin;
            Ok(())
        })
    }

    fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let released_handle = {
            let world_state = guard.worlds.get_mut(&(universe, world)).ok_or_else(|| {
                PersistError::not_found(format!("world {world} in universe {universe}"))
            })?;
            world_state.meta.admin = admin.clone();
            matches!(admin.status, WorldAdminStatus::Deleted)
                .then(|| world_state.meta.handle.clone())
        };
        if let Some(handle) = released_handle {
            if let Some(handles) = guard.world_handles.get_mut(&universe) {
                handles.remove(&handle);
            }
        }
        Self::sync_ready_state(&mut guard, universe, world, 0, self.config);
        Ok(())
    }

    fn current_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<WorldLease>, PersistError> {
        self.with_world(universe, world, |world_state| Ok(world_state.lease.clone()))
    }

    fn acquire_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let lease = {
            let (current_lease, admin_status) = {
                let world_state = state.worlds.entry((universe, world)).or_default();
                (world_state.lease.clone(), world_state.meta.admin.status)
            };
            if !admin_status.allows_new_leases() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: admin_status,
                    action: "acquire_world_lease".into(),
                }
                .into());
            }
            let next_lease = if let Some(current) = &current_lease {
                let current_holder_live = state
                    .workers
                    .get(&current.holder_worker_id)
                    .is_some_and(|heartbeat| heartbeat.expires_at_ns >= now_ns);
                if current.expires_at_ns >= now_ns {
                    if current.holder_worker_id == worker_id {
                        current.clone()
                    } else if current_holder_live {
                        return Err(PersistConflict::LeaseHeld {
                            holder_worker_id: current.holder_worker_id.clone(),
                            epoch: current.epoch,
                            expires_at_ns: current.expires_at_ns,
                        }
                        .into());
                    } else {
                        let epoch = current.epoch + 1;
                        let lease = WorldLease {
                            holder_worker_id: worker_id.to_string(),
                            epoch,
                            expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                        };
                        lease
                    }
                } else {
                    let epoch = current.epoch + 1;
                    WorldLease {
                        holder_worker_id: worker_id.to_string(),
                        epoch,
                        expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                    }
                }
            } else {
                WorldLease {
                    holder_worker_id: worker_id.to_string(),
                    epoch: 1,
                    expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                }
            };
            state.worlds.entry((universe, world)).or_default().lease = Some(next_lease.clone());
            next_lease
        };
        state
            .lease_by_worker
            .retain(|(_, indexed_universe, indexed_world), _| {
                *indexed_universe != universe || *indexed_world != world
            });
        state
            .lease_by_worker
            .insert((worker_id.to_string(), universe, world), lease.clone());
        Ok(lease)
    }

    fn renew_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let renewed = {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
            let renewed = WorldLease {
                holder_worker_id: lease.holder_worker_id.clone(),
                epoch: lease.epoch,
                expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
            };
            world_state.lease = Some(renewed.clone());
            renewed
        };
        state.lease_by_worker.insert(
            (lease.holder_worker_id.clone(), universe, world),
            renewed.clone(),
        );
        Ok(renewed)
    }

    fn release_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
    ) -> Result<(), PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        {
            let world_state = state.worlds.entry((universe, world)).or_default();
            let Some(current) = &world_state.lease else {
                return Err(PersistConflict::LeaseMismatch {
                    expected_worker_id: lease.holder_worker_id.clone(),
                    expected_epoch: lease.epoch,
                    actual_worker_id: None,
                    actual_epoch: None,
                }
                .into());
            };
            if current.holder_worker_id != lease.holder_worker_id || current.epoch != lease.epoch {
                return Err(PersistConflict::LeaseMismatch {
                    expected_worker_id: lease.holder_worker_id.clone(),
                    expected_epoch: lease.epoch,
                    actual_worker_id: Some(current.holder_worker_id.clone()),
                    actual_epoch: Some(current.epoch),
                }
                .into());
            }
            world_state.lease = None;
        }
        state
            .lease_by_worker
            .remove(&(lease.holder_worker_id.clone(), universe, world));
        Ok(())
    }

    fn list_worker_worlds(
        &self,
        worker_id: &str,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut worlds = Vec::new();
        for ((indexed_worker_id, universe, world_id), lease) in &guard.lease_by_worker {
            if indexed_worker_id != worker_id || lease.expires_at_ns < now_ns {
                continue;
            }
            if !matches_universe_filter(universe_filter, *universe) {
                continue;
            }
            if worlds.len() >= limit as usize {
                break;
            }
            let Some(world_state) = guard.worlds.get(&(*universe, *world_id)) else {
                continue;
            };
            worlds.push(NodeWorldRuntimeInfo {
                universe_id: *universe,
                info: Self::world_runtime_info_from_state(*world_id, world_state, now_ns),
            });
        }
        worlds.sort_by_key(|entry| (entry.universe_id, entry.info.world_id));
        if worlds.len() > limit as usize {
            worlds.truncate(limit as usize);
        }
        Ok(worlds)
    }

    fn journal_append_batch_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.validate_journal_batch(entries)?;
        self.with_world_mut(universe, world, |world_state| {
            Self::assert_live_lease(world_state, lease, now_ns)?;
            self.journal_append_batch_inner(world_state, expected_head, entries)
        })
    }

    fn inbox_commit_cursor_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
            Self::inbox_commit_cursor_inner(world_state, old_cursor, new_cursor)
        }?;
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(())
    }

    fn drain_inbox_to_journal_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.validate_journal_batch(journal_entries)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let head = {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
            Self::inbox_commit_cursor_inner(world_state, old_cursor, new_cursor)?;
            self.journal_append_batch_inner(world_state, expected_head, journal_entries)
        }?;
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(head)
    }

    fn materialize_query_projections_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        validate_query_projection_materialization(&materialization)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = state.worlds.get_mut(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        Self::assert_live_lease(world_state, lease, now_ns)?;
        Self::apply_query_projection_materialization(world_state, materialization);
        Ok(())
    }

    fn apply_query_projection_delta_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        delta: QueryProjectionDelta,
    ) -> Result<(), PersistError> {
        validate_query_projection_delta(&delta)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let world_state = state.worlds.get_mut(&(universe, world)).ok_or_else(|| {
            PersistError::not_found(format!("world {world} in universe {universe}"))
        })?;
        Self::assert_live_lease(world_state, lease, now_ns)?;
        Self::apply_query_projection_delta(world_state, delta);
        Ok(())
    }

    fn snapshot_index_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        self.with_world_mut(universe, world, |world_state| {
            Self::assert_live_lease(world_state, lease, now_ns)?;
            if let Some(existing) = world_state.snapshots.get(&record.height) {
                if existing == &record {
                    return Ok(());
                }
                if !can_upgrade_snapshot_record(existing, &record) {
                    ensure_monotonic_snapshot_records(&world_state.snapshots, &record)?;
                }
            } else {
                ensure_monotonic_snapshot_records(&world_state.snapshots, &record)?;
            }
            world_state.snapshots.insert(record.height, record);
            Ok(())
        })
    }

    fn snapshot_commit_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        validate_snapshot_commit_request(&request)?;
        let snapshot_hash = self.cas_put_verified(universe, &request.snapshot_bytes)?;
        let expected_ref = snapshot_hash.to_hex();
        if request.record.snapshot_ref != expected_ref {
            return Err(PersistError::validation(format!(
                "snapshot_ref {} must equal CAS hash {}",
                request.record.snapshot_ref, expected_ref
            )));
        }
        self.validate_journal_batch(std::slice::from_ref(&request.snapshot_journal_entry))?;
        if let Some(baseline) = &request.baseline_journal_entry {
            self.validate_journal_batch(&[
                request.snapshot_journal_entry.clone(),
                baseline.clone(),
            ])?;
        }

        let result = self.with_world_mut(universe, world, |world_state| {
            Self::assert_live_lease(world_state, lease, now_ns)?;
            ensure_monotonic_snapshot_records(&world_state.snapshots, &request.record)?;
            if world_state.journal_head != request.expected_head {
                return Err(PersistConflict::HeadAdvanced {
                    expected: request.expected_head,
                    actual: world_state.journal_head,
                }
                .into());
            }
            if request.promote_baseline {
                validate_baseline_promotion_record(&request.record)?;
                if let Some(active) = &world_state.active_baseline {
                    if request.record.height < active.height {
                        return Err(PersistError::validation(format!(
                            "baseline cannot regress from {} to {}",
                            active.height, request.record.height
                        )));
                    }
                    if request.record.height == active.height && request.record != *active {
                        return Err(PersistConflict::BaselineMismatch {
                            height: request.record.height,
                        }
                        .into());
                    }
                }
            }

            world_state
                .snapshots
                .insert(request.record.height, request.record.clone());

            let first_height = world_state.journal_head;
            world_state.journal_entries.insert(
                world_state.journal_head,
                request.snapshot_journal_entry.clone(),
            );
            world_state.journal_head += 1;

            if request.promote_baseline {
                world_state.journal_entries.insert(
                    world_state.journal_head,
                    request
                        .baseline_journal_entry
                        .clone()
                        .expect("validated baseline journal entry"),
                );
                world_state.journal_head += 1;
                world_state.active_baseline = Some(request.record.clone());
                Self::update_meta_from_baseline(world_state, &request.record);
            }

            Ok(SnapshotCommitResult {
                snapshot_hash,
                first_height,
                next_head: world_state.journal_head,
                baseline_promoted: request.promote_baseline,
            })
        })?;
        let mut state = self.state.lock().unwrap();
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(result)
    }

    fn snapshot_promote_baseline_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_baseline_promotion_record(&record)?;
        self.with_world_mut(universe, world, |world_state| {
            Self::assert_live_lease(world_state, lease, now_ns)?;
            let indexed = world_state.snapshots.get(&record.height).ok_or_else(|| {
                PersistError::not_found(format!("snapshot at height {}", record.height))
            })?;
            if indexed != &record {
                return Err(PersistConflict::SnapshotMismatch {
                    height: record.height,
                }
                .into());
            }
            if let Some(active) = &world_state.active_baseline {
                if record.height < active.height {
                    return Err(PersistError::validation(format!(
                        "baseline cannot regress from {} to {}",
                        active.height, record.height
                    )));
                }
                if record.height == active.height && record != *active {
                    return Err(PersistConflict::BaselineMismatch {
                        height: record.height,
                    }
                    .into());
                }
            }
            Self::update_meta_from_baseline(world_state, &record);
            world_state.active_baseline = Some(record);
            Ok(())
        })?;
        let mut state = self.state.lock().unwrap();
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(())
    }

    fn segment_index_put_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        if record.segment.end < record.segment.start {
            return Err(PersistError::validation(format!(
                "segment end {} must be >= start {}",
                record.segment.end, record.segment.start
            )));
        }
        self.with_world_mut(universe, world, |world_state| {
            Self::assert_live_lease(world_state, lease, now_ns)?;
            if let Some(existing) = world_state.segments.get(&record.segment.end) {
                if existing == &record {
                    return Ok(());
                }
                return Err(PersistConflict::SegmentExists {
                    end_height: record.segment.end,
                }
                .into());
            }
            world_state.segments.insert(record.segment.end, record);
            Ok(())
        })
    }

    fn segment_export_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        validate_segment_export_request(&request)?;
        let mut state = self.state.lock().unwrap();
        let record = {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
            let baseline = world_state.active_baseline.clone().ok_or_else(|| {
                PersistError::validation("segment export requires active baseline")
            })?;
            let safe_exclusive_end = baseline.height.saturating_sub(request.hot_tail_margin);
            if request.segment.end >= safe_exclusive_end {
                return Err(PersistError::validation(format!(
                    "segment end {} must be strictly below active baseline {} with hot-tail margin {}",
                    request.segment.end, baseline.height, request.hot_tail_margin
                )));
            }
            if request.segment.end >= world_state.journal_head {
                return Err(PersistError::validation(format!(
                    "segment end {} must be below current journal head {}",
                    request.segment.end, world_state.journal_head
                )));
            }

            let existing_record = world_state.segments.get(&request.segment.end).cloned();
            if let Some(existing) = existing_record {
                if existing.segment != request.segment {
                    return Err(PersistConflict::SegmentExists {
                        end_height: request.segment.end,
                    }
                    .into());
                }
                let body_hash = Hash::from_hex_str(&existing.body_ref).map_err(|err| {
                    PersistError::validation(format!(
                        "invalid segment body_ref '{}': {err}",
                        existing.body_ref
                    ))
                })?;
                if !self.cas_has(universe, body_hash)? {
                    return Err(PersistCorruption::MissingSegmentBody {
                        segment: request.segment,
                        hash: body_hash,
                    }
                    .into());
                }
                existing
            } else {
                let mut entries = Vec::new();
                for height in request.segment.start..=request.segment.end {
                    let entry = world_state
                        .journal_entries
                        .get(&height)
                        .cloned()
                        .ok_or(PersistCorruption::MissingJournalEntry { height })?;
                    entries.push((height, entry));
                }
                let segment_bytes = encode_segment_entries(request.segment, &entries)?;
                let body_hash = self.cas_put_verified(universe, &segment_bytes)?;
                let record = SegmentIndexRecord {
                    segment: request.segment,
                    body_ref: body_hash.to_hex(),
                    checksum: segment_checksum(&segment_bytes),
                };
                world_state
                    .segments
                    .insert(request.segment.end, record.clone());
                record
            }
        };

        let world_state = state.worlds.get_mut(&(universe, world)).unwrap();
        let mut deleted_entries = 0u64;
        let mut chunk_start = request.segment.start;
        while chunk_start <= request.segment.end {
            let chunk_end =
                (chunk_start + request.delete_chunk_entries as u64 - 1).min(request.segment.end);
            for height in chunk_start..=chunk_end {
                if world_state.journal_entries.remove(&height).is_some() {
                    deleted_entries += 1;
                }
            }
            chunk_start = chunk_end.saturating_add(1);
        }

        let result = SegmentExportResult {
            record,
            exported_entries: request.segment.end - request.segment.start + 1,
            deleted_entries,
        };
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(result)
    }

    fn publish_effect_dispatches_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[EffectDispatchItem],
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
        }
        let mut published = 0u32;
        for item in items {
            if item.world_id != world {
                return Err(PersistError::validation(format!(
                    "effect dispatch world mismatch: expected {world}, got {}",
                    item.world_id
                )));
            }
            if state
                .effects_dedupe
                .entry(universe)
                .or_default()
                .contains_key(&item.intent_hash)
            {
                continue;
            }
            let item = self.normalize_effect_dispatch_item(universe, item.clone())?;
            let seq = self.next_effect_seq(&mut state, universe, item.shard);
            state
                .effects_pending
                .entry(universe)
                .or_default()
                .insert((item.shard, seq), item.clone());
            state.effects_dedupe.entry(universe).or_default().insert(
                item.intent_hash.clone(),
                EffectDedupeRecord {
                    status: DispatchStatus::Pending,
                    completed_at_ns: None,
                    gc_after_ns: None,
                },
            );
            let world_state = state.worlds.entry((universe, world)).or_default();
            world_state.pending_effects_count = world_state.pending_effects_count.saturating_add(1);
            published = published.saturating_add(1);
        }
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(published)
    }

    fn claim_pending_effects_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<(QueueSeq, EffectDispatchItem)>, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let candidates: Vec<_> = state
            .effects_pending
            .entry(universe)
            .or_default()
            .iter()
            .filter(|((_, _), item)| item.world_id == world)
            .take(limit as usize)
            .map(|((shard, seq), item)| (*shard, seq.clone(), item.clone()))
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut claimed = Vec::with_capacity(candidates.len());
        for (shard, seq, item) in candidates {
            state
                .effects_pending
                .entry(universe)
                .or_default()
                .remove(&(shard, seq.clone()));
            state.effects_inflight.entry(universe).or_default().insert(
                (shard, seq.clone()),
                EffectInFlightItem {
                    dispatch: item.clone(),
                    claim_until_ns: now_ns.saturating_add(claim_ttl_ns),
                    worker_id: Some(worker_id.to_string()),
                },
            );
            state.effects_dedupe.entry(universe).or_default().insert(
                item.intent_hash.clone(),
                EffectDedupeRecord {
                    status: DispatchStatus::InFlight,
                    completed_at_ns: None,
                    gc_after_ns: None,
                },
            );
            claimed.push((seq, item));
        }
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(claimed)
    }

    fn ack_effect_dispatch_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        shard: u16,
        seq: QueueSeq,
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let inflight = state.effects_inflight.entry(universe).or_default();
        let found_key = (shard, seq.clone());
        let item = inflight
            .remove(&found_key)
            .ok_or_else(|| PersistError::not_found(format!("effect inflight seq {seq}")))?;
        if item.worker_id.as_deref() != Some(worker_id) {
            return Err(PersistError::validation(format!(
                "effect inflight seq {seq} not owned by worker {worker_id}"
            )));
        }
        let normalized_receipt =
            self.normalize_inbox_item(universe, InboxItem::Receipt(receipt))?;
        let receipt = match normalized_receipt {
            InboxItem::Receipt(receipt) => receipt,
            _ => unreachable!("normalized receipt ingress remains receipt"),
        };
        let world_state = state.worlds.entry((universe, world)).or_default();
        let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
        world_state.next_inbox_seq = world_state.next_inbox_seq.saturating_add(1);
        world_state
            .inbox_entries
            .insert(seq, InboxItem::Receipt(receipt.clone()));
        world_state.notify_counter = world_state.notify_counter.saturating_add(1);
        state.effects_dedupe.entry(universe).or_default().insert(
            item.dispatch.intent_hash.clone(),
            self.effect_terminal_record(DispatchStatus::Complete, now_ns),
        );
        let world_state = state.worlds.entry((universe, world)).or_default();
        world_state.pending_effects_count = world_state.pending_effects_count.saturating_sub(1);
        let gc_bucket = gc_bucket_for(
            now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns),
            self.config.dedupe_gc.bucket_width_ns,
        );
        state
            .effects_dedupe_gc
            .entry(universe)
            .or_default()
            .insert((gc_bucket, item.dispatch.intent_hash), ());
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(())
    }

    fn retain_effect_dispatches_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        valid_intents: &std::collections::HashSet<[u8; 32]>,
        now_ns: u64,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut dropped = 0u32;
        let mut dropped_hashes = Vec::new();

        let pending_to_remove: Vec<_> = state
            .effects_pending
            .get(&universe)
            .into_iter()
            .flat_map(|pending| pending.iter())
            .filter_map(|(key, item)| {
                if item.world_id != world || item.intent_hash.len() != 32 {
                    return None;
                }
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&item.intent_hash);
                (!valid_intents.contains(&hash)).then_some((key.clone(), item.intent_hash.clone()))
            })
            .collect();
        for (key, hash) in pending_to_remove {
            if state
                .effects_pending
                .entry(universe)
                .or_default()
                .remove(&key)
                .is_some()
            {
                dropped = dropped.saturating_add(1);
                dropped_hashes.push(hash);
            }
        }

        let inflight_to_remove: Vec<_> = state
            .effects_inflight
            .get(&universe)
            .into_iter()
            .flat_map(|inflight| inflight.iter())
            .filter_map(|(key, item)| {
                if item.dispatch.world_id != world || item.dispatch.intent_hash.len() != 32 {
                    return None;
                }
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&item.dispatch.intent_hash);
                (!valid_intents.contains(&hash))
                    .then_some((key.clone(), item.dispatch.intent_hash.clone()))
            })
            .collect();
        for (key, hash) in inflight_to_remove {
            if state
                .effects_inflight
                .entry(universe)
                .or_default()
                .remove(&key)
                .is_some()
            {
                dropped = dropped.saturating_add(1);
                dropped_hashes.push(hash);
            }
        }

        if dropped > 0 {
            let world_state = state.worlds.entry((universe, world)).or_default();
            world_state.pending_effects_count = world_state
                .pending_effects_count
                .saturating_sub(dropped as u64);
            let gc_bucket = gc_bucket_for(
                now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns),
                self.config.dedupe_gc.bucket_width_ns,
            );
            for hash in dropped_hashes {
                state.effects_dedupe.entry(universe).or_default().insert(
                    hash.clone(),
                    self.effect_terminal_record(DispatchStatus::Failed, now_ns),
                );
                state
                    .effects_dedupe_gc
                    .entry(universe)
                    .or_default()
                    .insert((gc_bucket, hash), ());
            }
            Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        }

        Ok(dropped)
    }

    fn requeue_expired_effect_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let expired: Vec<_> = state
            .effects_inflight
            .entry(universe)
            .or_default()
            .iter()
            .filter(|(_, item)| item.claim_until_ns <= now_ns)
            .take(limit as usize)
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect();
        let mut requeued = 0u32;
        for (key, item) in expired {
            state
                .effects_inflight
                .entry(universe)
                .or_default()
                .remove(&key);
            state
                .effects_pending
                .entry(universe)
                .or_default()
                .insert(key.clone(), item.dispatch.clone());
            state.effects_dedupe.entry(universe).or_default().insert(
                item.dispatch.intent_hash.clone(),
                EffectDedupeRecord {
                    status: DispatchStatus::Pending,
                    completed_at_ns: None,
                    gc_after_ns: None,
                },
            );
            Self::sync_ready_state(
                &mut state,
                universe,
                item.dispatch.world_id,
                now_ns,
                self.config,
            );
            requeued = requeued.saturating_add(1);
        }
        Ok(requeued)
    }

    fn publish_due_timers_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[TimerDueItem],
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        {
            let world_state = state.worlds.entry((universe, world)).or_default();
            Self::assert_live_lease(world_state, lease, now_ns)?;
        }
        let mut published = 0u32;
        for item in items {
            if item.world_id != world {
                return Err(PersistError::validation(format!(
                    "timer due world mismatch: expected {world}, got {}",
                    item.world_id
                )));
            }
            if state
                .timers_dedupe
                .entry(universe)
                .or_default()
                .contains_key(&item.intent_hash)
            {
                continue;
            }
            state.timers_due.entry(universe).or_default().insert(
                (
                    item.shard,
                    item.time_bucket,
                    item.deliver_at_ns,
                    item.intent_hash.clone(),
                ),
                item.clone(),
            );
            state.timers_dedupe.entry(universe).or_default().insert(
                item.intent_hash.clone(),
                TimerDedupeRecord {
                    status: DeliveredStatus::Pending,
                    completed_at_ns: None,
                    gc_after_ns: None,
                },
            );
            let world_state = state.worlds.entry((universe, world)).or_default();
            world_state.next_timer_due_at_ns = match world_state.next_timer_due_at_ns {
                Some(existing) => Some(existing.min(item.deliver_at_ns)),
                None => Some(item.deliver_at_ns),
            };
            published = published.saturating_add(1);
        }
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(published)
    }

    fn claim_due_timers_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<TimerDueItem>, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let candidates: Vec<_> = state
            .timers_due
            .entry(universe)
            .or_default()
            .iter()
            .filter(|(_, item)| item.world_id == world && item.deliver_at_ns <= now_ns)
            .take(limit as usize)
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut claimed = Vec::with_capacity(candidates.len());
        for (key, item) in candidates {
            state.timers_due.entry(universe).or_default().remove(&key);
            state.timers_inflight.entry(universe).or_default().insert(
                item.intent_hash.clone(),
                MemoryTimerInFlightItem {
                    due: item.clone(),
                    claim: TimerClaim {
                        intent_hash: item.intent_hash.clone(),
                        claim_until_ns: now_ns.saturating_add(claim_ttl_ns),
                        worker_id: Some(worker_id.to_string()),
                    },
                },
            );
            claimed.push(item);
        }
        let next_due = Self::recompute_next_timer_due(world, &state, universe);
        state
            .worlds
            .entry((universe, world))
            .or_default()
            .next_timer_due_at_ns = next_due;
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(claimed)
    }

    fn ack_timer_delivery_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        intent_hash: &[u8],
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let inflight = state.timers_inflight.entry(universe).or_default();
        let item = inflight
            .remove(intent_hash)
            .ok_or_else(|| PersistError::not_found("timer inflight item"))?;
        if item.due.world_id != world {
            return Err(PersistError::validation(format!(
                "timer inflight world mismatch: expected {world}, got {}",
                item.due.world_id
            )));
        }
        if item.claim.worker_id.as_deref() != Some(worker_id) {
            return Err(PersistError::validation(format!(
                "timer inflight {} not owned by worker {worker_id}",
                hex::encode(intent_hash)
            )));
        }
        let normalized_receipt =
            self.normalize_inbox_item(universe, InboxItem::Receipt(receipt))?;
        let receipt = match normalized_receipt {
            InboxItem::Receipt(receipt) => receipt,
            _ => unreachable!("normalized receipt ingress remains receipt"),
        };
        {
            let world_state = state.worlds.entry((universe, world)).or_default();
            let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
            world_state.next_inbox_seq = world_state.next_inbox_seq.saturating_add(1);
            world_state
                .inbox_entries
                .insert(seq, InboxItem::Receipt(receipt));
            world_state.notify_counter = world_state.notify_counter.saturating_add(1);
        }
        let next_due = Self::recompute_next_timer_due(world, &state, universe);
        state
            .worlds
            .entry((universe, world))
            .or_default()
            .next_timer_due_at_ns = next_due;
        state.timers_dedupe.entry(universe).or_default().insert(
            intent_hash.to_vec(),
            self.timer_terminal_record(DeliveredStatus::Delivered, now_ns),
        );
        let gc_bucket = gc_bucket_for(
            now_ns.saturating_add(self.config.dedupe_gc.timer_retention_ns),
            self.config.dedupe_gc.bucket_width_ns,
        );
        state
            .timers_dedupe_gc
            .entry(universe)
            .or_default()
            .insert((gc_bucket, intent_hash.to_vec()), ());
        Self::sync_ready_state(&mut state, universe, world, now_ns, self.config);
        Ok(())
    }

    fn outstanding_intent_hashes_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<Vec<[u8; 32]>, PersistError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut hashes = Vec::new();
        if let Some(pending) = state.effects_pending.get(&universe) {
            for item in pending.values() {
                if item.world_id == world && item.intent_hash.len() == 32 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&item.intent_hash);
                    hashes.push(hash);
                }
            }
        }
        if let Some(inflight) = state.effects_inflight.get(&universe) {
            for item in inflight.values() {
                if item.dispatch.world_id == world && item.dispatch.intent_hash.len() == 32 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&item.dispatch.intent_hash);
                    hashes.push(hash);
                }
            }
        }
        if let Some(due) = state.timers_due.get(&universe) {
            for item in due.values() {
                if item.world_id == world
                    && item.deliver_at_ns <= now_ns
                    && item.intent_hash.len() == 32
                {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&item.intent_hash);
                    hashes.push(hash);
                }
            }
        }
        if let Some(inflight) = state.timers_inflight.get(&universe) {
            for item in inflight.values() {
                if item.due.world_id == world && item.due.intent_hash.len() == 32 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&item.due.intent_hash);
                    hashes.push(hash);
                }
            }
        }
        hashes.sort_unstable();
        hashes.dedup();
        Ok(hashes)
    }

    fn requeue_expired_timer_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let expired: Vec<_> = state
            .timers_inflight
            .entry(universe)
            .or_default()
            .iter()
            .filter(|(_, item)| item.claim.claim_until_ns <= now_ns)
            .take(limit as usize)
            .map(|(intent_hash, item)| (intent_hash.clone(), item.clone()))
            .collect();
        let mut requeued = 0u32;
        for (intent_hash, item) in expired {
            state
                .timers_inflight
                .entry(universe)
                .or_default()
                .remove(&intent_hash);
            state.timers_due.entry(universe).or_default().insert(
                (
                    item.due.shard,
                    item.due.time_bucket,
                    item.due.deliver_at_ns,
                    item.due.intent_hash.clone(),
                ),
                item.due.clone(),
            );
            let world_state = state
                .worlds
                .entry((universe, item.due.world_id))
                .or_default();
            world_state.next_timer_due_at_ns = match world_state.next_timer_due_at_ns {
                Some(existing) => Some(existing.min(item.due.deliver_at_ns)),
                None => Some(item.due.deliver_at_ns),
            };
            Self::sync_ready_state(&mut state, universe, item.due.world_id, now_ns, self.config);
            requeued = requeued.saturating_add(1);
        }
        Ok(requeued)
    }

    fn portal_send(
        &self,
        universe: UniverseId,
        dest_world: WorldId,
        now_ns: u64,
        message_id: &[u8],
        item: InboxItem,
    ) -> Result<PortalSendResult, PersistError> {
        let item = self.normalize_inbox_item(universe, item)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        if let Some(existing) = guard
            .portal_dedupe
            .get(&(universe, dest_world))
            .and_then(|dedupe| dedupe.get(message_id))
        {
            return Ok(PortalSendResult {
                status: PortalSendStatus::AlreadyEnqueued,
                enqueued_seq: existing.enqueued_seq.clone(),
            });
        }
        let Some(world_state) = guard.worlds.get_mut(&(universe, dest_world)) else {
            return Err(PersistError::not_found(format!(
                "world {dest_world} in universe {universe}"
            )));
        };
        let seq = InboxSeq::from_u64(world_state.next_inbox_seq);
        world_state.next_inbox_seq = world_state.next_inbox_seq.saturating_add(1);
        world_state.inbox_entries.insert(seq.clone(), item);
        world_state.notify_counter = world_state.notify_counter.saturating_add(1);
        Self::sync_ready_state(&mut guard, universe, dest_world, 0, self.config);
        guard
            .portal_dedupe
            .entry((universe, dest_world))
            .or_default()
            .insert(
                message_id.to_vec(),
                self.portal_terminal_record(Some(seq.clone()), now_ns),
            );
        let gc_bucket = gc_bucket_for(
            now_ns.saturating_add(self.config.dedupe_gc.portal_retention_ns),
            self.config.dedupe_gc.bucket_width_ns,
        );
        guard
            .portal_dedupe_gc
            .entry(universe)
            .or_default()
            .insert((gc_bucket, dest_world, message_id.to_vec()), ());
        Ok(PortalSendResult {
            status: PortalSendStatus::Enqueued,
            enqueued_seq: Some(seq),
        })
    }

    fn sweep_effect_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<_> = state
            .effects_dedupe_gc
            .entry(universe)
            .or_default()
            .keys()
            .filter(|(bucket, _)| *bucket <= max_bucket)
            .take(limit as usize)
            .cloned()
            .collect();
        let mut swept = 0u32;
        for (bucket, intent_hash) in candidates {
            let should_delete = state
                .effects_dedupe
                .entry(universe)
                .or_default()
                .get(&intent_hash)
                .and_then(|record| record.gc_after_ns)
                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns);
            if should_delete {
                state
                    .effects_dedupe
                    .entry(universe)
                    .or_default()
                    .remove(&intent_hash);
                swept = swept.saturating_add(1);
            }
            state
                .effects_dedupe_gc
                .entry(universe)
                .or_default()
                .remove(&(bucket, intent_hash));
        }
        Ok(swept)
    }

    fn sweep_timer_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<_> = state
            .timers_dedupe_gc
            .entry(universe)
            .or_default()
            .keys()
            .filter(|(bucket, _)| *bucket <= max_bucket)
            .take(limit as usize)
            .cloned()
            .collect();
        let mut swept = 0u32;
        for (bucket, intent_hash) in candidates {
            let should_delete = state
                .timers_dedupe
                .entry(universe)
                .or_default()
                .get(&intent_hash)
                .and_then(|record| record.gc_after_ns)
                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns);
            if should_delete {
                state
                    .timers_dedupe
                    .entry(universe)
                    .or_default()
                    .remove(&intent_hash);
                swept = swept.saturating_add(1);
            }
            state
                .timers_dedupe_gc
                .entry(universe)
                .or_default()
                .remove(&(bucket, intent_hash));
        }
        Ok(swept)
    }

    fn sweep_portal_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<_> = state
            .portal_dedupe_gc
            .entry(universe)
            .or_default()
            .keys()
            .filter(|(bucket, _, _)| *bucket <= max_bucket)
            .take(limit as usize)
            .cloned()
            .collect();
        let mut swept = 0u32;
        for (bucket, world, message_id) in candidates {
            let should_delete = state
                .portal_dedupe
                .entry((universe, world))
                .or_default()
                .get(&message_id)
                .and_then(|record| record.gc_after_ns)
                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns);
            if should_delete {
                state
                    .portal_dedupe
                    .entry((universe, world))
                    .or_default()
                    .remove(&message_id);
                swept = swept.saturating_add(1);
            }
            state
                .portal_dedupe_gc
                .entry(universe)
                .or_default()
                .remove(&(bucket, world, message_id));
        }
        Ok(swept)
    }
}
