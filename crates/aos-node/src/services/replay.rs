use std::sync::Arc;

use aos_air_types::{DefModule, ModuleRuntime, WasmArtifact, builtins};
use aos_cbor::{HASH_PREFIX, Hash};
use aos_kernel::journal::{Journal, OwnedJournalEntry, SnapshotRecord as KernelSnapshotRecord};
use aos_kernel::{
    Kernel, KernelConfig, LoadedManifest, ManifestLoader, TraceQuery, trace_get,
    workflow_trace_summary_with_routes,
};
use aos_node::{
    BackendError, CreateWorldRequest, ForkWorldRequest, ImportedSeedSource, SeedKind,
    SnapshotRecord, SnapshotSelector, UniverseId, WorldConfig, WorldId,
    rewrite_snapshot_for_fork_policy,
};
use serde_json::Value as JsonValue;

use crate::blobstore::HostedCas;
use crate::services::{HostedCasService, HostedJournalService, HostedMetaService};
use crate::vault::HostedVault;
use crate::worker::WorkerError;

#[derive(Clone)]
pub struct HostedReplayService {
    journal: HostedJournalService,
    stores: HostedCasService,
    meta: HostedMetaService,
    vault: HostedVault,
}

impl HostedReplayService {
    pub fn new(
        journal: HostedJournalService,
        stores: HostedCasService,
        meta: HostedMetaService,
        vault: HostedVault,
    ) -> Self {
        Self {
            journal,
            stores,
            meta,
            vault,
        }
    }

    pub fn create_fork_seed_request(
        &self,
        universe_id: UniverseId,
        new_world_id: WorldId,
        request: &ForkWorldRequest,
    ) -> Result<CreateWorldRequest, WorkerError> {
        self.create_fork_seed_request_inner(universe_id, new_world_id, request)
    }

    pub fn trace_summary(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<JsonValue, WorkerError> {
        let kernel = self.open_world_kernel(universe_id, world_id)?;
        workflow_trace_summary_with_routes(&kernel, None).map_err(WorkerError::Kernel)
    }

    pub fn trace(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        query: TraceQuery,
    ) -> Result<JsonValue, WorkerError> {
        let kernel = self.open_world_kernel(universe_id, world_id)?;
        Ok(trace_get(&kernel, query)?)
    }

    pub fn state_json(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<&str>,
    ) -> Result<Option<JsonValue>, WorkerError> {
        let kernel = self.open_world_kernel(universe_id, world_id)?;
        let key_cbor = key
            .map(|value| aos_cbor::to_canonical_cbor(&value))
            .transpose()
            .map_err(WorkerError::from)?;
        let Some(bytes) = kernel
            .workflow_state_bytes(workflow, key_cbor.as_deref())
            .map_err(WorkerError::Kernel)?
        else {
            return Ok(None);
        };
        let cbor_value: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
        Ok(Some(serde_json::to_value(cbor_value)?))
    }
}

impl HostedReplayService {
    fn kernel_config_for_world(
        &self,
        universe_id: UniverseId,
    ) -> Result<KernelConfig, WorkerError> {
        let fallback_module_cache_dir = self.stores.module_cache_dir_for_domain(universe_id)?;
        let world_config =
            WorldConfig::from_env_with_fallback_module_cache_dir(fallback_module_cache_dir);
        Ok(world_config.apply_kernel_defaults(KernelConfig {
            universe_id: universe_id.as_uuid(),
            secret_resolver: Some(Arc::new(self.vault.resolver_for_universe(universe_id))),
            ..KernelConfig::default()
        }))
    }

