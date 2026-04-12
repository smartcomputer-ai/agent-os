#[path = "../../aos-runtime/tests/helpers.rs"]
mod helpers;

use aos_effect_types::{GovPatchInput, GovProposeParams};
use aos_kernel::journal::JournalRecord;
use aos_kernel::{Store, governance::ManifestPatch};
use aos_node::{
    CommandIngress, CreateWorldRequest, CreateWorldSource, ImportedSeedSource, MemoryLogRuntime,
    MemoryShardWorker, SeedKind, SnapshotRecord, SubmissionEnvelope, SubmissionPlane,
    SubmissionRejection, WorldLogPlane, WorldSeed,
};
use uuid::Uuid;

#[test]
fn memory_log_runtime_batches_submission_into_authoritative_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::timer_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(8)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;

    let payload = serde_cbor::to_vec(&helpers::fixtures::start_event("frame"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-1",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        payload,
    ))?;

    let frames = runtime.process_partition(partition)?;
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert_eq!(frame.world_seq_start, 0);
    assert_eq!(frame.world_seq_end + 1, frame.records.len() as u64);
    assert!(matches!(frame.records[0], JournalRecord::DomainEvent(_)));
    assert!(
        frame
            .records
            .iter()
            .any(|record| matches!(record, JournalRecord::EffectIntent(_)))
    );
    assert_eq!(runtime.world_frames(world).len(), 1);
    assert_eq!(runtime.pending_submission_count(), 0);
    Ok(())
}

#[test]
fn memory_log_runtime_rejects_stale_world_epoch_without_advancing_world_log()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;

    let payload = serde_cbor::to_vec(&helpers::fixtures::start_event("stale"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-stale",
        universe,
        world,
        world_epoch.saturating_sub(1),
        helpers::fixtures::START_SCHEMA,
        payload,
    ))?;

    let frames = runtime.process_partition(partition)?;
    assert!(frames.is_empty());
    assert!(runtime.world_frames(world).is_empty());
    assert_eq!(runtime.rejected_submissions().len(), 1);
    assert_eq!(
        runtime.rejected_submissions()[0].reason,
        SubmissionRejection::WorldEpochMismatch {
            expected: world_epoch,
            got: world_epoch.saturating_sub(1),
        }
    );
    Ok(())
}

#[test]
fn memory_log_runtime_rejects_duplicate_submission_id_without_advancing_world_log()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;

    let payload = serde_cbor::to_vec(&helpers::fixtures::start_event("dup"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-dup",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        payload.clone(),
    ))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-dup",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        payload,
    ))?;

    let frames = runtime.process_partition(partition)?;
    assert_eq!(frames.len(), 1);
    assert_eq!(runtime.world_frames(world).len(), 1);
    assert_eq!(runtime.rejected_submissions().len(), 1);
    assert_eq!(
        runtime.rejected_submissions()[0].reason,
        SubmissionRejection::DuplicateSubmissionId
    );
    Ok(())
}

#[test]
fn memory_log_runtime_replays_world_from_authoritative_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(2)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;

    let payload = serde_cbor::to_vec(&helpers::fixtures::start_event("replay"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-replay",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        payload,
    ))?;
    runtime.process_partition(partition)?;

    let live = runtime
        .world_host(universe, world)
        .expect("live world")
        .state("com.acme/Simple@1", None);
    let replayed = runtime.replay_world(universe, world)?;
    let replay_state = replayed.state("com.acme/Simple@1", None);

    assert_eq!(live, Some(vec![0xAA]));
    assert_eq!(replay_state, live);
    Ok(())
}

