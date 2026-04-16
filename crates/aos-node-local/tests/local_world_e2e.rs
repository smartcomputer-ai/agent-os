mod common;
mod support;

use aos_cbor::to_canonical_cbor;
use aos_node::{
    CborPayload, CreateWorldRequest, CreateWorldSource, DomainEventIngress,
    ForkPendingEffectPolicy, ForkWorldRequest, ImportedSeedSource, SeedKind, SnapshotSelector,
    WorldSeed,
};
use aos_node_local::{LocalControl, LocalStatePaths};

use common::{world, world2, world3};

#[test]
fn local_control_runs_a_real_world_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let control = LocalControl::open_batch(paths.root())?;
    let created = support::create_simple_world(&control, &paths, world(), 123)?;
    assert_eq!(created.record.world_id, world());

    let seq = control.enqueue_event(
        world(),
        DomainEventIngress {
            schema: support::fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::fixtures::start_event(
                "e2e-1",
            ))?),
            key: None,
            correlation_id: Some("corr-e2e-1".into()),
        },
    )?;
    assert_eq!(seq.to_string(), "0000000000000000");

    let runtime = control.runtime(world())?;
    assert!(!runtime.has_pending_inbox);
    assert!(!runtime.has_pending_effects);
    assert_eq!(runtime.world_id, world());

    let workers = control.workers(10)?;
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, LocalControl::WORKER_ID);

    let worker_worlds = control.worker_worlds(LocalControl::WORKER_ID, 10)?;
    assert_eq!(worker_worlds.len(), 1);
    assert_eq!(worker_worlds[0].world_id, world());
    assert_eq!(worker_worlds[0].universe_id, aos_node::UniverseId::nil());

    let head = control.journal_head(world())?;
    assert!(head.journal_head > 0);
    let manifest = control.manifest(world())?;
    assert_eq!(head.manifest_hash, Some(manifest.manifest_hash.clone()));

    let journal = control.journal_entries(world(), 0, 128)?;
    assert_eq!(head.retained_from, journal.retained_from);
    assert_eq!(journal.from, journal.retained_from);
    assert!(
        journal
            .entries
            .iter()
            .all(|entry| entry.seq >= journal.retained_from)
    );

    let raw_journal = control.journal_entries_raw(world(), 0, 128)?;
    assert_eq!(raw_journal.from, raw_journal.retained_from);
    assert_eq!(raw_journal.retained_from, journal.retained_from);
    assert!(
        raw_journal
            .entries
            .iter()
            .all(|entry| entry.seq >= raw_journal.retained_from)
    );

    let state = control.state_get(world(), "com.acme/Simple@1", None, None)?;
    assert_eq!(state.workflow, "com.acme/Simple@1");
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    assert_eq!(
        state.cell.as_ref().map(|cell| cell.journal_head),
        Some(head.journal_head)
    );

    Ok(())
}

#[test]
fn local_control_supports_seeded_create_and_world_forking() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempfile::tempdir()?;
    let paths = LocalStatePaths::new(temp.path().join(".aos"));
    let control = LocalControl::open_batch(paths.root())?;
    let created = support::create_simple_world(&control, &paths, world(), 10)?;

    let seeded = control.create_world(CreateWorldRequest {
        world_id: Some(world2()),
        universe_id: aos_node::UniverseId::nil(),
        created_at_ns: 11,
        source: CreateWorldSource::Seed {
            seed: WorldSeed {
                baseline: created.record.active_baseline.clone(),
                seed_kind: SeedKind::Import,
                imported_from: Some(ImportedSeedSource {
                    source: "local-test".into(),
                    external_world_id: Some(world().to_string()),
                    external_snapshot_ref: Some(
                        created.record.active_baseline.snapshot_ref.clone(),
                    ),
                }),
            },
        },
    })?;
    assert_eq!(
        seeded.record.active_baseline,
        created.record.active_baseline
    );
    let forked = control.fork_world(ForkWorldRequest {
        src_world_id: world(),
        src_snapshot: SnapshotSelector::ActiveBaseline,
        new_world_id: Some(world3()),
        forked_at_ns: 12,
        pending_effect_policy: ForkPendingEffectPolicy::ClearAllPendingExternalState,
    })?;
    assert_eq!(
        forked.record.active_baseline,
        created.record.active_baseline
    );
    let seq = control.enqueue_event(
        world3(),
        DomainEventIngress {
            schema: support::fixtures::START_SCHEMA.into(),
            value: CborPayload::inline(to_canonical_cbor(&support::fixtures::start_event(
                "forked-1",
            ))?),
            key: None,
            correlation_id: Some("corr-forked-1".into()),
        },
    )?;
    assert_eq!(seq.to_string(), "0000000000000000");

    let state = control.state_get(world3(), "com.acme/Simple@1", None, None)?;
    assert_eq!(state.state_b64.as_deref(), Some("qg=="));
    Ok(())
}