    fn load_manifest_into_local_cas(
        &self,
        universe_id: UniverseId,
        manifest_hash: &str,
    ) -> Result<LoadedManifest, WorkerError> {
        let manifest = parse_plane_hash_like(manifest_hash, "manifest_hash")?;
        let store = self.stores.store_for_domain(universe_id)?;
        let manifest_bytes = store.get(manifest).map_err(WorkerError::Persist)?;
        let manifest_doc: aos_air_types::Manifest = serde_cbor::from_slice(&manifest_bytes)?;
        for named in manifest_doc
            .schemas
            .iter()
            .chain(manifest_doc.modules.iter())
            .chain(manifest_doc.workflows.iter())
            .chain(manifest_doc.effects.iter())
            .chain(manifest_doc.secrets.iter())
        {
            if is_builtin_manifest_ref(named.name.as_str()) {
                continue;
            }
            let hash = parse_plane_hash_like(named.hash.as_str(), "manifest_ref")?;
            let _ = store.get(hash).map_err(WorkerError::Persist)?;
        }
        let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest)?;
        for module in loaded.modules.values() {
            let Some(hash_ref) = wasm_module_hash(module) else {
                continue;
            };
            if is_builtin_module(module.name.as_str()) || is_zero_hash(hash_ref.as_str()) {
                continue;
            }
            let wasm_hash = aos_cbor::Hash::from_hex_str(hash_ref.as_str()).map_err(|_| {
                WorkerError::LogFirst(BackendError::InvalidHashRef(hash_ref.to_string()))
            })?;
            let _ = store.get(wasm_hash).map_err(WorkerError::Persist)?;
        }
        Ok(loaded)
    }

