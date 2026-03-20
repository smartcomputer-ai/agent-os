use super::*;
use crate::{
    CborPayload, CellStateProjectionDelete, CellStateProjectionRecord, CreateWorldSeedRequest,
    ForkWorldRequest, HeadProjectionRecord, InboxItem, PersistConflict, PersistCorruption,
    PersistenceConfig, QueryProjectionDelta, QueryProjectionMaterialization, SeedKind,
    SegmentExportRequest, SnapshotCommitRequest, SnapshotRecord, SnapshotSelector, WorkerHeartbeat,
    WorkflowCellStateProjection, WorkspaceProjectionDelete, WorkspaceVersionProjectionRecord,
    WorldAdminLifecycle, WorldAdminStatus, WorldAdminStore, WorldLineage, WorldSeed, WorldStore,
};
use aos_effects::ReceiptStatus;
use uuid::Uuid;

fn universe() -> UniverseId {
    UniverseId::from(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap())
}

fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

fn second_world() -> WorldId {
    WorldId::from(Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap())
}

fn timer_ingress(seed: u8) -> InboxItem {
    InboxItem::TimerFired(crate::TimerFiredIngress {
        timer_id: format!("timer-{seed}"),
        payload: CborPayload::inline(vec![seed]),
        correlation_id: None,
    })
}

fn control_ingress(bytes: &[u8]) -> InboxItem {
    InboxItem::Control(crate::CommandIngress {
        command_id: "cmd-memory".into(),
        command: "event-send".into(),
        actor: None,
        payload: CborPayload::inline(bytes.to_vec()),
        submitted_at_ns: 0,
    })
}

fn queued_command(command_id: &str, command: &str, submitted_at_ns: u64) -> CommandRecord {
    CommandRecord {
        command_id: command_id.into(),
        command: command.into(),
        status: crate::CommandStatus::Queued,
        submitted_at_ns,
        started_at_ns: None,
        finished_at_ns: None,
        journal_height: None,
        manifest_hash: None,
        result_payload: None,
        error: None,
    }
}

fn snapshot(height: JournalHeight, snapshot_ref: &str) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: snapshot_ref.to_string(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some("sha256:manifest".into()),
    }
}

fn snapshot_with_state(
    persistence: &MemoryWorldPersistence,
    height: JournalHeight,
    workflow: &str,
    key: Option<Vec<u8>>,
    state: &[u8],
) -> SnapshotRecord {
    let manifest_hash = persistence
        .cas_put_verified(universe(), b"manifest-with-state")
        .expect("store manifest hash");
    let snapshot_bytes = serde_cbor::to_vec(&aos_kernel::snapshot::KernelSnapshot::new(
        height,
        vec![aos_kernel::snapshot::WorkflowStateEntry {
            workflow: workflow.into(),
            key,
            state: state.to_vec(),
            state_hash: *Hash::of_bytes(state).as_bytes(),
            last_active_ns: 777,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        Some(*manifest_hash.as_bytes()),
    ))
    .expect("encode kernel snapshot with state");
    let snapshot_hash = persistence
        .cas_put_verified(universe(), &snapshot_bytes)
        .expect("store snapshot hash");
    SnapshotRecord {
        snapshot_ref: snapshot_hash.to_hex(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some(manifest_hash.to_hex()),
    }
}

fn snapshot_with_workspace_history(
    persistence: &MemoryWorldPersistence,
    height: JournalHeight,
    workspace: &str,
    latest: u64,
    versions: &[(u64, &str, &str, u64)],
) -> SnapshotRecord {
    #[derive(serde::Serialize)]
    struct WorkspaceCommitMeta<'a> {
        root_hash: &'a str,
        owner: &'a str,
        created_at: u64,
    }
    #[derive(serde::Serialize)]
    struct WorkspaceHistory<'a> {
        latest: u64,
        versions: BTreeMap<u64, WorkspaceCommitMeta<'a>>,
    }

    let manifest_hash = persistence
        .cas_put_verified(universe(), b"manifest-with-workspace")
        .expect("store workspace manifest hash");
    let versions_map = versions
        .iter()
        .map(|(version, root_hash, owner, created_at)| {
            (
                *version,
                WorkspaceCommitMeta {
                    root_hash,
                    owner,
                    created_at: *created_at,
                },
            )
        })
        .collect();
    let history_bytes = serde_cbor::to_vec(&WorkspaceHistory {
        latest,
        versions: versions_map,
    })
    .expect("encode workspace history");
    let snapshot_bytes = serde_cbor::to_vec(&aos_kernel::snapshot::KernelSnapshot::new(
        height,
        vec![aos_kernel::snapshot::WorkflowStateEntry {
            workflow: "sys/Workspace@1".into(),
            key: Some(serde_cbor::to_vec(&workspace.to_string()).expect("encode workspace key")),
            state: history_bytes.clone(),
            state_hash: *Hash::of_bytes(&history_bytes).as_bytes(),
            last_active_ns: 888,
        }],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        Some(*manifest_hash.as_bytes()),
    ))
    .expect("encode workspace snapshot");
    let snapshot_hash = persistence
        .cas_put_verified(universe(), &snapshot_bytes)
        .expect("store workspace snapshot hash");
    SnapshotRecord {
        snapshot_ref: snapshot_hash.to_hex(),
        height,
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height),
        manifest_hash: Some(manifest_hash.to_hex()),
    }
}

