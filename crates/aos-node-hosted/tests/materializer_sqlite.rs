use std::collections::BTreeMap;
use std::str::FromStr;

use aos_cbor::Hash;
use aos_node::{CborPayload, LocalStatePaths, SnapshotRecord, UniverseId, WorldId};
use aos_node_hosted::kafka::WorldMetaProjection;
use aos_node_hosted::materializer::{
    CellStateProjectionRecord, HeadProjectionRecord, MaterializedCellRow,
    MaterializedJournalEntryRow, MaterializedWorldRow, MaterializerSourceOffsetRow,
    MaterializerSqliteStore, WorkspaceRegistryProjectionRecord, WorkspaceVersionProjectionRecord,
};
use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

fn universe_id() -> UniverseId {
    UniverseId::from_str("00000000-0000-0000-0000-000000000001").expect("valid universe id")
}

fn world_id() -> WorldId {
    WorldId::from_str("00000000-0000-0000-0000-0000000000a1").expect("valid world id")
}

fn other_world_id() -> WorldId {
    WorldId::from_str("00000000-0000-0000-0000-0000000000b2").expect("valid world id")
}

fn sample_baseline(height: u64, manifest_hash: &str) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: format!("sha256:{height:064x}"),
        height,
        universe_id: universe_id(),
        logical_time_ns: height * 100,
        receipt_horizon_height: Some(height),
        manifest_hash: Some(manifest_hash.into()),
    }
}

fn sample_world_meta(token: &str, journal_head: u64, manifest_hash: &str) -> WorldMetaProjection {
    WorldMetaProjection {
        universe_id: universe_id(),
        projection_token: token.into(),
        world_epoch: 1,
        journal_head,
        manifest_hash: manifest_hash.into(),
        active_baseline: sample_baseline(journal_head, manifest_hash),
        updated_at_ns: journal_head * 10,
    }
}

fn sample_workspace(
    workspace: &str,
    journal_head: u64,
    latest_version: u64,
) -> WorkspaceRegistryProjectionRecord {
    let mut versions = BTreeMap::new();
    versions.insert(
        latest_version,
        WorkspaceVersionProjectionRecord {
            root_hash: format!("sha256:{latest_version:064x}"),
            owner: "lukas".into(),
            created_at_ns: latest_version * 100,
        },
    );
    WorkspaceRegistryProjectionRecord {
        journal_head,
        workspace: workspace.into(),
        latest_version,
        versions,
        updated_at_ns: journal_head * 10,
    }
}

fn sample_cell(workflow: &str, key_bytes: &[u8], journal_head: u64) -> MaterializedCellRow {
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

#[test]
fn materializer_sqlite_persists_offsets_and_world_meta() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut store = MaterializerSqliteStore::from_paths(&paths)?;

    let row_b = MaterializerSourceOffsetRow {
        journal_topic: "aos-journal-b".into(),
        partition: 0,
        last_offset: 18,
        updated_at_ns: 2,
    };
    let row_a = MaterializerSourceOffsetRow {
        journal_topic: "aos-journal-a".into(),
        partition: 1,
        last_offset: 7,
        updated_at_ns: 1,
    };
    let row_a_updated = MaterializerSourceOffsetRow {
        last_offset: 9,
        updated_at_ns: 3,
        ..row_a.clone()
    };

    store.persist_source_offset(&row_b)?;
    store.persist_source_offset(&row_a)?;
    store.persist_source_offset(&row_a_updated)?;

    let offsets = store.load_source_offsets()?;
    assert_eq!(offsets, vec![row_a_updated, row_b.clone()]);
    assert_eq!(
        store.load_source_offset("aos-journal-b", 0)?,
        Some(row_b.clone())
    );

    let meta = sample_world_meta("tok-1", 12, "sha256:feed");
    assert!(!store.apply_world_meta_projection(world_id(), &meta)?);
    assert_eq!(
        store.load_projection_token(world_id())?,
        Some("tok-1".into())
    );
    assert_eq!(
        store.load_head_projection(universe_id(), world_id())?,
        Some(HeadProjectionRecord {
            journal_head: 12,
            manifest_hash: "sha256:feed".into(),
            universe_id: universe_id(),
            updated_at_ns: 120,
        })
    );
    assert_eq!(
        store.load_world_projection(universe_id(), world_id())?,
        Some(MaterializedWorldRow {
            world_id: world_id(),
            universe_id: universe_id(),
            journal_head: 12,
            manifest_hash: "sha256:feed".into(),
            active_baseline: sample_baseline(12, "sha256:feed"),
        })
    );
    assert_eq!(
        store.load_world_projections_page(universe_id(), None, 10)?,
        vec![MaterializedWorldRow {
            world_id: world_id(),
            universe_id: universe_id(),
            journal_head: 12,
            manifest_hash: "sha256:feed".into(),
            active_baseline: sample_baseline(12, "sha256:feed"),
        }]
    );
    assert!(store.config().db_path.is_file());

    Ok(())
}