    fn create_fork_seed_request_inner(
        &self,
        universe_id: UniverseId,
        new_world_id: WorldId,
        request: &ForkWorldRequest,
    ) -> Result<CreateWorldRequest, WorkerError> {
        aos_node::validate_fork_world_request(request)?;
        let selected =
            self.select_source_snapshot(universe_id, request.src_world_id, &request.src_snapshot)?;
        let store = self.stores.store_for_domain(selected.universe_id)?;
        let _ = store
            .get(parse_plane_hash_like(
                &selected.snapshot_ref,
                "src_snapshot_ref",
            )?)
            .map_err(WorkerError::Persist)?;
        let selected_bytes = store
            .get(parse_plane_hash_like(
                &selected.snapshot_ref,
                "src_snapshot_ref",
            )?)
            .map_err(WorkerError::Persist)?;
        let rewritten =
            rewrite_snapshot_for_fork_policy(&selected_bytes, &request.pending_effect_policy)?;
        let snapshot_ref = if let Some(bytes) = rewritten {
            store.put_verified(&bytes)?.to_hex()
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

    fn select_source_snapshot(
        &self,
        universe_id_hint: UniverseId,
        world_id: WorldId,
        selector: &SnapshotSelector,
    ) -> Result<SnapshotRecord, WorkerError> {
        self.journal.refresh()?;
        let checkpoint = self.checkpoint_entry_for_world(universe_id_hint, world_id)?;
        let (universe_id, checkpoint_snapshot, tail_frames) = if let Some(checkpoint) = checkpoint {
            let universe_id = checkpoint.universe_id;
            let checkpoint_snapshot = snapshot_record_from_checkpoint(&checkpoint.baseline);
            let tail_frames = self.journal.world_tail_frames(
                world_id,
                checkpoint.baseline.height,
                checkpoint.journal_cursor.clone(),
            )?;
            (universe_id, Some(checkpoint_snapshot), tail_frames)
        } else {
            let frames = self.journal.world_frames(world_id)?;
            let universe_id = resolved_universe_from_frames(&frames).unwrap_or(universe_id_hint);
            (universe_id, None, frames)
        };
        match selector {
            SnapshotSelector::ActiveBaseline => snapshot_record_from_frames(&tail_frames, |_| true)
                .or(checkpoint_snapshot)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                }),
            SnapshotSelector::ByHeight { height } => checkpoint_snapshot
                .filter(|snapshot| snapshot.height == *height)
                .or_else(|| {
                    snapshot_record_from_frames(&tail_frames, |snapshot| snapshot.height == *height)
                })
                .ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::not_found(format!(
                        "snapshot at height {height} not found for world {world_id}"
                    )))
                }),
            SnapshotSelector::ByRef { snapshot_ref } => checkpoint_snapshot
                .filter(|snapshot| snapshot.snapshot_ref == *snapshot_ref)
                .or_else(|| {
                    snapshot_record_from_frames(&tail_frames, |snapshot| {
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

    fn checkpoint_entry_for_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<Option<aos_node::WorldCheckpointRef>, WorkerError> {
        self.meta.latest_world_checkpoint(universe_id, world_id)
    }

    fn open_world_kernel(
        &self,
        universe_id_hint: UniverseId,
        world_id: WorldId,
    ) -> Result<Kernel<HostedCas>, WorkerError> {
        self.journal.refresh()?;
        let checkpoint = self.checkpoint_entry_for_world(universe_id_hint, world_id)?;
        if let Some(checkpoint_world) = checkpoint {
            let universe_id = checkpoint_world.universe_id;
            let loaded = self.load_manifest_into_local_cas(
                universe_id,
                &checkpoint_world.baseline.manifest_hash,
            )?;
            let store = self.stores.store_for_domain(universe_id)?;
            return self.open_world_kernel_with_checkpoint(
                universe_id,
                world_id,
                &store,
                loaded,
                &checkpoint_world,
            );
        }

        let frames = self.journal.world_frames(world_id)?;
        let universe_id = resolved_universe_from_frames(&frames).unwrap_or(universe_id_hint);
        let manifest_hash =
            manifest_hash_from_frames(&frames).ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let loaded = self.load_manifest_into_local_cas(universe_id, &manifest_hash)?;
        let store = self.stores.store_for_domain(universe_id)?;
        self.open_world_kernel_with_frames(universe_id, world_id, &store, loaded, &frames)
    }

    fn open_world_kernel_with_frames(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        store: &Arc<HostedCas>,
        loaded: LoadedManifest,
        frames: &[aos_node::WorldLogFrame],
    ) -> Result<Kernel<HostedCas>, WorkerError> {
        let active_baseline =
            snapshot_record_from_frames(frames, |_| true).ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let tail_frames = frames
            .iter()
            .filter(|frame| frame.world_seq_end > active_baseline.height)
            .cloned()
            .collect::<Vec<_>>();
        reopen_kernel_from_frame_log(
            Arc::clone(store),
            loaded,
            &active_baseline,
            &tail_frames,
            self.kernel_config_for_world(universe_id)?,
        )
    }

    fn open_world_kernel_with_checkpoint(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        store: &Arc<HostedCas>,
        loaded: LoadedManifest,
        checkpoint: &aos_node::WorldCheckpointRef,
    ) -> Result<Kernel<HostedCas>, WorkerError> {
        let tail_frames = self
            .journal
            .world_tail_frames(
                world_id,
                checkpoint.baseline.height,
                checkpoint.journal_cursor.clone(),
            )?
            .into_iter()
            .filter(|frame| frame.universe_id == universe_id)
            .collect::<Vec<_>>();
        reopen_kernel_from_frame_log(
            Arc::clone(store),
            loaded,
            &snapshot_record_from_checkpoint(&checkpoint.baseline),
            &tail_frames,
            self.kernel_config_for_world(universe_id)?,
        )
    }
}

fn snapshot_record_from_checkpoint(baseline: &aos_node::PromotableBaselineRef) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: baseline.snapshot_ref.clone(),
        height: baseline.height,
        universe_id: baseline.universe_id,
        logical_time_ns: baseline.logical_time_ns,
        receipt_horizon_height: Some(baseline.receipt_horizon_height),
        manifest_hash: Some(baseline.manifest_hash.clone()),
    }
}

fn snapshot_record_from_frames(
    frames: &[aos_node::WorldLogFrame],
    mut predicate: impl FnMut(&SnapshotRecord) -> bool,
) -> Option<SnapshotRecord> {
    for frame in frames.iter().rev() {
        for record in frame.records.iter().rev() {
            let aos_kernel::journal::JournalRecord::Snapshot(snapshot) = record else {
                continue;
            };
            let candidate = SnapshotRecord {
                snapshot_ref: snapshot.snapshot_ref.clone(),
                height: snapshot.height,
                universe_id: snapshot.universe_id.into(),
                logical_time_ns: snapshot.logical_time_ns,
                receipt_horizon_height: snapshot.receipt_horizon_height,
                manifest_hash: snapshot.manifest_hash.clone(),
            };
            if predicate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn manifest_hash_from_frames(frames: &[aos_node::WorldLogFrame]) -> Option<String> {
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

fn resolved_universe_from_frames(frames: &[aos_node::WorldLogFrame]) -> Option<UniverseId> {
    frames.last().map(|frame| frame.universe_id)
}

fn is_builtin_manifest_ref(name: &str) -> bool {
    builtins::find_builtin_schema(name).is_some()
        || builtins::find_builtin_module(name).is_some()
        || builtins::find_builtin_workflow(name).is_some()
        || builtins::find_builtin_effect(name).is_some()
}

fn is_builtin_module(name: &str) -> bool {
    builtins::find_builtin_module(name).is_some()
}

fn is_zero_hash(value: &str) -> bool {
    let trimmed = value.strip_prefix("sha256:").unwrap_or(value);
    trimmed.len() == 64 && trimmed.bytes().all(|byte| byte == b'0')
}

fn wasm_module_hash(module: &DefModule) -> Option<&aos_air_types::HashRef> {
    match &module.runtime {
        ModuleRuntime::Wasm {
            artifact: WasmArtifact::WasmModule { hash },
        } => Some(hash),
        _ => None,
    }
}

fn parse_plane_hash_like(value: &str, field: &str) -> Result<Hash, WorkerError> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with(HASH_PREFIX) {
        trimmed.to_string()
    } else {
        format!("{HASH_PREFIX}{trimmed}")
    };
    Hash::from_hex_str(&normalized).map_err(|_| {
        WorkerError::LogFirst(BackendError::InvalidHashRef(format!(
            "invalid {field} '{value}'"
        )))
    })
}

fn journal_entries_from_world_frames(
    frames: &[aos_node::WorldLogFrame],
) -> Result<Vec<OwnedJournalEntry>, WorkerError> {
    let mut entries = Vec::new();
    for frame in frames {
        for (offset, record) in frame.records.iter().enumerate() {
            entries.push(OwnedJournalEntry {
                seq: frame.world_seq_start + offset as u64,
                kind: record.kind(),
                payload: serde_cbor::to_vec(record)?,
            });
        }
    }
    Ok(entries)
}

fn kernel_snapshot_record(snapshot: &SnapshotRecord) -> KernelSnapshotRecord {
    KernelSnapshotRecord {
        snapshot_ref: snapshot.snapshot_ref.clone(),
        height: snapshot.height,
        universe_id: snapshot.universe_id.as_uuid(),
        logical_time_ns: snapshot.logical_time_ns,
        receipt_horizon_height: snapshot.receipt_horizon_height,
        manifest_hash: snapshot.manifest_hash.clone(),
    }
}

fn reopen_kernel_from_frame_log(
    store: Arc<HostedCas>,
    loaded: LoadedManifest,
    active_baseline: &SnapshotRecord,
    frames: &[aos_node::WorldLogFrame],
    kernel_config: KernelConfig,
) -> Result<Kernel<HostedCas>, WorkerError> {
    if frames.is_empty() {
        let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
            store,
            loaded,
            Journal::new(),
            kernel_config,
        )?;
        kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
        kernel.compact_journal_through(active_baseline.height)?;
        return Ok(kernel);
    }

    let replay_entries = journal_entries_from_world_frames(frames)?;
    let replay_from = replay_entries.first().map(|entry| entry.seq).unwrap_or(0);
    let journal =
        Journal::from_entries(&replay_entries).map_err(|err| WorkerError::Build(err.into()))?;
    let mut kernel = Kernel::from_loaded_manifest_without_replay_with_config(
        store,
        loaded,
        journal,
        kernel_config,
    )?;
    kernel.restore_snapshot_record(&kernel_snapshot_record(active_baseline))?;
    kernel.replay_entries_from(replay_from)?;
    kernel.compact_journal_through(active_baseline.height)?;
    Ok(kernel)
}