fn seed_request(
    persistence: &MemoryWorldPersistence,
    world_id: WorldId,
    height: JournalHeight,
) -> CreateWorldSeedRequest {
    let snapshot_bytes = serde_cbor::to_vec(&aos_kernel::snapshot::KernelSnapshot::new(
        height,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        height * 10,
        None,
    ))
    .expect("encode kernel snapshot");
    let manifest_hash = persistence
        .cas_put_verified(universe(), b"manifest")
        .expect("store manifest hash");
    let snapshot_hash = persistence
        .cas_put_verified(universe(), &snapshot_bytes)
        .expect("store snapshot hash");
    CreateWorldSeedRequest {
        world_id: Some(world_id),
        handle: None,
        seed: WorldSeed {
            baseline: SnapshotRecord {
                snapshot_ref: snapshot_hash.to_hex(),
                height,
                logical_time_ns: height * 10,
                receipt_horizon_height: Some(height),
                manifest_hash: Some(manifest_hash.to_hex()),
            },
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: Some("gpu".into()),
        created_at_ns: 123,
    }
}

#[test]
fn world_create_from_seed_materializes_head_and_cell_projections_from_snapshot() {
    let persistence = MemoryWorldPersistence::new();
    let baseline = snapshot_with_state(&persistence, 5, "com.acme/Simple@1", None, b"hello-state");
    let result = persistence
        .world_create_from_seed(
            universe(),
            CreateWorldSeedRequest {
                world_id: Some(world()),
                handle: None,
                seed: WorldSeed {
                    baseline: baseline.clone(),
                    seed_kind: SeedKind::Genesis,
                    imported_from: None,
                },
                placement_pin: None,
                created_at_ns: 321,
            },
        )
        .unwrap();

    assert_eq!(result.record.journal_head, 6);

    let head = persistence
        .head_projection(universe(), world())
        .unwrap()
        .unwrap();
    assert_eq!(head.journal_head, 5);
    assert_eq!(head.manifest_hash, baseline.manifest_hash.unwrap());

    let mono_key_hash = Hash::of_bytes(b"").as_bytes().to_vec();
    let cell = persistence
        .cell_state_projection(universe(), world(), "com.acme/Simple@1", &mono_key_hash)
        .unwrap()
        .unwrap();
    assert_eq!(cell.journal_head, 5);
    assert_eq!(cell.workflow, "com.acme/Simple@1");
    assert_eq!(cell.key_bytes, Vec::<u8>::new());
    assert_eq!(cell.state_hash, Hash::of_bytes(b"hello-state").to_hex());
    assert_eq!(cell.size, 11);
    assert_eq!(cell.last_active_ns, 777);
}

#[test]
fn world_create_from_seed_materializes_workspace_version_bindings() {
    let persistence = MemoryWorldPersistence::new();
    let root_v1 = Hash::of_bytes(b"workspace-root-v1").to_hex();
    let root_v2 = Hash::of_bytes(b"workspace-root-v2").to_hex();
    let baseline = snapshot_with_workspace_history(
        &persistence,
        7,
        "shell",
        2,
        &[(1, &root_v1, "alice", 111), (2, &root_v2, "bob", 222)],
    );

    persistence
        .world_create_from_seed(
            universe(),
            CreateWorldSeedRequest {
                world_id: Some(world()),
                handle: None,
                seed: WorldSeed {
                    baseline,
                    seed_kind: SeedKind::Genesis,
                    imported_from: None,
                },
                placement_pin: None,
                created_at_ns: 321,
            },
        )
        .unwrap();

    let workspace = persistence
        .workspace_projection(universe(), world(), "shell")
        .unwrap()
        .unwrap();
    assert_eq!(workspace.journal_head, 7);
    assert_eq!(workspace.workspace, "shell");
    assert_eq!(workspace.latest_version, 2);
    assert_eq!(workspace.versions.len(), 2);
    assert_eq!(workspace.versions.get(&1).unwrap().root_hash, root_v1);
    assert_eq!(workspace.versions.get(&2).unwrap().root_hash, root_v2);
    assert_eq!(workspace.versions.get(&2).unwrap().owner, "bob");
}

#[test]
fn guarded_projection_materialization_replaces_previous_cell_rows() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, world(), 0))
        .unwrap();
    let lease = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 100, 1_000)
        .unwrap();

    persistence
        .materialize_query_projections_guarded(
            universe(),
            world(),
            &lease,
            200,
            QueryProjectionMaterialization {
                head: HeadProjectionRecord {
                    journal_head: 2,
                    manifest_hash: Hash::of_bytes(b"manifest-a").to_hex(),
                    updated_at_ns: 200,
                },
                workflows: vec![WorkflowCellStateProjection {
                    workflow: "com.acme/Simple@1".into(),
                    cells: vec![CellStateProjectionRecord {
                        journal_head: 2,
                        workflow: "com.acme/Simple@1".into(),
                        key_hash: Hash::of_bytes(b"").as_bytes().to_vec(),
                        key_bytes: Vec::new(),
                        state_hash: Hash::of_bytes(b"state-a").to_hex(),
                        size: 7,
                        last_active_ns: 200,
                    }],
                }],
                workspaces: Vec::new(),
            },
        )
        .unwrap();

    let first = persistence
        .list_cell_state_projections(universe(), world(), "com.acme/Simple@1", None, 10)
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].state_hash, Hash::of_bytes(b"state-a").to_hex());

    persistence
        .materialize_query_projections_guarded(
            universe(),
            world(),
            &lease,
            300,
            QueryProjectionMaterialization {
                head: HeadProjectionRecord {
                    journal_head: 3,
                    manifest_hash: Hash::of_bytes(b"manifest-b").to_hex(),
                    updated_at_ns: 300,
                },
                workflows: Vec::new(),
                workspaces: Vec::new(),
            },
        )
        .unwrap();

    let head = persistence
        .head_projection(universe(), world())
        .unwrap()
        .unwrap();
    assert_eq!(head.journal_head, 3);
    assert_eq!(head.manifest_hash, Hash::of_bytes(b"manifest-b").to_hex());
    assert!(
        persistence
            .list_cell_state_projections(universe(), world(), "com.acme/Simple@1", None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn guarded_projection_delta_updates_only_touched_rows() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, world(), 0))
        .unwrap();
    let lease = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 100, 1_000)
        .unwrap();
    let alpha_key = serde_cbor::to_vec(&"alpha").unwrap();
    let beta_key = serde_cbor::to_vec(&"beta").unwrap();

    persistence
        .apply_query_projection_delta_guarded(
            universe(),
            world(),
            &lease,
            200,
            QueryProjectionDelta {
                head: HeadProjectionRecord {
                    journal_head: 2,
                    manifest_hash: Hash::of_bytes(b"manifest-a").to_hex(),
                    updated_at_ns: 200,
                },
                cell_upserts: vec![
                    CellStateProjectionRecord {
                        journal_head: 2,
                        workflow: "com.acme/Keyed@1".into(),
                        key_hash: Hash::of_bytes(&alpha_key).as_bytes().to_vec(),
                        key_bytes: alpha_key.clone(),
                        state_hash: Hash::of_bytes(b"state-alpha").to_hex(),
                        size: 11,
                        last_active_ns: 200,
                    },
                    CellStateProjectionRecord {
                        journal_head: 2,
                        workflow: "com.acme/Keyed@1".into(),
                        key_hash: Hash::of_bytes(&beta_key).as_bytes().to_vec(),
                        key_bytes: beta_key.clone(),
                        state_hash: Hash::of_bytes(b"state-beta").to_hex(),
                        size: 10,
                        last_active_ns: 200,
                    },
                ],
                cell_deletes: Vec::new(),
                workspace_upserts: Vec::new(),
                workspace_deletes: Vec::new(),
            },
        )
        .unwrap();

    persistence
        .apply_query_projection_delta_guarded(
            universe(),
            world(),
            &lease,
            300,
            QueryProjectionDelta {
                head: HeadProjectionRecord {
                    journal_head: 3,
                    manifest_hash: Hash::of_bytes(b"manifest-b").to_hex(),
                    updated_at_ns: 300,
                },
                cell_upserts: vec![CellStateProjectionRecord {
                    journal_head: 3,
                    workflow: "com.acme/Keyed@1".into(),
                    key_hash: Hash::of_bytes(&alpha_key).as_bytes().to_vec(),
                    key_bytes: alpha_key.clone(),
                    state_hash: Hash::of_bytes(b"state-alpha-v2").to_hex(),
                    size: 14,
                    last_active_ns: 300,
                }],
                cell_deletes: Vec::new(),
                workspace_upserts: vec![WorkspaceRegistryProjectionRecord {
                    journal_head: 3,
                    workspace: "shell".into(),
                    latest_version: 2,
                    versions: BTreeMap::from([
                        (
                            1,
                            WorkspaceVersionProjectionRecord {
                                root_hash: Hash::of_bytes(b"root-v1").to_hex(),
                                owner: "alice".into(),
                                created_at_ns: 111,
                            },
                        ),
                        (
                            2,
                            WorkspaceVersionProjectionRecord {
                                root_hash: Hash::of_bytes(b"root-v2").to_hex(),
                                owner: "bob".into(),
                                created_at_ns: 222,
                            },
                        ),
                    ]),
                    updated_at_ns: 300,
                }],
                workspace_deletes: Vec::new(),
            },
        )
        .unwrap();

    let alpha = persistence
        .cell_state_projection(
            universe(),
            world(),
            "com.acme/Keyed@1",
            Hash::of_bytes(&alpha_key).as_bytes(),
        )
        .unwrap()
        .unwrap();
    let beta = persistence
        .cell_state_projection(
            universe(),
            world(),
            "com.acme/Keyed@1",
            Hash::of_bytes(&beta_key).as_bytes(),
        )
        .unwrap()
        .unwrap();
    let workspace = persistence
        .workspace_projection(universe(), world(), "shell")
        .unwrap()
        .unwrap();

    assert_eq!(alpha.journal_head, 3);
    assert_eq!(alpha.state_hash, Hash::of_bytes(b"state-alpha-v2").to_hex());
    assert_eq!(beta.journal_head, 2);
    assert_eq!(beta.state_hash, Hash::of_bytes(b"state-beta").to_hex());
    assert_eq!(workspace.journal_head, 3);
    assert_eq!(workspace.latest_version, 2);

    persistence
        .apply_query_projection_delta_guarded(
            universe(),
            world(),
            &lease,
            400,
            QueryProjectionDelta {
                head: HeadProjectionRecord {
                    journal_head: 4,
                    manifest_hash: Hash::of_bytes(b"manifest-c").to_hex(),
                    updated_at_ns: 400,
                },
                cell_upserts: Vec::new(),
                cell_deletes: vec![CellStateProjectionDelete {
                    workflow: "com.acme/Keyed@1".into(),
                    key_hash: Hash::of_bytes(&beta_key).as_bytes().to_vec(),
                }],
                workspace_upserts: Vec::new(),
                workspace_deletes: vec![WorkspaceProjectionDelete {
                    workspace: "shell".into(),
                }],
            },
        )
        .unwrap();

    assert!(
        persistence
            .cell_state_projection(
                universe(),
                world(),
                "com.acme/Keyed@1",
                Hash::of_bytes(&beta_key).as_bytes(),
            )
            .unwrap()
            .is_none()
    );
    assert!(
        persistence
            .workspace_projection(universe(), world(), "shell")
            .unwrap()
            .is_none()
    );
}

