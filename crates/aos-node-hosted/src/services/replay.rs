use std::sync::Arc;

use aos_air_types::builtins;
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::{KernelConfig, LoadedManifest, ManifestLoader};
use aos_node::{
    CreateWorldRequest, ForkWorldRequest, ImportedSeedSource, PlaneError, SeedKind, SnapshotRecord,
    SnapshotSelector, UniverseId, WorldId, open_plane_world_from_checkpoint,
    open_plane_world_from_frames, parse_plane_hash_like, partition_for_world,
    rewrite_snapshot_for_fork_policy,
};
use aos_runtime::trace::{TraceQuery as RuntimeTraceQuery, trace_get};
use aos_runtime::{WorldConfig, WorldHost};
use serde_json::Value as JsonValue;

use crate::blobstore::HostedCas;
use crate::services::{HostedCasService, HostedJournalService, HostedMetaService};
use crate::vault::HostedVault;
use crate::worker::WorkerError;

#[derive(Clone)]
pub struct HostedReplayService {
    paths: aos_node::LocalStatePaths,
    world_config: WorldConfig,
    journal: HostedJournalService,
    stores: HostedCasService,
    meta: HostedMetaService,
    vault: HostedVault,
}

impl HostedReplayService {
    pub fn new(
        paths: aos_node::LocalStatePaths,
        journal: HostedJournalService,
        stores: HostedCasService,
        meta: HostedMetaService,
        vault: HostedVault,
    ) -> Self {
        Self {
            paths,
            world_config: WorldConfig::from_env_with_fallback_module_cache_dir(None),
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
        Ok(self
            .open_world_host(universe_id, world_id)?
            .trace_summary()?)
    }

    pub fn trace(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        query: RuntimeTraceQuery,
    ) -> Result<JsonValue, WorkerError> {
        let host = self.open_world_host(universe_id, world_id)?;
        Ok(trace_get(host.kernel(), query)?)
    }

    pub fn state_json(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        workflow: &str,
        key: Option<&str>,
    ) -> Result<Option<JsonValue>, WorkerError> {
        let host = self.open_world_host(universe_id, world_id)?;
        let key_cbor = key
            .map(|value| aos_cbor::to_canonical_cbor(&value))
            .transpose()
            .map_err(WorkerError::from)?;
        let Some(bytes) = host.state(workflow, key_cbor.as_deref()) else {
            return Ok(None);
        };
        let cbor_value: serde_cbor::Value = serde_cbor::from_slice(&bytes)?;
        Ok(Some(serde_json::to_value(cbor_value)?))
    }
}

impl HostedReplayService {
    fn kernel_config_for_world(&self, universe_id: UniverseId) -> KernelConfig {
        KernelConfig {
            universe_id: universe_id.as_uuid(),
            secret_resolver: Some(Arc::new(self.vault.resolver_for_universe(universe_id))),
            ..KernelConfig::default()
        }
    }