#[test]
fn materializer_sqlite_recovers_from_partial_bootstrap_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let db_path = temp.path().join("materializer.sqlite3");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "
        create table materializer_meta (
            singleton integer primary key check (singleton = 1),
            schema_version integer not null
        );
        create table source_offsets (
            journal_topic text not null,
            partition integer not null,
            last_offset integer not null,
            updated_at_ns integer not null,
            primary key (journal_topic, partition)
        );
        create table head_projection (
            universe_id text not null,
            world_id text not null,
            journal_head integer not null,
            manifest_hash text not null,
            updated_at_ns integer not null,
            record blob not null,
            primary key (world_id)
        );
        ",
    )?;
    drop(conn);

    let paths = LocalStatePaths::new(temp.path());
    let mut store = MaterializerSqliteStore::from_paths(&paths)?;
    let meta = sample_world_meta("tok-1", 12, "sha256:feed");

    store.apply_world_meta_projection(world_id(), &meta)?;

    assert_eq!(
        store.load_world_projection(universe_id(), world_id())?,
        Some(MaterializedWorldRow {
            world_id: world_id(),
            universe_id: universe_id(),
            journal_head: 12,
            manifest_hash: "sha256:feed".into(),
            active_baseline: sample_baseline(12, "sha256:feed"),
        })
    );

    Ok(())
}

#[test]
fn materializer_sqlite_applies_token_gated_rows_and_clears_on_token_change()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut store = MaterializerSqliteStore::from_paths(&paths)?;

    let initial_meta = sample_world_meta("tok-1", 12, "sha256:feed");
    store.apply_world_meta_projection(world_id(), &initial_meta)?;

    let ws_alpha = sample_workspace("alpha", 5, 2);
    let ws_beta = sample_workspace("beta", 6, 4);
    assert!(store.apply_workspace_projection(world_id(), "tok-1", &ws_beta)?);
    assert!(store.apply_workspace_projection(world_id(), "tok-1", &ws_alpha)?);

    let cell_b = sample_cell("demo/Counter@1", b"b", 8);
    let cell_a = sample_cell("demo/Counter@1", b"a", 7);
    assert!(store.apply_cell_projection(world_id(), "tok-1", &cell_b)?);
    assert!(store.apply_cell_projection(world_id(), "tok-1", &cell_a)?);

    assert_eq!(
        store.load_workspace_projections(universe_id(), world_id())?,
        vec![ws_alpha.clone(), ws_beta.clone()]
    );
    assert_eq!(
        store.load_cell_projections(universe_id(), world_id(), "demo/Counter@1", 10)?,
        vec![cell_a.clone(), cell_b.clone()]
    );

    let stale_workspace = sample_workspace("stale", 9, 1);
    let stale_cell = sample_cell("demo/Counter@1", b"stale", 9);
    assert!(!store.apply_workspace_projection(world_id(), "tok-old", &stale_workspace)?);
    assert!(!store.apply_cell_projection(world_id(), "tok-old", &stale_cell)?);
    assert_eq!(
        store.load_workspace_projection(universe_id(), world_id(), "stale")?,
        None
    );
    assert_eq!(
        store.load_cell_projection(universe_id(), world_id(), "demo/Counter@1", b"stale")?,
        None
    );

    assert!(store.apply_workspace_tombstone(world_id(), "alpha")?);
    assert!(store.apply_cell_tombstone(world_id(), "demo/Counter@1", &cell_a.cell.key_hash)?);
    assert_eq!(
        store.load_workspace_projection(universe_id(), world_id(), "alpha")?,
        None
    );
    assert_eq!(
        store.load_cell_projection(universe_id(), world_id(), "demo/Counter@1", b"a")?,
        None
    );

    let journal_rows = vec![MaterializedJournalEntryRow {
        seq: 13,
        kind: "manifest".into(),
        record: json!({ "manifest_hash": "sha256:feed" }),
        raw_cbor: vec![0xA1],
    }];
    store.append_journal_entries(
        universe_id(),
        world_id(),
        13,
        Some("sha256:feed".into()),
        &journal_rows,
        None,
    )?;
    assert_eq!(
        store
            .load_journal_entries(universe_id(), world_id(), 0, 10)?
            .expect("journal before reset")
            .entries
            .len(),
        1
    );

    let reset_meta = sample_world_meta("tok-2", 20, "sha256:bead");
    assert!(store.apply_world_meta_projection(world_id(), &reset_meta)?);
    assert_eq!(
        store.load_projection_token(world_id())?,
        Some("tok-2".into())
    );
    assert!(
        store
            .load_workspace_projections(universe_id(), world_id())?
            .is_empty()
    );
    assert!(
        store
            .load_cell_projections(universe_id(), world_id(), "demo/Counter@1", 10)?
            .is_empty()
    );
    let journal_head = store
        .load_journal_head(universe_id(), world_id())?
        .expect("journal head after reset");
    assert_eq!(journal_head.journal_head, 20);
    assert_eq!(journal_head.retained_from, 21);
    let post_reset = store
        .load_journal_entries(universe_id(), world_id(), 0, 10)?
        .expect("journal state after reset");
    assert_eq!(post_reset.from, 21);
    assert_eq!(post_reset.retained_from, 21);
    assert!(post_reset.entries.is_empty());

    Ok(())
}