#[test]
fn cas_put_verified_is_idempotent_and_hash_correct() {
    let persistence = MemoryWorldPersistence::new();
    let bytes = b"hello persistence";

    let first = persistence.cas_put_verified(universe(), bytes).unwrap();
    let second = persistence.cas_put_verified(universe(), bytes).unwrap();

    assert_eq!(first, Hash::of_bytes(bytes));
    assert_eq!(first, second);
    assert!(persistence.cas_has(universe(), first).unwrap());
    assert_eq!(persistence.cas_get(universe(), first).unwrap(), bytes);
}

#[test]
fn cas_stat_reports_direct_chunked_layout() {
    let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
        cas: crate::CasConfig {
            verify_reads: true,
            ..crate::CasConfig::default()
        },
        ..PersistenceConfig::default()
    });
    let bytes = b"definitely larger than four bytes";
    let hash = persistence.cas_put_verified(universe(), bytes).unwrap();

    let meta = persistence.cas.stat(universe(), hash).unwrap();
    assert_eq!(meta.layout_kind, crate::CasLayoutKind::Direct);
    assert_eq!(meta.size_bytes, bytes.len() as u64);
    assert_eq!(meta.chunk_size, 64 * 1024);
    assert_eq!(meta.chunk_count, 1);
    assert_eq!(persistence.cas_get(universe(), hash).unwrap(), bytes);
}

#[test]
fn cas_read_detects_chunk_corruption() {
    let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
        cas: crate::CasConfig {
            verify_reads: true,
            ..crate::CasConfig::default()
        },
        ..PersistenceConfig::default()
    });
    let bytes = vec![7u8; 80_000];
    let hash = persistence.cas_put_verified(universe(), &bytes).unwrap();

    persistence
        .cas()
        .debug_replace_chunk(universe(), hash, 0, b"tampered".to_vec());

    let err = persistence.cas_get(universe(), hash).unwrap_err();
    assert!(matches!(
        err,
        PersistError::Corrupt(PersistCorruption::CasSizeMismatch { .. })
    ));
}

