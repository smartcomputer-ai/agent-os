//! Legacy pre-cutover world lifecycle helpers.
//!
//! This file is intentionally not on the compiled hosted worker path.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use aos_air_types::ModuleKind;
use aos_air_types::builtins;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::{KernelConfig, LoadedManifest, ManifestLoader};
use aos_node::{
    CheckpointBackend, CreateWorldRequest, ForkWorldRequest, ImportedSeedSource, BackendError,
    RegisteredWorldSummary, SeedKind, SnapshotRecord, SnapshotSelector, UniverseId, WorldId,
    open_plane_world_from_checkpoint, open_plane_world_from_frames, partition_for_world,
    rewrite_snapshot_for_fork_policy,
};
use aos_runtime::WorldHost;
use uuid::Uuid;

use super::types::{
    ActiveWorld, CreateWorldAccepted, HostedWorkerCore, HostedWorldMetadata,
    HostedWorldSummary, ProjectionContinuity, RegisteredWorld, WorkerError,
};
use super::util::{parse_hash_ref, snapshot_record_from_checkpoint, snapshot_record_from_frames};

impl HostedWorkerCore {
    pub(super) fn log_world_opened(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        trigger: &'static str,
        total_open_ms: u128,
    ) {
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        let world_epoch = self
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.world_epoch)
            .unwrap_or(1);
        tracing::info!(
            universe_id = %universe_id,
            world_id = %world_id,
            partition,
            world_epoch,
            trigger,
            total_open_ms,
            "aos-node-hosted world opened"
        );
    }

    pub(super) fn kernel_config_for_world(
        &self,
        universe_id: UniverseId,
    ) -> Result<KernelConfig, WorkerError> {
        let world_config = self.infra.world_config_for_domain(universe_id)?;
        Ok(world_config.apply_kernel_defaults(KernelConfig {
            universe_id: universe_id.as_uuid(),
            secret_resolver: Some(Arc::new(
                self.infra.vault.resolver_for_universe(universe_id),
            )),
            ..KernelConfig::default()
        }))
    }

    pub(super) fn with_world_host_for_read<T>(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        f: impl FnOnce(&WorldHost<crate::blobstore::HostedCas>) -> Result<T, WorkerError>,
    ) -> Result<T, WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        if let Some(world) = self.state.active_worlds.get(&world_id) {
            return f(&world.host);
        }
        let reopened = self.reopen_registered_world_host(universe_id, world_id)?;
        f(&reopened)
    }

    pub(super) fn submit_create_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        request: CreateWorldRequest,
    ) -> Result<CreateWorldAccepted, WorkerError> {
        self.seed_world_direct(universe_id, world_id, request, false)?;
        let effective_partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        Ok(CreateWorldAccepted {
            submission_id: format!("seed-{world_id}"),
            submission_offset: 0,
            world_id,
            effective_partition,
        })
    }

    pub(super) fn bootstrap_recovery(&mut self) -> Result<(), WorkerError> {
        Ok(())
    }

    pub(super) fn require_default_universe(
        &self,
        universe_id: UniverseId,
    ) -> Result<(), WorkerError> {
        let _ = universe_id;
        Ok(())
    }

    pub(super) fn create_fork_seed_request(
        &mut self,
        universe_id: UniverseId,
        new_world_id: WorldId,
        request: &ForkWorldRequest,
    ) -> Result<CreateWorldRequest, WorkerError> {
        let selected =
            self.select_source_snapshot(universe_id, request.src_world_id, &request.src_snapshot)?;
        self.hydrate_snapshot_into_local_cas(selected.universe_id, &selected.snapshot_ref)?;
        let selected_bytes = self
            .infra
            .store_for_domain(selected.universe_id)?
            .get(parse_hash_ref(&selected.snapshot_ref)?)?;
        let rewritten =
            rewrite_snapshot_for_fork_policy(&selected_bytes, &request.pending_effect_policy)?;
        let snapshot_ref = if let Some(bytes) = rewritten {
            self.infra
                .store_for_domain(selected.universe_id)?
                .put_verified(&bytes)?
                .to_hex()
        } else {
            selected.snapshot_ref.clone()
        };
        Ok(CreateWorldRequest {
            world_id: Some(new_world_id),
            universe_id: selected.universe_id,
            created_at_ns: request.forked_at_ns,
            source: aos_node::CreateWorldSource::Seed {
                seed: aos_node::WorldSeed {
                    baseline: SnapshotRecord {
                        snapshot_ref,
                        height: selected.height,
                        universe_id: selected.universe_id,
                        logical_time_ns: selected.logical_time_ns,
                        receipt_horizon_height: selected.receipt_horizon_height,
                        manifest_hash: selected.manifest_hash.clone(),
                    },
                    seed_kind: SeedKind::Import,
                    imported_from: Some(ImportedSeedSource {
                        source: "fork".into(),
                        external_world_id: Some(request.src_world_id.to_string()),
                        external_snapshot_ref: Some(selected.snapshot_ref),
                    }),
                },
            },
        })
    }

    pub(super) fn sync_active_worlds(
        &mut self,
        assigned_partitions: &[u32],
        newly_assigned_partitions: &[u32],
    ) -> Result<(), WorkerError> {
        let assigned = assigned_partitions.iter().copied().collect::<BTreeSet<_>>();
        for partition in newly_assigned_partitions {
            self.infra.kafka.recover_partition_from_broker(*partition)?;
        }
        self.state.active_worlds.retain(|&world_id, _| {
            assigned.contains(&partition_for_world(
                world_id,
                self.infra.kafka.partition_count(),
            ))
        });
        Ok(())
    }

    pub(super) fn world_disabled_reason(&self, world_id: WorldId) -> Option<&str> {
        self.state
            .registered_worlds
            .get(&world_id)
            .and_then(|world| world.disabled_reason.as_deref())
    }

    pub(super) fn disable_world(&mut self, world_id: WorldId, reason: impl Into<String>) {
        let reason = reason.into();
        self.state.active_worlds.remove(&world_id);
        if let Some(world) = self.state.registered_worlds.get_mut(&world_id) {
            world.disabled_reason = Some(reason.clone());
            let warning = format!("disabled: {reason}");
            if !world.metadata.warnings.iter().any(|item| item == &warning) {
                world.metadata.warnings.push(warning);
            }
        }
    }

    pub(super) fn activate_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        if self.state.active_worlds.contains_key(&world_id) {
            return Ok(());
        }
        let (last_checkpointed_head, last_checkpointed_at_ns) = self
            .checkpoint_watermark_for_world(universe_id, world_id)?
            .unwrap_or((0, 0));
        let open_started = Instant::now();
        let host = self.reopen_registered_world_host(universe_id, world_id)?;
        let total_open_ms = open_started.elapsed().as_millis();
        let active_baseline =
            self.select_source_snapshot(universe_id, world_id, &SnapshotSelector::ActiveBaseline)?;
        let projection_bootstrapped = self.prepare_projection_continuity_for_reopen(
            world_id,
            host.heights().head,
            &active_baseline,
        )?;
        let universe_id = self
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        self.state.active_worlds.insert(
            world_id,
            ActiveWorld {
                last_checkpointed_head,
                last_checkpointed_at_ns,
                host,
                accepted_submission_ids: BTreeSet::new(),
                projection_bootstrapped,
            },
        );
        self.log_world_opened(universe_id, world_id, "assignment", total_open_ms);
        Ok(())
    }

    pub(super) fn ensure_registered_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        let _ = universe_id;
        if self.state.registered_worlds.contains_key(&world_id) {
            return Ok(());
        }
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        self.infra.kafka.recover_partition_from_broker(partition)?;
        let universe_id = self.universe_id_from_journal(universe_id, world_id).ok_or(
            WorkerError::UnknownWorld {
                universe_id,
                world_id,
            },
        )?;
        let manifest_hash = self
            .manifest_hash_from_journal(universe_id, world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let world_epoch = self
            .world_epoch_from_journal(universe_id, world_id)
            .unwrap_or_else(|| {
                self.world_epoch_from_checkpoint(universe_id, world_id)
                    .unwrap_or(1)
            });
        self.register_world_from_manifest_hash(universe_id, world_id, &manifest_hash, world_epoch)
    }

    pub(super) fn select_source_snapshot(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        selector: &SnapshotSelector,
    ) -> Result<SnapshotRecord, WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        self.infra.kafka.recover_partition_from_broker(partition)?;
        let universe_id = self
            .state
            .registered_worlds
            .get(&world_id)
            .map(|world| world.universe_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        let checkpoint_snapshot = self
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .latest_checkpoint(&journal_topic, partition)
            .and_then(|checkpoint| {
                checkpoint
                    .worlds
                    .iter()
                    .find(|item| item.universe_id == universe_id && item.world_id == world_id)
                    .map(|item| snapshot_record_from_checkpoint(&item.baseline))
            });
        let frames = self.infra.kafka.world_frames(world_id);
        match selector {
            SnapshotSelector::ActiveBaseline => snapshot_record_from_frames(frames, |_| true)
                .or(checkpoint_snapshot)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                }),
            SnapshotSelector::ByHeight { height } => checkpoint_snapshot
                .filter(|snapshot| snapshot.height == *height)
                .or_else(|| {
                    snapshot_record_from_frames(frames, |snapshot| snapshot.height == *height)
                })
                .ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::not_found(format!(
                        "snapshot at height {height} not found for world {world_id}"
                    )))
                }),
            SnapshotSelector::ByRef { snapshot_ref } => checkpoint_snapshot
                .filter(|snapshot| snapshot.snapshot_ref == *snapshot_ref)
                .or_else(|| {
                    snapshot_record_from_frames(frames, |snapshot| {
                        snapshot.snapshot_ref == *snapshot_ref
                    })
                })
                .ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::not_found(format!(
                        "snapshot ref {snapshot_ref} not found for world {world_id}"
                    )))
                }),
        }
    }

    pub(super) fn universe_id_from_journal(
        &self,
        _universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<UniverseId> {
        let frames = self.infra.kafka.world_frames(world_id);
        frames
            .iter()
            .flat_map(|frame| frame.records.iter())
            .find_map(|record| {
                let aos_kernel::journal::JournalRecord::Snapshot(snapshot) = record else {
                    return None;
                };
                Some(snapshot.universe_id.into())
            })
    }

    pub(super) fn manifest_hash_from_journal(
        &self,
        _universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<String> {
        let frames = self.infra.kafka.world_frames(world_id);
        let last_snapshot = frames
            .iter()
            .flat_map(|frame| frame.records.iter())
            .filter_map(|record| match record {
                aos_kernel::journal::JournalRecord::Snapshot(snapshot) => {
                    snapshot.manifest_hash.clone()
                }
                _ => None,
            })
            .next_back();
        last_snapshot.or_else(|| {
            frames.first().and_then(|frame| {
                frame.records.iter().find_map(|record| match record {
                    aos_kernel::journal::JournalRecord::Snapshot(snapshot) => {
                        snapshot.manifest_hash.clone()
                    }
                    _ => None,
                })
            })
        })
    }

    pub(super) fn world_epoch_from_journal(
        &self,
        _universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<u64> {
        self.infra
            .kafka
            .world_frames(world_id)
            .last()
            .map(|frame| frame.world_epoch)
    }

    pub(super) fn world_epoch_from_checkpoint(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<u64> {
        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        let partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        self.infra
            .blob_meta_for_domain_mut(universe_id)
            .ok()?
            .latest_checkpoint(&journal_topic, partition)
            .and_then(|checkpoint| {
                checkpoint
                    .worlds
                    .iter()
                    .find(|item| item.universe_id == universe_id && item.world_id == world_id)
                    .map(|item| item.world_epoch)
            })
    }

    pub(super) fn register_world_from_manifest_hash(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        manifest_hash: &str,
        world_epoch: u64,
    ) -> Result<(), WorkerError> {
        let loaded = self.load_manifest_into_local_cas(universe_id, manifest_hash)?;
        let workflow_modules = loaded
            .modules
            .values()
            .filter(|module| matches!(module.module_kind, ModuleKind::Workflow))
            .map(|module| module.name.to_string())
            .collect::<Vec<_>>();
        let world_store = self.infra.store_for_domain(universe_id)?;
        self.state.registered_worlds.insert(
            world_id,
            RegisteredWorld {
                universe_id,
                store: world_store,
                loaded,
                manifest_hash: manifest_hash.to_owned(),
                world_epoch,
                projection_token: Uuid::new_v4().to_string(),
                projection_continuity: None,
                disabled_reason: None,
                metadata: HostedWorldMetadata {
                    workflow_modules,
                    warnings: Vec::new(),
                },
            },
        );
        Ok(())
    }

    pub(super) fn world_runtime_info(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<aos_node::WorldRuntimeInfo, WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        let summary =
            self.world_summary(universe_id, world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
        let baseline =
            self.select_source_snapshot(universe_id, world_id, &SnapshotSelector::ActiveBaseline)?;
        Ok(aos_node::WorldRuntimeInfo {
            world_id,
            universe_id: summary.universe_id,
            created_at_ns: 0,
            manifest_hash: Some(summary.manifest_hash),
            active_baseline_height: Some(baseline.height),
            notify_counter: 0,
            has_pending_inbox: false,
            has_pending_effects: false,
            next_timer_due_at_ns: None,
            has_pending_maintenance: false,
        })
    }

    pub(super) fn world_summary_response(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<aos_node::api::WorldSummaryResponse, WorkerError> {
        Ok(aos_node::api::WorldSummaryResponse {
            runtime: self.world_runtime_info(universe_id, world_id)?,
            active_baseline: self.select_source_snapshot(
                universe_id,
                world_id,
                &SnapshotSelector::ActiveBaseline,
            )?,
        })
    }

    pub(super) fn load_manifest_into_local_cas(
        &mut self,
        universe_id: UniverseId,
        manifest_hash: &str,
    ) -> Result<LoadedManifest, WorkerError> {
        let manifest = parse_hash_ref(manifest_hash)?;
        let store = self.infra.store_for_domain(universe_id)?;
        let manifest_bytes = store.get(manifest).map_err(WorkerError::Persist)?;
        let manifest_doc: aos_air_types::Manifest = serde_cbor::from_slice(&manifest_bytes)?;
        for named in manifest_doc
            .schemas
            .iter()
            .chain(manifest_doc.modules.iter())
            .chain(manifest_doc.effects.iter())
            .chain(manifest_doc.caps.iter())
            .chain(manifest_doc.policies.iter())
        {
            if is_builtin_manifest_ref(named.name.as_str()) {
                continue;
            }
            let hash = parse_hash_ref(named.hash.as_str())?;
            let _ = store.get(hash).map_err(WorkerError::Persist)?;
        }
        for secret in &manifest_doc.secrets {
            let aos_air_types::SecretEntry::Ref(named) = secret else {
                continue;
            };
            let hash = parse_hash_ref(named.hash.as_str())?;
            let _ = store.get(hash).map_err(WorkerError::Persist)?;
        }
        let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest)?;
        for module in loaded.modules.values() {
            if is_builtin_module(module.name.as_str()) || is_zero_hash(module.wasm_hash.as_str()) {
                continue;
            }
            let wasm_hash =
                aos_cbor::Hash::from_hex_str(module.wasm_hash.as_str()).map_err(|_| {
                    WorkerError::LogFirst(BackendError::InvalidHashRef(module.wasm_hash.to_string()))
                })?;
            let _ = store.get(wasm_hash).map_err(WorkerError::Persist)?;
        }
        Ok(loaded)
    }

    pub(super) fn hydrate_snapshot_into_local_cas(
        &mut self,
        universe_id: UniverseId,
        snapshot_ref: &str,
    ) -> Result<(), WorkerError> {
        let _ = self
            .infra
            .store_for_domain(universe_id)?
            .get(parse_hash_ref(snapshot_ref)?)
            .map_err(WorkerError::Persist)?;
        Ok(())
    }

    pub(super) fn world_summary(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<HostedWorldSummary> {
        let runtime_summary = self.registered_world(universe_id, world_id)?;
        let world = self.state.registered_worlds.get(&world_id)?;
        Some(HostedWorldSummary {
            universe_id: world.universe_id,
            world_id,
            world_root: String::new(),
            manifest_hash: runtime_summary.manifest_hash,
            world_epoch: runtime_summary.world_epoch,
            effective_partition: runtime_summary.effective_partition,
            next_world_seq: runtime_summary.next_world_seq,
            workflow_modules: world.metadata.workflow_modules.clone(),
            warnings: world.metadata.warnings.clone(),
        })
    }

    pub(super) fn registered_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<RegisteredWorldSummary> {
        let _ = universe_id;
        let world = self.state.registered_worlds.get(&world_id)?;
        Some(RegisteredWorldSummary {
            universe_id: world.universe_id,
            world_id,
            world_epoch: world.world_epoch,
            effective_partition: partition_for_world(world_id, self.infra.kafka.partition_count()),
            manifest_hash: world.manifest_hash.clone(),
            next_world_seq: self.infra.kafka.next_world_seq(world_id),
        })
    }

    pub(super) fn checkpoint_entry_for_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<(aos_node::PartitionCheckpoint, aos_node::WorldCheckpointRef)>, WorkerError>
    {
        let effective_partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        Ok(self
            .infra
            .blob_meta_for_domain_mut(universe_id)?
            .latest_checkpoint(&journal_topic, effective_partition)
            .cloned()
            .and_then(|checkpoint| {
                checkpoint
                    .worlds
                    .iter()
                    .find(|item| item.universe_id == universe_id && item.world_id == world_id)
                    .cloned()
                    .map(|item| (checkpoint, item))
            }))
    }

    pub(super) fn checkpoint_watermark_for_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<(u64, u64)>, WorkerError> {
        Ok(self
            .checkpoint_entry_for_world(universe_id, world_id)?
            .map(|(checkpoint, entry)| {
                (
                    entry.baseline.height,
                    if entry.checkpointed_at_ns != 0 {
                        entry.checkpointed_at_ns
                    } else {
                        checkpoint.created_at_ns
                    },
                )
            }))
    }

    pub(super) fn prepare_projection_continuity_for_reopen(
        &mut self,
        world_id: WorldId,
        journal_head: u64,
        active_baseline: &SnapshotRecord,
    ) -> Result<bool, WorkerError> {
        let world =
            self.state
                .registered_worlds
                .get_mut(&world_id)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                })?;
        let continuity_matches = world
            .projection_continuity
            .as_ref()
            .is_some_and(|continuity| {
                continuity.world_epoch == world.world_epoch
                    && continuity.last_projected_head == journal_head
                    && continuity.active_baseline == *active_baseline
            });
        if continuity_matches {
            return Ok(true);
        }
        world.projection_token = Uuid::new_v4().to_string();
        world.projection_continuity = None;
        Ok(false)
    }

    pub(super) fn record_projection_publish_success(
        &mut self,
        world_id: WorldId,
        journal_head: u64,
        active_baseline: SnapshotRecord,
    ) -> Result<(), WorkerError> {
        let world_epoch = self
            .state
            .registered_worlds
            .get(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id: self.infra.default_universe_id,
                world_id,
            })?
            .world_epoch;
        self.state
            .registered_worlds
            .get_mut(&world_id)
            .ok_or(WorkerError::UnknownWorld {
                universe_id: self.infra.default_universe_id,
                world_id,
            })?
            .projection_continuity = Some(ProjectionContinuity {
            world_epoch,
            last_projected_head: journal_head,
            active_baseline,
        });
        if let Some(active_world) = self.state.active_worlds.get_mut(&world_id) {
            active_world.projection_bootstrapped = true;
        }
        Ok(())
    }

    pub(super) fn invalidate_projection_continuity(
        &mut self,
        world_id: WorldId,
    ) -> Result<(), WorkerError> {
        let new_token = Uuid::new_v4().to_string();
        {
            let world = self.state.registered_worlds.get_mut(&world_id).ok_or(
                WorkerError::UnknownWorld {
                    universe_id: self.infra.default_universe_id,
                    world_id,
                },
            )?;
            world.projection_token = new_token;
            world.projection_continuity = None;
        }
        if let Some(active_world) = self.state.active_worlds.get_mut(&world_id) {
            active_world.projection_bootstrapped = false;
        }
        Ok(())
    }

    pub(super) fn reopen_registered_world_host(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldHost<crate::blobstore::HostedCas>, WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        let (world_store, world_loaded, universe_id) =
            {
                let world = self.state.registered_worlds.get(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                (
                    Arc::clone(&world.store),
                    world.loaded.clone(),
                    world.universe_id,
                )
            };
        let effective_partition = partition_for_world(world_id, self.infra.kafka.partition_count());
        self.infra
            .kafka
            .recover_partition_from_broker(effective_partition)?;
        let checkpoint = self.checkpoint_entry_for_world(universe_id, world_id)?;

        let host = match checkpoint {
            Some((_checkpoint, checkpoint_world)) => {
                let tail_frames = self
                    .infra
                    .kafka
                    .world_frames(world_id)
                    .iter()
                    .filter(|frame| {
                        frame.universe_id == universe_id
                            && frame.world_seq_end > checkpoint_world.baseline.height
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                open_plane_world_from_checkpoint(
                    Arc::clone(&world_store),
                    world_loaded.clone(),
                    &checkpoint_world.baseline,
                    &tail_frames,
                    self.infra.world_config_for_domain(universe_id)?,
                    EffectAdapterConfig::default(),
                    self.kernel_config_for_world(universe_id)?,
                )
                .map_err(WorkerError::LogFirst)
            }
            None => {
                let frames = self.infra.kafka.world_frames(world_id).to_vec();
                open_plane_world_from_frames(
                    world_store,
                    world_loaded,
                    &frames,
                    self.infra.world_config_for_domain(universe_id)?,
                    EffectAdapterConfig::default(),
                    self.kernel_config_for_world(universe_id)?,
                )
                .map_err(WorkerError::LogFirst)
            }
        }?;
        Ok(host)
    }

    pub(super) fn reopen_registered_world_host_from_log(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldHost<crate::blobstore::HostedCas>, WorkerError> {
        self.ensure_registered_world(universe_id, world_id)?;
        let (world_store, world_loaded, universe_id) =
            {
                let world = self.state.registered_worlds.get(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                (
                    Arc::clone(&world.store),
                    world.loaded.clone(),
                    world.universe_id,
                )
            };
        let frames = self.infra.kafka.world_frames(world_id).to_vec();
        let host = open_plane_world_from_frames(
            world_store,
            world_loaded,
            &frames,
            self.infra.world_config_for_domain(universe_id)?,
            EffectAdapterConfig::default(),
            self.kernel_config_for_world(universe_id)?,
        )
        .map_err(WorkerError::LogFirst)?;
        Ok(host)
    }
}

fn is_builtin_manifest_ref(name: &str) -> bool {
    builtins::find_builtin_schema(name).is_some()
        || builtins::find_builtin_module(name).is_some()
        || builtins::find_builtin_effect(name).is_some()
        || builtins::find_builtin_cap(name).is_some()
}

fn is_builtin_module(name: &str) -> bool {
    builtins::find_builtin_module(name).is_some()
}

fn is_zero_hash(value: &str) -> bool {
    let trimmed = value.strip_prefix("sha256:").unwrap_or(value);
    trimmed.len() == 64 && trimmed.bytes().all(|byte| byte == b'0')
}
