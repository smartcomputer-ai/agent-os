use std::str::FromStr;

use aos_cbor::Hash;
use aos_kernel::MemStore;
use aos_kernel::journal::{
    CustomRecord, JournalRecord, ManifestRecord, SnapshotRecord as KernelSnapshotRecord,
};
use aos_node::{CborPayload, LocalStatePaths, SnapshotRecord, UniverseId, WorldId, WorldLogFrame};
use aos_node_hosted::kafka::{
    CellProjectionUpsert, PartitionLogEntry, ProjectionKey, ProjectionRecord, ProjectionValue,
    WorkspaceProjectionUpsert, WorldMetaProjection,
};
use aos_node_hosted::materializer::{
    CellStateProjectionRecord, MaterializedCellRow, Materializer, MaterializerConfig,
    WorkspaceRegistryProjectionRecord, WorkspaceVersionProjectionRecord,
};
use tempfile::tempdir;

fn universe_id() -> UniverseId {
    UniverseId::from_str("00000000-0000-0000-0000-000000000001").expect("valid universe id")
}

fn world_id() -> WorldId {
    WorldId::from_str("00000000-0000-0000-0000-0000000000a1").expect("valid world id")
}

fn baseline(height: u64, manifest_hash: &str) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: format!("sha256:{height:064x}"),
        height,
        universe_id: universe_id(),
        logical_time_ns: height * 100,
        receipt_horizon_height: Some(height),
        manifest_hash: Some(manifest_hash.into()),
    }
}

fn world_meta(token: &str, journal_head: u64, manifest_hash: &str) -> WorldMetaProjection {
    WorldMetaProjection {
        universe_id: universe_id(),
        projection_token: token.into(),
        world_epoch: 1,
        journal_head,
        manifest_hash: manifest_hash.into(),
        active_baseline: baseline(journal_head, manifest_hash),
        updated_at_ns: journal_head * 10,
    }
}

fn workspace_projection(workspace: &str, journal_head: u64) -> WorkspaceRegistryProjectionRecord {
    let mut versions = std::collections::BTreeMap::new();
    versions.insert(
        1,
        WorkspaceVersionProjectionRecord {
            root_hash: "sha256:workspace".into(),
            owner: "lukas".into(),
            created_at_ns: journal_head * 10,
        },
    );
    WorkspaceRegistryProjectionRecord {
        journal_head,
        workspace: workspace.into(),
        latest_version: 1,
        versions,
        updated_at_ns: journal_head * 10,
    }
}

fn cell_projection(workflow: &str, key_bytes: &[u8], journal_head: u64) -> MaterializedCellRow {
    let state_bytes =
        format!("state:{workflow}:{}", String::from_utf8_lossy(key_bytes)).into_bytes();
    let state_hash = Hash::of_bytes(&state_bytes);
    MaterializedCellRow {
        cell: CellStateProjectionRecord {
            journal_head,
            workflow: workflow.into(),
            key_hash: Hash::of_bytes(key_bytes).as_bytes().to_vec(),
            key_bytes: key_bytes.to_vec(),
            state_hash: state_hash.to_hex(),
            size: state_bytes.len() as u64,
            last_active_ns: journal_head * 100,
        },
        state_payload: CborPayload::externalized(state_hash, state_bytes.len() as u64),
    }
}

fn journal_frame(offset: u64, manifest_hash: &str) -> PartitionLogEntry {
    PartitionLogEntry {
        offset,
        frame: WorldLogFrame {
            format_version: 1,
            universe_id: universe_id(),
            world_id: world_id(),
            world_epoch: 1,
            world_seq_start: 10,
            world_seq_end: 11,
            records: vec![
                JournalRecord::Manifest(ManifestRecord {
                    manifest_hash: manifest_hash.into(),
                }),
                JournalRecord::Snapshot(KernelSnapshotRecord {
                    snapshot_ref: "sha256:snap".into(),
                    height: 11,
                    universe_id: universe_id().as_uuid(),
                    logical_time_ns: 1100,
                    receipt_horizon_height: Some(11),
                    manifest_hash: Some(manifest_hash.into()),
                }),
                JournalRecord::Custom(CustomRecord {
                    tag: "demo".into(),
                    data: vec![1, 2, 3],
                }),
            ],
        },
    }
}