#[test]
fn journal_append_batch_conflicts_on_head_mismatch() {
    let persistence = MemoryWorldPersistence::new();
    let first = persistence
        .journal_append_batch(universe(), world(), 0, &[b"a".to_vec(), b"b".to_vec()])
        .unwrap();
    assert_eq!(first, 0);
    let err = persistence
        .journal_append_batch(universe(), world(), 0, &[b"c".to_vec()])
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::HeadAdvanced {
            expected: 0,
            actual: 2
        })
    ));
    assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
}

#[test]
fn journal_read_range_detects_missing_entry_in_requested_window() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .journal_append_batch(
            universe(),
            world(),
            0,
            &[b"one".to_vec(), b"two".to_vec(), b"three".to_vec()],
        )
        .unwrap();
    persistence.debug_remove_journal_entry(universe(), world(), 1);

    let err = persistence
        .journal_read_range(universe(), world(), 0, 3)
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Corrupt(PersistCorruption::MissingJournalEntry { height: 1 })
    ));
}

#[test]
fn inbox_cursor_is_monotonic_and_compare_and_swap() {
    let persistence = MemoryWorldPersistence::new();
    let first = persistence
        .inbox_enqueue(universe(), world(), timer_ingress(1))
        .unwrap();
    let second = persistence
        .inbox_enqueue(universe(), world(), timer_ingress(2))
        .unwrap();

    persistence
        .inbox_commit_cursor(universe(), world(), None, first.clone())
        .unwrap();

    let err = persistence
        .inbox_commit_cursor(universe(), world(), None, second.clone())
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::InboxCursorAdvanced { .. })
    ));

    let err = persistence
        .inbox_commit_cursor(universe(), world(), Some(first.clone()), first.clone())
        .unwrap();
    assert_eq!(err, ());

    let err = persistence
        .inbox_commit_cursor(
            universe(),
            world(),
            Some(first.clone()),
            InboxSeq::from_u64(0),
        )
        .unwrap();
    assert_eq!(err, ());

    persistence
        .inbox_commit_cursor(universe(), world(), Some(first), second.clone())
        .unwrap();
    assert_eq!(
        persistence.inbox_cursor(universe(), world()).unwrap(),
        Some(second)
    );
}

#[test]
fn inbox_enqueue_externalizes_large_payloads_to_cas() {
    let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
        inbox: crate::InboxConfig {
            inline_payload_threshold_bytes: 4,
        },
        ..PersistenceConfig::default()
    });
    let payload = b"payload larger than threshold".to_vec();
    let seq = persistence
        .inbox_enqueue(universe(), world(), control_ingress(&payload))
        .unwrap();

    let stored = persistence
        .inbox_read_after(universe(), world(), None, 1)
        .unwrap()
        .into_iter()
        .find(|(item_seq, _)| *item_seq == seq)
        .map(|(_, item)| item)
        .unwrap();

    match stored {
        InboxItem::Control(control) => {
            assert_eq!(control.payload.inline_cbor, None);
            let hash = Hash::from_hex_str(control.payload.cbor_ref.as_deref().unwrap()).unwrap();
            assert_eq!(
                control.payload.cbor_sha256.as_deref(),
                Some(hash.to_hex().as_str())
            );
            assert_eq!(control.payload.cbor_size, Some(payload.len() as u64));
            assert_eq!(persistence.cas_get(universe(), hash).unwrap(), payload);
        }
        other => panic!("unexpected inbox item: {other:?}"),
    }
}

#[test]
fn submit_command_is_idempotent_and_enqueues_once() {
    let persistence = MemoryWorldPersistence::new();
    let ingress = crate::CommandIngress {
        command_id: "cmd-1".into(),
        command: "world-pause".into(),
        actor: Some("ops".into()),
        payload: CborPayload::inline(vec![1, 2, 3]),
        submitted_at_ns: 11,
    };
    let initial = queued_command("cmd-1", "world-pause", 11);

    let first = persistence
        .submit_command(universe(), world(), ingress.clone(), initial.clone())
        .unwrap();
    let second = persistence
        .submit_command(universe(), world(), ingress, initial.clone())
        .unwrap();

    assert_eq!(first, initial);
    assert_eq!(second, initial);
    assert_eq!(
        persistence
            .command_record(universe(), world(), "cmd-1")
            .unwrap(),
        Some(initial)
    );
    assert_eq!(
        persistence
            .inbox_read_after(universe(), world(), None, 8)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn submit_command_rejects_idempotency_payload_mismatch() {
    let persistence = MemoryWorldPersistence::new();
    let initial = queued_command("cmd-1", "world-pause", 11);
    persistence
        .submit_command(
            universe(),
            world(),
            crate::CommandIngress {
                command_id: "cmd-1".into(),
                command: "world-pause".into(),
                actor: None,
                payload: CborPayload::inline(vec![1]),
                submitted_at_ns: 11,
            },
            initial,
        )
        .unwrap();

    let err = persistence
        .submit_command(
            universe(),
            world(),
            crate::CommandIngress {
                command_id: "cmd-1".into(),
                command: "world-pause".into(),
                actor: None,
                payload: CborPayload::inline(vec![2]),
                submitted_at_ns: 12,
            },
            queued_command("cmd-1", "world-pause", 12),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::CommandRequestMismatch { .. })
    ));
}

#[test]
fn drain_inbox_to_journal_updates_head_and_cursor_atomically() {
    let persistence = MemoryWorldPersistence::new();
    let first = persistence
        .inbox_enqueue(universe(), world(), timer_ingress(1))
        .unwrap();
    let second = persistence
        .inbox_enqueue(universe(), world(), timer_ingress(2))
        .unwrap();

    let first_height = persistence
        .drain_inbox_to_journal(
            universe(),
            world(),
            None,
            second.clone(),
            0,
            &[b"j1".to_vec(), b"j2".to_vec()],
        )
        .unwrap();
    assert_eq!(first_height, 0);
    assert_eq!(
        persistence.inbox_cursor(universe(), world()).unwrap(),
        Some(second)
    );
    assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
    assert_eq!(
        persistence
            .journal_read_range(universe(), world(), 0, 2)
            .unwrap(),
        vec![(0, b"j1".to_vec()), (1, b"j2".to_vec())]
    );

    let err = persistence
        .drain_inbox_to_journal(
            universe(),
            world(),
            Some(first),
            InboxSeq::from_u64(1),
            0,
            &[b"j3".to_vec()],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::InboxCursorAdvanced { .. })
    ));
    assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 2);
}