#[test]
fn bump_world_epoch_fences_old_submissions() -> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(8)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;

    let partition = runtime.effective_partition_for(universe, world)?;
    let bumped_world_epoch = runtime.bump_world_epoch(universe, world)?;
    assert!(bumped_world_epoch > world_epoch);

    let stale_payload = serde_cbor::to_vec(&helpers::fixtures::start_event("stale-after-reroute"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-stale-after-reroute",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        stale_payload,
    ))?;

    let stale_frames = runtime.process_partition(partition)?;
    assert!(stale_frames.is_empty());
    assert_eq!(runtime.rejected_submissions().len(), 1);
    assert_eq!(
        runtime.rejected_submissions()[0].reason,
        SubmissionRejection::WorldEpochMismatch {
            expected: bumped_world_epoch,
            got: world_epoch,
        }
    );

    let accepted_payload =
        serde_cbor::to_vec(&helpers::fixtures::start_event("accepted-after-reroute"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-accepted-after-reroute",
        universe,
        world,
        bumped_world_epoch,
        helpers::fixtures::START_SCHEMA,
        accepted_payload,
    ))?;

    let accepted_frames = runtime.process_partition(partition)?;
    assert_eq!(accepted_frames.len(), 1);
    Ok(())
}

#[test]
fn shard_worker_checkpoint_recovers_world_and_replays_tail()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;
    let mut worker = MemoryShardWorker::new(partition);

    let first_payload = serde_cbor::to_vec(&helpers::fixtures::start_event("before-checkpoint"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-before-checkpoint",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        first_payload,
    ))?;
    worker.run_once(&mut runtime)?;

    let checkpoint = worker.publish_checkpoint(&mut runtime, 1234)?;
    assert_eq!(checkpoint.partition, partition);
    assert_eq!(checkpoint.worlds.len(), 1);
    let live_bounds_after_checkpoint = runtime
        .world_host(universe, world)
        .expect("live world after checkpoint")
        .journal_bounds();
    assert_eq!(
        live_bounds_after_checkpoint.retained_from,
        checkpoint.worlds[0].baseline.height.saturating_add(1)
    );

    let second_payload = serde_cbor::to_vec(&helpers::fixtures::start_event("after-checkpoint"))?;
    runtime.submit(SubmissionEnvelope::domain_event(
        "submission-after-checkpoint",
        universe,
        world,
        world_epoch,
        helpers::fixtures::START_SCHEMA,
        second_payload,
    ))?;
    worker.run_once(&mut runtime)?;

    let live = runtime
        .world_host(universe, world)
        .expect("live world")
        .state("com.acme/Simple@1", None);
    let recovered = runtime.replay_world_from_checkpoint(&checkpoint, universe, world)?;
    let recovered_state = recovered.state("com.acme/Simple@1", None);
    let recovered_journal = recovered.kernel().dump_journal()?;

    assert_eq!(live, Some(vec![0xAA]));
    assert_eq!(recovered_state, live);
    assert_eq!(recovered_journal.len(), 1);
    assert!(matches!(
        recovered_journal[0].kind,
        aos_kernel::journal::JournalKind::DomainEvent
    ));
    Ok(())
}

#[test]
fn memory_log_runtime_creates_world_from_create_submission()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let seed_world = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();

    runtime.register_world(universe, seed_world, store.clone(), loaded.clone())?;
    let manifest_hash = runtime
        .world_host(universe, seed_world)
        .expect("seed world")
        .kernel()
        .manifest_hash()
        .to_hex();

    runtime.submit(SubmissionEnvelope::create_world(
        "create-world-1",
        universe,
        world,
        CreateWorldRequest {
            world_id: Some(world),
            universe_id: aos_node::UniverseId::nil(),
            created_at_ns: 0,
            source: CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.clone(),
            },
        },
    ))?;

    let partition = aos_node::partition_for_world(world, runtime.partition_count());
    let frames = runtime.process_partition(partition)?;
    assert_eq!(frames.len(), 1);
    assert!(runtime.registered_world(universe, world).is_some());
    assert!(!runtime.world_frames(world).is_empty());
    let replayed = runtime.replay_world(universe, world)?;
    assert_eq!(replayed.kernel().manifest_hash().to_hex(), manifest_hash);
    Ok(())
}

