use std::sync::Arc;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::journal::Journal;
use aos_kernel::{KernelConfig, LoadedManifest, ManifestLoader, Store};
use aos_runtime::{HostError, WorldConfig, WorldHost};

use crate::{CreateWorldRequest, CreateWorldSource, PersistError, UniverseId, WorldId};

use super::decode::{
    journal_entries_from_world_frames, latest_plane_snapshot_record, parse_plane_hash_like,
};
use super::model::{CanonicalWorldRecord, PromotableBaselineRef, WorldLogFrame};
use super::traits::{PlaneCreatedWorld, PlaneError};

pub fn open_plane_world_from_frames<S: Store + 'static>(
    store: Arc<S>,
    loaded: LoadedManifest,
    frames: &[WorldLogFrame],
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<WorldHost<S>, PlaneError> {
    let replay_entries = journal_entries_from_world_frames(frames)?;
    Ok(WorldHost::from_loaded_manifest_with_journal_replay(
        store,
        loaded,
        Journal::from_entries(&replay_entries)
            .map_err(|err| PlaneError::Host(HostError::External(err.to_string())))?,
        world_config,
        adapter_config,
        kernel_config,
        None,
    )?)
}

pub fn open_plane_world_from_checkpoint<S: Store + 'static>(
    store: Arc<S>,
    loaded: LoadedManifest,
    baseline: &PromotableBaselineRef,
    tail_frames: &[WorldLogFrame],
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<WorldHost<S>, PlaneError> {
    let replay_entries = journal_entries_from_world_frames(tail_frames)?;
    let replay = aos_runtime::JournalReplayOpen {
        active_baseline: aos_kernel::journal::SnapshotRecord {
            snapshot_ref: baseline.snapshot_ref.clone(),
            height: baseline.height,
            universe_id: baseline.universe_id.as_uuid(),
            logical_time_ns: baseline.logical_time_ns,
            receipt_horizon_height: Some(baseline.receipt_horizon_height),
            manifest_hash: Some(baseline.manifest_hash.clone()),
        },
        replay_seed: None,
    };
    Ok(WorldHost::from_loaded_manifest_with_journal_replay(
        store,
        loaded,
        Journal::from_entries(&replay_entries)
            .map_err(|err| PlaneError::Host(HostError::External(err.to_string())))?,
        world_config,
        adapter_config,
        kernel_config,
        Some(replay),
    )?)
}

pub fn create_plane_world_from_request<S: Store + 'static>(
    store: Arc<S>,
    request: &CreateWorldRequest,
    universe_id: UniverseId,
    world_id: WorldId,
    world_epoch: u64,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    mut kernel_config: KernelConfig,
) -> Result<PlaneCreatedWorld<S>, PlaneError> {
    crate::validate_create_world_request(request)
        .map_err(|err| PlaneError::Persist(PersistError::validation(err.to_string())))?;
    kernel_config.universe_id = request.universe_id.as_uuid();
    match &request.source {
        CreateWorldSource::Manifest { manifest_hash } => {
            let initial_manifest_hash = parse_plane_hash_like(manifest_hash, "manifest_hash")?;
            let loaded = ManifestLoader::load_from_hash(store.as_ref(), initial_manifest_hash)?;
            let mut host = WorldHost::from_loaded_manifest_with_journal_replay(
                store,
                loaded,
                Journal::new(),
                world_config,
                adapter_config,
                kernel_config,
                None,
            )?;
            let journal_tail_start = host.journal_bounds().next_seq;
            host.snapshot()?;
            let tail = host.kernel().dump_journal_from(journal_tail_start)?;
            if tail.is_empty() {
                return Err(PlaneError::Host(HostError::External(
                    "create-world snapshot produced no journal records".into(),
                )));
            }
            let mut records = Vec::with_capacity(tail.len());
            for entry in &tail {
                let record: CanonicalWorldRecord = serde_cbor::from_slice(&entry.payload)?;
                records.push(record);
            }
            let active_baseline = latest_plane_snapshot_record(&tail)
                .map(|snapshot| crate::SnapshotRecord {
                    snapshot_ref: snapshot.snapshot_ref,
                    height: snapshot.height,
                    universe_id: snapshot.universe_id.into(),
                    logical_time_ns: snapshot.logical_time_ns,
                    receipt_horizon_height: snapshot.receipt_horizon_height,
                    manifest_hash: snapshot.manifest_hash,
                })
                .ok_or_else(|| {
                    PlaneError::Host(HostError::External("missing snapshot record".into()))
                })?;
            Ok(PlaneCreatedWorld {
                host,
                initial_manifest_hash: initial_manifest_hash.to_hex(),
                active_baseline,
                initial_frame: Some(WorldLogFrame {
                    format_version: 1,
                    universe_id,
                    world_id,
                    world_epoch,
                    world_seq_start: 0,
                    world_seq_end: records.len() as u64 - 1,
                    records,
                }),
            })
        }
        CreateWorldSource::Seed { seed } => {
            let manifest_hash =
                seed.baseline.manifest_hash.as_deref().ok_or_else(|| {
                    PersistError::validation("seed baseline requires manifest_hash")
                })?;
            let initial_manifest_hash =
                parse_plane_hash_like(manifest_hash, "seed.baseline.manifest_hash")?;
            let loaded = ManifestLoader::load_from_hash(store.as_ref(), initial_manifest_hash)?;
            let baseline = PromotableBaselineRef {
                snapshot_ref: seed.baseline.snapshot_ref.clone(),
                snapshot_manifest_ref: None,
                manifest_hash: manifest_hash.to_owned(),
                height: seed.baseline.height,
                universe_id: seed.baseline.universe_id,
                logical_time_ns: seed.baseline.logical_time_ns,
                receipt_horizon_height: seed
                    .baseline
                    .receipt_horizon_height
                    .unwrap_or(seed.baseline.height),
            };
            let host = open_plane_world_from_checkpoint(
                store,
                loaded,
                &baseline,
                &[],
                world_config,
                adapter_config,
                kernel_config,
            )?;
            Ok(PlaneCreatedWorld {
                host,
                initial_manifest_hash: initial_manifest_hash.to_hex(),
                active_baseline: seed.baseline.clone(),
                initial_frame: None,
            })
        }
    }
}

pub fn create_plane_world_from_manifest<S: Store + 'static>(
    store: Arc<S>,
    request: &CreateWorldRequest,
    universe_id: UniverseId,
    world_id: WorldId,
    world_epoch: u64,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
) -> Result<PlaneCreatedWorld<S>, PlaneError> {
    create_plane_world_from_request(
        store,
        request,
        universe_id,
        world_id,
        world_epoch,
        world_config,
        adapter_config,
        kernel_config,
    )
}