#[test]
fn snapshot_index_and_baseline_are_monotonic() {
    let persistence = MemoryWorldPersistence::new();
    let s1 = snapshot(2, "sha256:s1");
    let s2 = snapshot(4, "sha256:s2");

    persistence
        .snapshot_index(universe(), world(), s1.clone())
        .unwrap();
    persistence
        .snapshot_promote_baseline(universe(), world(), s1.clone())
        .unwrap();
    persistence
        .snapshot_index(universe(), world(), s2.clone())
        .unwrap();
    persistence
        .snapshot_promote_baseline(universe(), world(), s2.clone())
        .unwrap();

    let err = persistence
        .snapshot_promote_baseline(universe(), world(), s1.clone())
        .unwrap_err();
    assert!(matches!(err, PersistError::Validation(_)));
    assert_eq!(
        persistence
            .snapshot_active_baseline(universe(), world())
            .unwrap(),
        s2
    );
}

#[test]
fn snapshot_commit_appends_journal_and_promotes_baseline_atomically() {
    let persistence = MemoryWorldPersistence::new();
    let snapshot_bytes = b"kernel-snapshot".to_vec();
    let snapshot_hash = Hash::of_bytes(&snapshot_bytes);
    let record = SnapshotRecord {
        snapshot_ref: snapshot_hash.to_hex(),
        height: 0,
        logical_time_ns: 0,
        receipt_horizon_height: Some(0),
        manifest_hash: Some("sha256:manifest".into()),
    };

    let result = persistence
        .snapshot_commit(
            universe(),
            world(),
            SnapshotCommitRequest {
                expected_head: 0,
                snapshot_bytes,
                record: record.clone(),
                snapshot_journal_entry: b"snapshot".to_vec(),
                baseline_journal_entry: Some(b"baseline".to_vec()),
                promote_baseline: true,
            },
        )
        .unwrap();

    assert_eq!(result.snapshot_hash, snapshot_hash);
    assert_eq!(result.first_height, 0);
    assert_eq!(result.next_head, 2);
    assert!(result.baseline_promoted);
    assert_eq!(
        persistence
            .journal_read_range(universe(), world(), 0, 8)
            .unwrap(),
        vec![(0, b"snapshot".to_vec()), (1, b"baseline".to_vec())]
    );
    assert_eq!(
        persistence
            .snapshot_active_baseline(universe(), world())
            .unwrap(),
        record
    );
}

#[test]
fn snapshot_promotion_requires_receipt_horizon_equal_height() {
    let persistence = MemoryWorldPersistence::new();
    let record = SnapshotRecord {
        snapshot_ref: "sha256:snapshot".into(),
        height: 2,
        logical_time_ns: 20,
        receipt_horizon_height: None,
        manifest_hash: Some("sha256:manifest".into()),
    };

    persistence
        .snapshot_index(universe(), world(), record.clone())
        .unwrap();
    let err = persistence
        .snapshot_promote_baseline(universe(), world(), record)
        .unwrap_err();
    assert!(matches!(err, PersistError::Validation(_)));
}

#[test]
fn segment_index_is_immutable_per_end_height() {
    let persistence = MemoryWorldPersistence::new();
    let first = SegmentIndexRecord {
        segment: crate::SegmentId::new(0, 9).unwrap(),
        body_ref: "sha256:first".into(),
        checksum: "sha256:first".into(),
    };
    let replacement = SegmentIndexRecord {
        segment: crate::SegmentId::new(0, 9).unwrap(),
        body_ref: "sha256:second".into(),
        checksum: "sha256:second".into(),
    };

    persistence
        .segment_index_put(universe(), world(), first.clone())
        .unwrap();
    let err = persistence
        .segment_index_put(universe(), world(), replacement)
        .unwrap_err();
    assert!(matches!(err, PersistError::Conflict(_)));
    assert_eq!(
        persistence
            .segment_index_read_from(universe(), world(), 0, 8)
            .unwrap(),
        vec![first]
    );
}

#[test]
fn segment_export_moves_hot_journal_entries_into_segment_object() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .journal_append_batch(
            universe(),
            world(),
            0,
            &[b"j0".to_vec(), b"j1".to_vec(), b"j2".to_vec()],
        )
        .unwrap();
    let baseline = SnapshotRecord {
        snapshot_ref: "sha256:snapshot".into(),
        height: 3,
        logical_time_ns: 30,
        receipt_horizon_height: Some(3),
        manifest_hash: Some("sha256:manifest".into()),
    };
    persistence
        .snapshot_index(universe(), world(), baseline.clone())
        .unwrap();
    persistence
        .snapshot_promote_baseline(universe(), world(), baseline)
        .unwrap();

    let result = persistence
        .segment_export(
            universe(),
            world(),
            SegmentExportRequest {
                segment: crate::SegmentId::new(0, 1).unwrap(),
                hot_tail_margin: 0,
                delete_chunk_entries: 1,
            },
        )
        .unwrap();

    assert_eq!(result.exported_entries, 2);
    assert_eq!(result.deleted_entries, 2);
    assert_eq!(
        persistence
            .segment_read_entries(universe(), world(), crate::SegmentId::new(0, 1).unwrap())
            .unwrap(),
        vec![(0, b"j0".to_vec()), (1, b"j1".to_vec())]
    );
    assert_eq!(
        persistence
            .journal_read_range(universe(), world(), 2, 8)
            .unwrap(),
        vec![(2, b"j2".to_vec())]
    );
    assert_eq!(
        persistence
            .journal_read_range(universe(), world(), 0, 8)
            .unwrap(),
        vec![
            (0, b"j0".to_vec()),
            (1, b"j1".to_vec()),
            (2, b"j2".to_vec()),
        ]
    );
}

#[test]
fn runtime_leases_fence_stale_mutations() {
    let persistence = MemoryWorldPersistence::new();
    let lease = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 10, 50)
        .unwrap();

    persistence
        .journal_append_batch_guarded(universe(), world(), &lease, 20, 0, &[b"entry".to_vec()])
        .unwrap();

    let err = persistence
        .journal_append_batch_guarded(universe(), world(), &lease, 61, 1, &[b"late".to_vec()])
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::LeaseHeld { .. })
    ));
}