#[test]
fn materializer_applies_projection_records_and_clears_old_token_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut materializer = Materializer::<MemStore>::from_config(MaterializerConfig::from_paths(
        &paths,
        "aos-journal",
    ))?;

    materializer.apply_projection_record(
        0,
        0,
        &ProjectionRecord {
            key: ProjectionKey::WorldMeta {
                world_id: world_id(),
            },
            value: Some(ProjectionValue::WorldMeta(world_meta(
                "tok-1",
                12,
                "sha256:feed",
            ))),
        },
    )?;
    materializer.apply_projection_record(
        0,
        1,
        &ProjectionRecord {
            key: ProjectionKey::Workspace {
                world_id: world_id(),
                workspace: "alpha".into(),
            },
            value: Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                projection_token: "tok-1".into(),
                record: workspace_projection("alpha", 12),
            })),
        },
    )?;
    let cell = cell_projection("demo/Counter@1", b"a", 12);
    materializer.apply_projection_record(
        0,
        2,
        &ProjectionRecord {
            key: ProjectionKey::Cell {
                world_id: world_id(),
                workflow: "demo/Counter@1".into(),
                key_hash: cell.cell.key_hash.clone(),
            },
            value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                projection_token: "tok-1".into(),
                record: cell.cell.clone(),
                state_payload: cell.state_payload.clone(),
            })),
        },
    )?;

    assert_eq!(
        materializer
            .sqlite()
            .load_workspace_projection(universe_id(), world_id(), "alpha")?
            .expect("workspace")
            .workspace,
        "alpha"
    );
    assert_eq!(
        materializer.sqlite().load_cell_projection(
            universe_id(),
            world_id(),
            "demo/Counter@1",
            b"a"
        )?,
        Some(cell.clone())
    );

    materializer.apply_projection_record(
        0,
        3,
        &ProjectionRecord {
            key: ProjectionKey::WorldMeta {
                world_id: world_id(),
            },
            value: Some(ProjectionValue::WorldMeta(world_meta(
                "tok-2",
                20,
                "sha256:bead",
            ))),
        },
    )?;
    materializer.apply_projection_record(
        0,
        4,
        &ProjectionRecord {
            key: ProjectionKey::Cell {
                world_id: world_id(),
                workflow: "demo/Counter@1".into(),
                key_hash: cell.cell.key_hash.clone(),
            },
            value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                projection_token: "tok-1".into(),
                record: cell.cell.clone(),
                state_payload: cell.state_payload.clone(),
            })),
        },
    )?;
    materializer.apply_projection_record(
        0,
        5,
        &ProjectionRecord {
            key: ProjectionKey::Workspace {
                world_id: world_id(),
                workspace: "beta".into(),
            },
            value: Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                projection_token: "tok-2".into(),
                record: workspace_projection("beta", 20),
            })),
        },
    )?;

    assert_eq!(
        materializer.sqlite().load_projection_token(world_id())?,
        Some("tok-2".into())
    );
    assert_eq!(
        materializer
            .sqlite()
            .load_workspace_projection(universe_id(), world_id(), "alpha")?,
        None
    );
    assert_eq!(
        materializer
            .sqlite()
            .load_workspace_projection(universe_id(), world_id(), "beta")?
            .expect("beta")
            .workspace,
        "beta"
    );
    assert_eq!(
        materializer.sqlite().load_cell_projection(
            universe_id(),
            world_id(),
            "demo/Counter@1",
            b"a"
        )?,
        None
    );
    assert_eq!(
        materializer.load_source_offset("aos-projection", 0)?,
        Some(5)
    );

    Ok(())
}

#[test]
fn materializer_indexes_journal_frames_and_dedupes_offsets()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut config = MaterializerConfig::from_paths(&paths, "aos-journal");
    config.retained_journal_entries_per_world = Some(16);
    let mut materializer = Materializer::<MemStore>::from_config(config)?;
    let entries = vec![journal_frame(7, "sha256:feed")];

    let first = materializer.materialize_partition(0, &entries)?;
    assert_eq!(first.processed_entries, 1);
    assert_eq!(first.journal_entries_indexed, 3);
    assert_eq!(first.last_offset, Some(7));

    let second = materializer.materialize_partition(0, &entries)?;
    assert_eq!(second.processed_entries, 0);
    assert_eq!(second.journal_entries_indexed, 0);
    assert_eq!(second.last_offset, Some(7));

    let journal = materializer
        .sqlite()
        .load_journal_entries(universe_id(), world_id(), 0, 10)?
        .expect("journal rows");
    assert_eq!(journal.entries.len(), 3);
    assert_eq!(journal.entries[0].seq, 10);
    assert_eq!(journal.entries[1].kind, "snapshot");
    assert_eq!(journal.entries[2].kind, "custom");

    let head = materializer
        .sqlite()
        .load_journal_head(universe_id(), world_id())?
        .expect("journal head");
    assert_eq!(head.journal_head, 11);
    assert_eq!(head.manifest_hash.as_deref(), Some("sha256:feed"));
    assert_eq!(materializer.load_source_offset("aos-journal", 0)?, Some(7));

    Ok(())
}