#[test]
fn materializer_sqlite_serves_retained_journal_tail() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let mut store = MaterializerSqliteStore::from_paths(&paths)?;

    let other_rows = vec![MaterializedJournalEntryRow {
        seq: 0,
        kind: "manifest".into(),
        record: json!({ "manifest_hash": "sha256:other" }),
        raw_cbor: vec![0xA0],
    }];
    store.append_journal_entries(
        universe_id(),
        other_world_id(),
        0,
        Some("sha256:other".into()),
        &other_rows,
        None,
    )?;

    let rows = vec![
        MaterializedJournalEntryRow {
            seq: 0,
            kind: "domain_event".into(),
            record: json!({ "schema": "demo/Event@1", "n": 0 }),
            raw_cbor: vec![0xA0],
        },
        MaterializedJournalEntryRow {
            seq: 1,
            kind: "manifest".into(),
            record: json!({ "manifest_hash": "sha256:bead" }),
            raw_cbor: vec![0xA1],
        },
        MaterializedJournalEntryRow {
            seq: 2,
            kind: "snapshot".into(),
            record: json!({ "snapshot_ref": "sha256:snap" }),
            raw_cbor: vec![0xA2],
        },
    ];
    store.append_journal_entries(
        universe_id(),
        world_id(),
        6,
        Some("sha256:bead".into()),
        &rows,
        None,
    )?;

    let head = store
        .load_journal_head(universe_id(), world_id())?
        .expect("journal head");
    assert_eq!(head.journal_head, 6);
    assert_eq!(head.retained_from, 0);
    assert_eq!(head.manifest_hash.as_deref(), Some("sha256:bead"));

    let entries = store
        .load_journal_entries(universe_id(), world_id(), 0, 2)?
        .expect("journal entries");
    assert_eq!(entries.from, 0);
    assert_eq!(entries.retained_from, 0);
    assert_eq!(entries.next_from, 2);
    assert_eq!(entries.entries.len(), 2);
    assert_eq!(entries.entries[0].kind, "domain_event");
    assert_eq!(entries.entries[1].seq, 1);

    let raw = store
        .load_journal_entries_raw(universe_id(), world_id(), 1, 3)?
        .expect("raw journal entries");
    assert_eq!(raw.from, 1);
    assert_eq!(raw.next_from, 3);
    assert_eq!(
        raw.entries
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    store.prune_journal_entries_through(universe_id(), world_id(), 1)?;
    let pruned_head = store
        .load_journal_head(universe_id(), world_id())?
        .expect("journal head after prune");
    assert_eq!(pruned_head.retained_from, 2);

    let pruned = store
        .load_journal_entries(universe_id(), world_id(), 0, 10)?
        .expect("journal entries after prune");
    assert_eq!(pruned.from, 2);
    assert_eq!(pruned.retained_from, 2);
    assert_eq!(pruned.next_from, 3);
    assert_eq!(pruned.entries.len(), 1);
    assert_eq!(pruned.entries[0].seq, 2);

    Ok(())
}