#[test]
fn memory_log_runtime_creates_seeded_world_from_existing_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let seed_world = Uuid::new_v4().into();
    let source_world = Uuid::new_v4().into();
    let seeded_world = Uuid::new_v4().into();
    runtime.register_world(universe, seed_world, store.clone(), loaded.clone())?;
    let manifest_hash = runtime
        .world_host(universe, seed_world)
        .expect("seed world")
        .kernel()
        .manifest_hash()
        .to_hex();

    runtime.submit(SubmissionEnvelope::create_world(
        "create-source-world",
        universe,
        source_world,
        CreateWorldRequest {
            world_id: Some(source_world),
            universe_id: aos_node::UniverseId::nil(),
            created_at_ns: 0,
            source: CreateWorldSource::Manifest {
                manifest_hash: manifest_hash.clone(),
            },
        },
    ))?;
    let source_partition = aos_node::partition_for_world(source_world, runtime.partition_count());
    let source_frames = runtime.process_partition(source_partition)?;
    assert_eq!(source_frames.len(), 1);
    let source_baseline = source_frames[0]
        .records
        .iter()
        .find_map(|record| match record {
            JournalRecord::Snapshot(snapshot) => Some(SnapshotRecord {
                snapshot_ref: snapshot.snapshot_ref.clone(),
                height: snapshot.height,
                universe_id: snapshot.universe_id.into(),
                logical_time_ns: snapshot.logical_time_ns,
                receipt_horizon_height: snapshot.receipt_horizon_height,
                manifest_hash: snapshot.manifest_hash.clone(),
            }),
            _ => None,
        })
        .expect("source create-world snapshot record");

    runtime.submit(SubmissionEnvelope::create_world(
        "create-seeded-world",
        universe,
        seeded_world,
        CreateWorldRequest {
            world_id: Some(seeded_world),
            universe_id: aos_node::UniverseId::nil(),
            created_at_ns: 7,
            source: CreateWorldSource::Seed {
                seed: WorldSeed {
                    baseline: source_baseline.clone(),
                    seed_kind: SeedKind::Import,
                    imported_from: Some(ImportedSeedSource {
                        source: "memory-test".into(),
                        external_world_id: Some(source_world.to_string()),
                        external_snapshot_ref: Some(source_baseline.snapshot_ref.clone()),
                    }),
                },
            },
        },
    ))?;
    let seeded_partition = aos_node::partition_for_world(seeded_world, runtime.partition_count());
    let seeded_frames = runtime.process_partition(seeded_partition)?;
    assert!(seeded_frames.is_empty());
    assert!(runtime.registered_world(universe, seeded_world).is_some());
    assert!(runtime.world_frames(seeded_world).is_empty());
    let replayed = runtime.replay_world(universe, seeded_world)?;
    assert_eq!(replayed.kernel().manifest_hash().to_hex(), manifest_hash);
    Ok(())
}

#[test]
fn memory_log_runtime_processes_governance_command_submission()
-> Result<(), Box<dyn std::error::Error>> {
    let store = helpers::fixtures::new_mem_store();
    let loaded = helpers::simple_state_manifest(&store);
    let mut runtime = MemoryLogRuntime::new(4)?;
    let universe = Uuid::new_v4().into();
    let world = Uuid::new_v4().into();
    let world_epoch = runtime.register_world(universe, world, store, loaded)?;
    let partition = runtime.effective_partition_for(universe, world)?;

    let patch = ManifestPatch {
        manifest: runtime
            .world_host(universe, world)
            .expect("registered world")
            .store()
            .get_node(
                runtime
                    .world_host(universe, world)
                    .expect("registered world")
                    .kernel()
                    .manifest_hash(),
            )?,
        nodes: Vec::new(),
    };
    runtime.submit(SubmissionEnvelope::command(
        "gov-propose-1",
        universe,
        world,
        world_epoch,
        CommandIngress {
            command_id: "cmd-gov-propose-1".into(),
            command: "gov-propose".into(),
            actor: None,
            payload: aos_node::CborPayload::inline(serde_cbor::to_vec(&GovProposeParams {
                patch: GovPatchInput::PatchCbor(serde_cbor::to_vec(&patch)?),
                summary: None,
                manifest_base: None,
                description: Some("test proposal".into()),
            })?),
            submitted_at_ns: 0,
        },
    ))?;

    let frames = runtime.process_partition(partition)?;
    assert_eq!(frames.len(), 1);
    assert_eq!(
        runtime
            .world_host(universe, world)
            .expect("registered world")
            .kernel()
            .governance()
            .proposals()
            .len(),
        1
    );
    assert!(runtime.rejected_submissions().is_empty());
    Ok(())
}