#[test]
fn runtime_acquire_after_expiry_increments_epoch() {
    let persistence = MemoryWorldPersistence::new();
    let first = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 100, 10)
        .unwrap();
    let second = persistence
        .acquire_world_lease(universe(), world(), "worker-b", 111, 10)
        .unwrap();

    assert_eq!(first.epoch, 1);
    assert_eq!(second.epoch, 2);
    assert_eq!(second.holder_worker_id, "worker-b");
}

#[test]
fn runtime_lists_active_workers_and_ready_worlds() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, world(), 0))
        .unwrap();
    persistence
        .heartbeat_worker(WorkerHeartbeat {
            worker_id: "worker-a".into(),
            pins: vec!["default".into()],
            last_seen_ns: 10,
            expires_at_ns: 30,
        })
        .unwrap();
    persistence
        .heartbeat_worker(WorkerHeartbeat {
            worker_id: "worker-b".into(),
            pins: vec!["pinned".into()],
            last_seen_ns: 10,
            expires_at_ns: 5,
        })
        .unwrap();
    persistence
        .enqueue_ingress(universe(), world(), timer_ingress(9))
        .unwrap();
    let lease = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 20, 10)
        .unwrap();

    let workers = persistence.list_active_workers(20, 8).unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].worker_id, "worker-a");

    let worlds = persistence
        .list_ready_worlds(20, 8, Some(&[universe()]))
        .unwrap();
    assert_eq!(worlds.len(), 1);
    assert_eq!(worlds[0].universe_id, universe());
    assert_eq!(worlds[0].info.world_id, world());
    assert_eq!(worlds[0].info.notify_counter, 1);
    assert!(worlds[0].info.has_pending_inbox);
    assert_eq!(worlds[0].info.lease, Some(lease.clone()));
    assert_eq!(worlds[0].info.meta.admin.status, WorldAdminStatus::Active);

    let leased = persistence
        .list_worker_worlds("worker-a", 20, 8, Some(&[universe()]))
        .unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].universe_id, universe());
    assert_eq!(leased[0].info.world_id, world());
}

#[test]
fn acquire_world_lease_reclaims_holder_without_live_heartbeat() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, world(), 0))
        .unwrap();
    persistence
        .heartbeat_worker(WorkerHeartbeat {
            worker_id: "worker-a".into(),
            pins: vec!["default".into()],
            last_seen_ns: 10,
            expires_at_ns: 15,
        })
        .unwrap();
    let first = persistence
        .acquire_world_lease(universe(), world(), "worker-a", 10, 100)
        .unwrap();

    let second = persistence
        .acquire_world_lease(universe(), world(), "worker-b", 20, 100)
        .unwrap();

    assert_eq!(first.epoch, 1);
    assert_eq!(second.epoch, 2);
    assert_eq!(second.holder_worker_id, "worker-b");
}

#[test]
fn dedupe_gc_sweeps_expired_terminal_effect_timer_and_portal_records() {
    let persistence = MemoryWorldPersistence::with_config(PersistenceConfig {
        dedupe_gc: crate::DedupeGcConfig {
            effect_retention_ns: 5,
            timer_retention_ns: 5,
            portal_retention_ns: 5,
            bucket_width_ns: 5,
        },
        ..PersistenceConfig::default()
    });
    let now_ns = 100;
    let lease = persistence
        .acquire_world_lease(universe(), world(), "worker-a", now_ns, 100)
        .unwrap();

    let effect = EffectDispatchItem {
        shard: 0,
        universe_id: universe(),
        world_id: world(),
        intent_hash: vec![1; 32],
        effect_kind: "http.request".into(),
        cap_name: "http".into(),
        params_inline_cbor: Some(vec![0xA0]),
        params_ref: None,
        params_size: None,
        params_sha256: None,
        idempotency_key: vec![2; 32],
        origin_name: "test".into(),
        policy_context_hash: None,
        enqueued_at_ns: now_ns,
    };
    persistence
        .publish_effect_dispatches_guarded(universe(), world(), &lease, now_ns, &[effect.clone()])
        .unwrap();
    let claimed = persistence
        .claim_pending_effects_for_world(universe(), world(), "worker-a", now_ns, 100, 8)
        .unwrap();
    persistence
        .ack_effect_dispatch_with_receipt(
            universe(),
            world(),
            "worker-a",
            effect.shard,
            claimed[0].0.clone(),
            now_ns,
            ReceiptIngress {
                intent_hash: effect.intent_hash.clone(),
                effect_kind: effect.effect_kind.clone(),
                adapter_id: "stub".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::inline(vec![1]),
                cost_cents: Some(0),
                signature: vec![0; 64],
                correlation_id: None,
            },
        )
        .unwrap();

    let timer = TimerDueItem {
        shard: 0,
        universe_id: universe(),
        world_id: world(),
        intent_hash: vec![3; 32],
        time_bucket: 0,
        deliver_at_ns: now_ns,
        payload_cbor: vec![0xA0],
        enqueued_at_ns: now_ns,
    };
    persistence
        .publish_due_timers_guarded(universe(), world(), &lease, now_ns, &[timer.clone()])
        .unwrap();
    let claimed_timers = persistence
        .claim_due_timers_for_world(universe(), world(), "worker-a", now_ns, 100, 8)
        .unwrap();
    persistence
        .ack_timer_delivery_with_receipt(
            universe(),
            world(),
            "worker-a",
            &claimed_timers[0].intent_hash,
            now_ns,
            ReceiptIngress {
                intent_hash: timer.intent_hash.clone(),
                effect_kind: "timer.set".into(),
                adapter_id: "timer.set".into(),
                status: ReceiptStatus::Ok,
                payload: CborPayload::inline(vec![2]),
                cost_cents: Some(0),
                signature: vec![0; 64],
                correlation_id: None,
            },
        )
        .unwrap();

    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, second_world(), 0))
        .unwrap();
    persistence
        .enqueue_ingress(universe(), second_world(), timer_ingress(9))
        .unwrap();
    assert_eq!(
        persistence
            .portal_send(
                universe(),
                second_world(),
                now_ns,
                b"portal-message",
                InboxItem::DomainEvent(crate::DomainEventIngress {
                    schema: "com.acme/Event@1".into(),
                    value: CborPayload::inline(vec![9]),
                    key: None,
                    correlation_id: None,
                }),
            )
            .unwrap()
            .status,
        PortalSendStatus::Enqueued
    );

    assert_eq!(
        persistence
            .sweep_effect_dedupe_gc(universe(), now_ns + 10, 8)
            .unwrap(),
        1
    );
    assert_eq!(
        persistence
            .sweep_timer_dedupe_gc(universe(), now_ns + 10, 8)
            .unwrap(),
        1
    );
    assert_eq!(
        persistence
            .sweep_portal_dedupe_gc(universe(), now_ns + 10, 8)
            .unwrap(),
        1
    );

    let guard = persistence.state.lock().unwrap();
    assert!(
        !guard
            .effects_dedupe
            .get(&universe())
            .is_some_and(|records| records.contains_key(&effect.intent_hash))
    );
    assert!(
        !guard
            .timers_dedupe
            .get(&universe())
            .is_some_and(|records| records.contains_key(&timer.intent_hash))
    );
    assert!(
        !guard
            .portal_dedupe
            .get(&(universe(), second_world()))
            .is_some_and(|records| records.contains_key(&b"portal-message"[..]))
    );
}