    fn world_config_for_domain(&self, universe_id: UniverseId) -> Result<WorldConfig, WorkerError> {
        let mut config = self.world_config.clone();
        let domain_paths = self.paths.for_universe(universe_id);
        domain_paths.ensure_root().map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(err.to_string()))
        })?;
        std::fs::create_dir_all(domain_paths.cache_root()).map_err(|err| {
            WorkerError::Persist(aos_node::PersistError::backend(format!(
                "create hosted domain cache dir: {err}"
            )))
        })?;
        config.module_cache_dir = Some(domain_paths.wasmtime_cache_dir());
        Ok(config)
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
            .chain(manifest_doc.effects.iter())
            .chain(manifest_doc.caps.iter())
            .chain(manifest_doc.policies.iter())
        {
            if is_builtin_manifest_ref(named.name.as_str()) {
                continue;
            }
            let hash = parse_plane_hash_like(named.hash.as_str(), "manifest_ref")?;
            let _ = store.get(hash).map_err(WorkerError::Persist)?;
        }
        for secret in &manifest_doc.secrets {
            let aos_air_types::SecretEntry::Ref(named) = secret else {
                continue;
            };
            let hash = parse_plane_hash_like(named.hash.as_str(), "secret_ref")?;
            let _ = store.get(hash).map_err(WorkerError::Persist)?;
        }
        let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest)?;
        for module in loaded.modules.values() {
            if is_builtin_module(module.name.as_str()) || is_zero_hash(module.wasm_hash.as_str()) {
                continue;
            }
            let wasm_hash =
                aos_cbor::Hash::from_hex_str(module.wasm_hash.as_str()).map_err(|_| {
                    WorkerError::LogFirst(PlaneError::InvalidHashRef(module.wasm_hash.to_string()))
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
        let frames = self.journal.world_frames(world_id)?;
        let universe_id = resolved_universe_from_frames(&frames).unwrap_or(universe_id_hint);
        let checkpoint_snapshot = self
            .checkpoint_entry_for_world(universe_id, world_id)?
            .map(|(_, world)| snapshot_record_from_checkpoint(&world.baseline));
        match selector {
            SnapshotSelector::ActiveBaseline => snapshot_record_from_frames(&frames, |_| true)
                .or(checkpoint_snapshot)
                .ok_or(WorkerError::UnknownWorld {
                    universe_id,
                    world_id,
                }),
            SnapshotSelector::ByHeight { height } => checkpoint_snapshot
                .filter(|snapshot| snapshot.height == *height)
                .or_else(|| {
                    snapshot_record_from_frames(&frames, |snapshot| snapshot.height == *height)
                })
                .ok_or_else(|| {
                    WorkerError::Persist(aos_node::PersistError::not_found(format!(
                        "snapshot at height {height} not found for world {world_id}"
                    )))
                }),
            SnapshotSelector::ByRef { snapshot_ref } => checkpoint_snapshot
                .filter(|snapshot| snapshot.snapshot_ref == *snapshot_ref)
                .or_else(|| {
                    snapshot_record_from_frames(&frames, |snapshot| {
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
    ) -> Result<Option<(aos_node::PartitionCheckpoint, aos_node::WorldCheckpointRef)>, WorkerError>
    {
        let partition = partition_for_world(world_id, self.journal.partition_count()?);
        Ok(self
            .meta
            .latest_checkpoint(universe_id, partition)?
            .and_then(|checkpoint| {
                checkpoint
                    .worlds
                    .iter()
                    .find(|item| item.universe_id == universe_id && item.world_id == world_id)
                    .cloned()
                    .map(|item| (checkpoint, item))
            }))
    }

    fn open_world_host(
        &self,
        universe_id_hint: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldHost<HostedCas>, WorkerError> {
        self.journal.refresh()?;
        let frames = self.journal.world_frames(world_id)?;
        let universe_id = resolved_universe_from_frames(&frames).unwrap_or(universe_id_hint);
        let checkpoint = self.checkpoint_entry_for_world(universe_id, world_id)?;
        let manifest_hash = manifest_hash_from_frames(&frames)
            .or_else(|| {
                checkpoint
                    .as_ref()
                    .map(|(_, world)| world.baseline.manifest_hash.clone())
            })
            .ok_or(WorkerError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let loaded = self.load_manifest_into_local_cas(universe_id, &manifest_hash)?;
        let store = self.stores.store_for_domain(universe_id)?;
        let mut host = match checkpoint {
            Some((_checkpoint, checkpoint_world)) => self.open_world_host_with_checkpoint(
                universe_id,
                world_id,
                &store,
                loaded.clone(),
                &checkpoint_world.baseline,
            )?,
            None => self.open_world_host_with_frames(universe_id, &store, loaded, &frames)?,
        };
        let _ = host
            .kernel_mut()
            .drain_effects()
            .map_err(WorkerError::Kernel)?;
        Ok(host)
    }

    fn open_world_host_with_frames(
        &self,
        universe_id: UniverseId,
        store: &Arc<HostedCas>,
        loaded: LoadedManifest,
        frames: &[aos_node::WorldLogFrame],
    ) -> Result<WorldHost<HostedCas>, WorkerError> {
        open_plane_world_from_frames(
            Arc::clone(store),
            loaded,
            frames,
            self.world_config_for_domain(universe_id)?,
            EffectAdapterConfig::default(),
            self.kernel_config_for_world(universe_id),
        )
        .map_err(WorkerError::LogFirst)
    }

    fn open_world_host_with_checkpoint(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
        store: &Arc<HostedCas>,
        loaded: LoadedManifest,
        baseline: &aos_node::PromotableBaselineRef,
    ) -> Result<WorldHost<HostedCas>, WorkerError> {
        let tail_frames = self
            .journal
            .world_frames(world_id)?
            .into_iter()
            .filter(|frame| {
                frame.universe_id == universe_id && frame.world_seq_end > baseline.height
            })
            .collect::<Vec<_>>();
        open_plane_world_from_checkpoint(
            Arc::clone(store),
            loaded,
            baseline,
            &tail_frames,
            self.world_config_for_domain(universe_id)?,
            EffectAdapterConfig::default(),
            self.kernel_config_for_world(universe_id),
        )
        .map_err(WorkerError::LogFirst)
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