#[test]
fn materializer_bootstrap_projection_partition_uses_latest_retained_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut materializer = Materializer::<MemStore>::from_config(MaterializerConfig::from_paths(
        &paths,
        "aos-journal",
    ))?;

    let tok1_cell = cell_projection("demo/Counter@1", b"a", 12);
    let tok2_cell = cell_projection("demo/Counter@1", b"b", 20);
    let entries = vec![
        (
            0,
            ProjectionRecord {
                key: ProjectionKey::WorldMeta {
                    world_id: world_id(),
                },
                value: Some(ProjectionValue::WorldMeta(world_meta(
                    "tok-1",
                    12,
                    "sha256:feed",
                ))),
            },
        ),
        (
            1,
            ProjectionRecord {
                key: ProjectionKey::Workspace {
                    world_id: world_id(),
                    workspace: "alpha".into(),
                },
                value: Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                    projection_token: "tok-1".into(),
                    record: workspace_projection("alpha", 12),
                })),
            },
        ),
        (
            2,
            ProjectionRecord {
                key: ProjectionKey::Workspace {
                    world_id: world_id(),
                    workspace: "alpha".into(),
                },
                value: None,
            },
        ),
        (
            3,
            ProjectionRecord {
                key: ProjectionKey::WorldMeta {
                    world_id: world_id(),
                },
                value: Some(ProjectionValue::WorldMeta(world_meta(
                    "tok-2",
                    20,
                    "sha256:bead",
                ))),
            },
        ),
        (
            4,
            ProjectionRecord {
                key: ProjectionKey::Workspace {
                    world_id: world_id(),
                    workspace: "alpha".into(),
                },
                value: Some(ProjectionValue::Workspace(WorkspaceProjectionUpsert {
                    projection_token: "tok-2".into(),
                    record: workspace_projection("alpha", 20),
                })),
            },
        ),
        (
            5,
            ProjectionRecord {
                key: ProjectionKey::Cell {
                    world_id: world_id(),
                    workflow: "demo/Counter@1".into(),
                    key_hash: tok1_cell.cell.key_hash.clone(),
                },
                value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                    projection_token: "tok-1".into(),
                    record: tok1_cell.cell.clone(),
                    state_payload: tok1_cell.state_payload.clone(),
                })),
            },
        ),
        (
            6,
            ProjectionRecord {
                key: ProjectionKey::Cell {
                    world_id: world_id(),
                    workflow: "demo/Counter@1".into(),
                    key_hash: tok2_cell.cell.key_hash.clone(),
                },
                value: Some(ProjectionValue::Cell(CellProjectionUpsert {
                    projection_token: "tok-2".into(),
                    record: tok2_cell.cell.clone(),
                    state_payload: tok2_cell.state_payload.clone(),
                })),
            },
        ),
    ];

    let outcome = materializer.bootstrap_projection_partition(0, &entries)?;
    assert_eq!(outcome.last_offset, Some(6));
    assert_eq!(
        materializer.load_source_offset("aos-projection", 0)?,
        Some(6)
    );
    assert_eq!(
        materializer.sqlite().load_projection_token(world_id())?,
        Some("tok-2".into())
    );
    assert_eq!(
        materializer
            .sqlite()
            .load_workspace_projection(universe_id(), world_id(), "alpha")?
            .expect("workspace after bootstrap")
            .journal_head,
        20
    );
    assert_eq!(
        materializer.sqlite().load_cell_projection(
            universe_id(),
            world_id(),
            "demo/Counter@1",
            b"a"
        )?,
        None
    );
    assert_eq!(
        materializer.sqlite().load_cell_projection(
            universe_id(),
            world_id(),
            "demo/Counter@1",
            b"b"
        )?,
        Some(tok2_cell)
    );

    Ok(())
}