#[test]
fn runtime_set_world_placement_pin_updates_listed_metadata() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .inbox_enqueue(universe(), world(), timer_ingress(1))
        .unwrap();
    persistence
        .set_world_placement_pin(universe(), world(), Some("gpu".into()))
        .unwrap();

    let worlds = persistence.list_worlds(universe(), 20, None, 8).unwrap();
    assert_eq!(worlds[0].meta.placement_pin.as_deref(), Some("gpu"));
}

#[test]
fn list_worlds_supports_after_cursor() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .inbox_enqueue(universe(), world(), timer_ingress(1))
        .unwrap();
    persistence
        .inbox_enqueue(universe(), second_world(), timer_ingress(2))
        .unwrap();

    let first_page = persistence.list_worlds(universe(), 20, None, 1).unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].world_id, world());

    let second_page = persistence
        .list_worlds(universe(), 20, Some(first_page[0].world_id), 1)
        .unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].world_id, second_world());
}

#[test]
fn runtime_set_world_admin_lifecycle_blocks_direct_ingress_but_keeps_ready_work_visible() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .inbox_enqueue(universe(), world(), timer_ingress(1))
        .unwrap();
    persistence
        .set_world_admin_lifecycle(
            universe(),
            world(),
            WorldAdminLifecycle {
                status: WorldAdminStatus::Paused,
                updated_at_ns: 123,
                operation_id: Some("op-1".into()),
                reason: Some("manual pause".into()),
            },
        )
        .unwrap();

    let worlds = persistence.list_worlds(universe(), 20, None, 8).unwrap();
    assert_eq!(worlds[0].meta.admin.status, WorldAdminStatus::Paused);
    let ready = persistence
        .list_ready_worlds(20, 8, Some(&[universe()]))
        .unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].info.meta.admin.status, WorldAdminStatus::Paused);
    assert!(matches!(
        persistence.enqueue_ingress(universe(), world(), timer_ingress(2)),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
}

#[test]
fn list_universes_supports_after_cursor() {
    let persistence = MemoryWorldPersistence::new();
    let first = universe();
    let second = UniverseId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap());
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(first),
            handle: None,
            created_at_ns: 1,
        })
        .unwrap();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(second),
            handle: None,
            created_at_ns: 2,
        })
        .unwrap();

    let first_page = persistence.list_universes(None, 1).unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(first_page[0].universe_id, first);

    let second_page = persistence.list_universes(Some(first), 1).unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].universe_id, second);
}

#[test]
fn handles_default_from_ids_and_can_be_renamed() {
    let persistence = MemoryWorldPersistence::new();
    let universe_id = universe();
    let world_id = world();
    let created_universe = persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(universe_id),
            handle: None,
            created_at_ns: 1,
        })
        .unwrap();
    assert_eq!(
        created_universe.record.meta.handle,
        default_universe_handle(universe_id)
    );

    let world = persistence
        .world_create_from_seed(universe_id, seed_request(&persistence, world_id, 7))
        .unwrap();
    assert_eq!(world.record.meta.handle, default_world_handle(world_id));

    let renamed_universe = persistence
        .set_universe_handle(universe_id, "prod-core".into())
        .unwrap();
    assert_eq!(renamed_universe.meta.handle, "prod-core");

    persistence
        .set_world_handle(universe_id, world_id, "shell-main".into())
        .unwrap();
    let info = persistence
        .world_runtime_info(universe_id, world_id, 0)
        .unwrap();
    assert_eq!(info.meta.handle, "shell-main");
}

#[test]
fn handle_renames_reject_collisions() {
    let persistence = MemoryWorldPersistence::new();
    let first_universe = universe();
    let second_universe =
        UniverseId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap());
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(first_universe),
            handle: Some("alpha".into()),
            created_at_ns: 1,
        })
        .unwrap();
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(second_universe),
            handle: Some("beta".into()),
            created_at_ns: 2,
        })
        .unwrap();

    let err = persistence
        .set_universe_handle(second_universe, "alpha".into())
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::UniverseHandleExists { .. })
    ));

    let first_world = world();
    let second_world = second_world();
    persistence
        .world_create_from_seed(
            first_universe,
            CreateWorldSeedRequest {
                world_id: Some(first_world),
                handle: Some("ops".into()),
                seed: seed_request(&persistence, first_world, 3).seed,
                placement_pin: None,
                created_at_ns: 10,
            },
        )
        .unwrap();
    persistence
        .world_create_from_seed(
            first_universe,
            CreateWorldSeedRequest {
                world_id: Some(second_world),
                handle: Some("ops-2".into()),
                seed: seed_request(&persistence, second_world, 4).seed,
                placement_pin: None,
                created_at_ns: 11,
            },
        )
        .unwrap();

    let err = persistence
        .set_world_handle(first_universe, second_world, "ops".into())
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::WorldHandleExists { .. })
    ));
}

