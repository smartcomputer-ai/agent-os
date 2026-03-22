use std::str::FromStr;

use aos_kernel::journal::{CustomRecord, JournalRecord};
use aos_node::{
    CommandRecord, CommandStatus, LocalSqlitePlanes, LocalStatePaths, SnapshotRecord, UniverseId,
    WorldId, WorldLogFrame, local_universe_id,
};
use tempfile::tempdir;

fn world_id_a() -> WorldId {
    WorldId::from_str("00000000-0000-0000-0000-0000000000a1").expect("valid world id")
}

fn world_id_b() -> WorldId {
    WorldId::from_str("00000000-0000-0000-0000-0000000000b2").expect("valid world id")
}

fn sample_snapshot(snapshot_ref: &str, height: u64) -> SnapshotRecord {
    SnapshotRecord {
        snapshot_ref: snapshot_ref.into(),
        height,
        universe_id: UniverseId::nil(),
        logical_time_ns: height * 10,
        receipt_horizon_height: Some(height.saturating_sub(1)),
        manifest_hash: Some(format!("sha256:{height:064x}")),
    }
}

fn sample_frame(
    world_id: WorldId,
    world_epoch: u64,
    world_seq_start: u64,
    tag: &str,
) -> WorldLogFrame {
    WorldLogFrame {
        format_version: 1,
        universe_id: local_universe_id(),
        world_id,
        world_epoch,
        world_seq_start,
        world_seq_end: world_seq_start,
        records: vec![JournalRecord::Custom(CustomRecord {
            tag: tag.into(),
            data: vec![world_seq_start as u8],
        })],
    }
}

fn sample_command(
    command_id: &str,
    status: CommandStatus,
    journal_height: Option<u64>,
    manifest_hash: Option<&str>,
) -> CommandRecord {
    CommandRecord {
        command_id: command_id.into(),
        command: "sync_world".into(),
        status,
        submitted_at_ns: 11,
        started_at_ns: Some(12),
        finished_at_ns: Some(13),
        journal_height,
        manifest_hash: manifest_hash.map(str::to_owned),
        result_payload: None,
        error: None,
    }
}

#[test]
fn sqlite_planes_bootstrap_runtime_meta_and_persist_updates()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());

    let planes = LocalSqlitePlanes::from_paths(&paths)?;
    assert_eq!(planes.load_runtime_meta()?, (0, 0));

    planes.persist_runtime_counters(7, 13)?;
    assert_eq!(planes.load_runtime_meta()?, (7, 13));

    drop(planes);

    let reopened = LocalSqlitePlanes::from_paths(&paths)?;
    assert_eq!(reopened.load_runtime_meta()?, (7, 13));
    assert!(paths.runtime_db().is_file());
    Ok(())
}

#[test]
fn sqlite_planes_round_trip_world_directory_and_checkpoint_heads()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let planes = LocalSqlitePlanes::from_paths(&paths)?;

    let world_a = world_id_a();
    let world_b = world_id_b();
    let world_epoch_a = 3;
    let world_epoch_b = 5;
    let baseline_a = sample_snapshot("cas:alpha-snapshot", 7);
    let baseline_b = sample_snapshot("cas:beta-snapshot", 9);

    planes.persist_world_directory(
        world_b,
        UniverseId::nil(),
        200,
        "sha256:beta",
        world_epoch_b,
    )?;
    planes.persist_checkpoint_head(world_b, &baseline_b, 10)?;
    planes.persist_world_directory(
        world_a,
        UniverseId::nil(),
        100,
        "sha256:alpha",
        world_epoch_a,
    )?;
    planes.persist_checkpoint_head(world_a, &baseline_a, 8)?;

    let rows = planes.load_world_directory()?;
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].0.world_id, world_a);
    assert_eq!(rows[0].0.universe_id, UniverseId::nil());
    assert_eq!(rows[0].0.created_at_ns, 100);
    assert_eq!(rows[0].0.initial_manifest_hash, "sha256:alpha");
    assert_eq!(rows[0].0.world_epoch, world_epoch_a);
    assert_eq!(rows[0].1.world_id, world_a);
    assert_eq!(rows[0].1.active_baseline, baseline_a);
    assert_eq!(rows[0].1.next_world_seq, 8);

    assert_eq!(rows[1].0.world_id, world_b);
    assert_eq!(rows[1].0.universe_id, UniverseId::nil());
    assert_eq!(rows[1].0.created_at_ns, 200);
    assert_eq!(rows[1].0.initial_manifest_hash, "sha256:beta");
    assert_eq!(rows[1].0.world_epoch, world_epoch_b);
    assert_eq!(rows[1].1.world_id, world_b);
    assert_eq!(rows[1].1.active_baseline, baseline_b);
    assert_eq!(rows[1].1.next_world_seq, 10);

    Ok(())
}

#[test]
fn sqlite_planes_load_world_frames_for_one_world_in_world_seq_order()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let planes = LocalSqlitePlanes::from_paths(&paths)?;
    let world_id = world_id_a();

    let later = sample_frame(world_id, 2, 10, "later");
    let earlier = sample_frame(world_id, 2, 0, "earlier");
    let other_world = sample_frame(world_id_b(), 2, 0, "other-world");

    planes.append_journal_frame(41, world_id, &later)?;
    planes.append_journal_frame(40, world_id, &earlier)?;
    planes.append_journal_frame(42, world_id_b(), &other_world)?;

    let frames = planes.load_frame_log_for_world(world_id)?;
    assert_eq!(frames, vec![earlier, later]);

    Ok(())
}

#[test]
fn sqlite_planes_upsert_command_projection_records() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let paths = LocalStatePaths::new(temp.path());
    let planes = LocalSqlitePlanes::from_paths(&paths)?;
    let world_id = world_id_a();

    let queued = sample_command("cmd-1", CommandStatus::Queued, Some(1), None);
    let updated = sample_command(
        "cmd-1",
        CommandStatus::Succeeded,
        Some(5),
        Some("sha256:updated"),
    );
    let running = sample_command("cmd-2", CommandStatus::Running, Some(3), None);

    planes.persist_command_projection(world_id, &queued)?;
    planes.persist_command_projection(world_id, &updated)?;
    planes.persist_command_projection(world_id, &running)?;

    let commands = planes.load_command_projection(world_id)?;
    assert_eq!(commands.len(), 2);
    assert_eq!(
        commands.keys().cloned().collect::<Vec<_>>(),
        vec!["cmd-1".to_string(), "cmd-2".to_string()],
    );
    assert_eq!(commands.get("cmd-1"), Some(&updated));
    assert_eq!(commands.get("cmd-2"), Some(&running));

    Ok(())
}