#[test]
fn deleted_resources_release_handles_and_block_renames() {
    let persistence = MemoryWorldPersistence::new();
    let first_universe = universe();
    let second_universe =
        UniverseId::from(Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap());
    persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(first_universe),
            handle: Some("alpha".into()),
            created_at_ns: 1,
        })
        .unwrap();
    let deleted_universe = persistence.delete_universe(first_universe, 2).unwrap();
    assert_eq!(deleted_universe.meta.handle, "alpha");
    assert!(matches!(
        persistence.get_universe_by_handle("alpha"),
        Err(PersistError::NotFound(_))
    ));
    assert!(matches!(
        persistence.set_universe_handle(first_universe, "beta".into()),
        Err(PersistError::Conflict(
            PersistConflict::UniverseAdminBlocked { .. }
        ))
    ));
    let recreated_universe = persistence
        .create_universe(CreateUniverseRequest {
            universe_id: Some(second_universe),
            handle: Some("alpha".into()),
            created_at_ns: 3,
        })
        .unwrap();
    assert_eq!(recreated_universe.record.meta.handle, "alpha");

    let persistence = MemoryWorldPersistence::new();
    let first_world = world();
    let second_world = second_world();
    persistence
        .world_create_from_seed(
            universe(),
            CreateWorldSeedRequest {
                world_id: Some(first_world),
                handle: Some("ops".into()),
                seed: seed_request(&persistence, first_world, 4).seed,
                placement_pin: None,
                created_at_ns: 4,
            },
        )
        .unwrap();
    persistence
        .set_world_admin_lifecycle(
            universe(),
            first_world,
            WorldAdminLifecycle {
                status: WorldAdminStatus::Deleted,
                updated_at_ns: 5,
                operation_id: Some("delete-op".into()),
                reason: Some("cleanup".into()),
            },
        )
        .unwrap();
    assert!(matches!(
        persistence.world_runtime_info_by_handle(universe(), "ops", 0),
        Err(PersistError::NotFound(_))
    ));
    assert!(matches!(
        persistence.set_world_handle(universe(), first_world, "ops-2".into()),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    let recreated_world = persistence
        .world_create_from_seed(
            universe(),
            CreateWorldSeedRequest {
                world_id: Some(second_world),
                handle: Some("ops".into()),
                seed: seed_request(&persistence, second_world, 5).seed,
                placement_pin: None,
                created_at_ns: 6,
            },
        )
        .unwrap();
    assert_eq!(recreated_world.record.meta.handle, "ops");
}

#[test]
fn archived_world_blocks_mutations_commands_and_leases() {
    let persistence = MemoryWorldPersistence::new();
    persistence
        .world_create_from_seed(universe(), seed_request(&persistence, world(), 0))
        .unwrap();
    persistence
        .set_world_admin_lifecycle(
            universe(),
            world(),
            WorldAdminLifecycle {
                status: WorldAdminStatus::Archived,
                updated_at_ns: 10,
                operation_id: Some("archive-op".into()),
                reason: Some("archive".into()),
            },
        )
        .unwrap();

    assert!(matches!(
        persistence.set_world_placement_pin(universe(), world(), Some("cpu".into())),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        persistence.set_world_handle(universe(), world(), "archived".into()),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        persistence.submit_command(
            universe(),
            world(),
            crate::CommandIngress {
                command_id: "cmd-archived".into(),
                command: "event-send".into(),
                actor: None,
                payload: CborPayload::inline(Vec::new()),
                submitted_at_ns: 10,
            },
            queued_command("cmd-archived", "event-send", 10),
        ),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        persistence.acquire_world_lease(universe(), world(), "worker-1", 10, 100),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
    assert!(matches!(
        persistence.enqueue_ingress(universe(), world(), control_ingress(&[])),
        Err(PersistError::Conflict(
            PersistConflict::WorldAdminBlocked { .. }
        ))
    ));
}

#[test]
fn world_create_from_seed_initializes_restore_roots_and_metadata() {
    let persistence = MemoryWorldPersistence::new();
    let request = seed_request(&persistence, world(), 7);

    let result = persistence
        .world_create_from_seed(universe(), request)
        .expect("create world from seed");

    assert_eq!(result.record.world_id, world());
    assert_eq!(result.record.journal_head, 8);
    assert_eq!(result.record.meta.active_baseline_height, Some(7));
    assert_eq!(result.record.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(result.record.meta.created_at_ns, 123);
    assert_eq!(
        result.record.meta.lineage,
        Some(WorldLineage::Genesis { created_at_ns: 123 })
    );
    assert_eq!(
        persistence
            .snapshot_active_baseline(universe(), world())
            .unwrap(),
        result.record.active_baseline
    );
    assert_eq!(persistence.journal_head(universe(), world()).unwrap(), 8);
}

#[test]
fn world_create_from_seed_requires_snapshot_and_manifest_in_cas() {
    let persistence = MemoryWorldPersistence::new();
    let request = CreateWorldSeedRequest {
        world_id: Some(world()),
        handle: None,
        seed: WorldSeed {
            baseline: SnapshotRecord {
                snapshot_ref: Hash::of_bytes(b"missing-snapshot").to_hex(),
                height: 0,
                logical_time_ns: 0,
                receipt_horizon_height: Some(0),
                manifest_hash: Some(Hash::of_bytes(b"missing-manifest").to_hex()),
            },
            seed_kind: SeedKind::Genesis,
            imported_from: None,
        },
        placement_pin: None,
        created_at_ns: 0,
    };

    let err = persistence
        .world_create_from_seed(universe(), request)
        .expect_err("missing CAS roots should fail");
    assert!(matches!(err, PersistError::NotFound(_)));
}

#[test]
fn world_fork_clones_selected_snapshot_and_records_lineage() {
    let persistence = MemoryWorldPersistence::new();
    let request = seed_request(&persistence, world(), 5);
    persistence
        .world_create_from_seed(universe(), request)
        .expect("create source world");

    let result = persistence
        .world_fork(
            universe(),
            ForkWorldRequest {
                src_world_id: world(),
                src_snapshot: SnapshotSelector::ActiveBaseline,
                new_world_id: Some(second_world()),
                handle: None,
                placement_pin: None,
                forked_at_ns: 456,
                pending_effect_policy: crate::ForkPendingEffectPolicy::default(),
            },
        )
        .expect("fork world");

    assert_eq!(result.record.world_id, second_world());
    assert_eq!(result.record.journal_head, 6);
    assert_eq!(result.record.meta.placement_pin.as_deref(), Some("gpu"));
    assert_eq!(
        result.record.meta.lineage,
        Some(WorldLineage::Fork {
            forked_at_ns: 456,
            src_universe_id: universe(),
            src_world_id: world(),
            src_snapshot_ref: result.record.active_baseline.snapshot_ref.clone(),
            src_height: 5,
        })
    );
}
