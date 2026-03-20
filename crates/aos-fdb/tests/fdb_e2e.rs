#![cfg(feature = "foundationdb-backend")]

use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use aos_fdb::{
    CasConfig, CborPayload, CommandIngress, FdbRuntime, FdbWorldPersistence, InboxConfig,
    InboxItem, PersistenceConfig, SegmentId, SegmentIndexRecord, SnapshotRecord, TimerFiredIngress,
    UniverseId, WorldId, WorldStore,
};
use uuid::Uuid;

fn open_persistence(
    runtime: Arc<FdbRuntime>,
    config: PersistenceConfig,
) -> Result<FdbWorldPersistence, Box<dyn std::error::Error>> {
    match env::var_os("FDB_CLUSTER_FILE") {
        Some(cluster_file) => Ok(FdbWorldPersistence::open(
            runtime,
            Some(PathBuf::from(cluster_file)),
            config,
        )?),
        None => Ok(FdbWorldPersistence::open_default(runtime, config)?),
    }
}

fn cluster_is_reachable() -> bool {
    let cluster_file = env::var_os("FDB_CLUSTER_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/etc/foundationdb/fdb.cluster"));
    let cluster_line = match fs::read_to_string(&cluster_file) {
        Ok(contents) => contents
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        Err(_) => return false,
    };
    let Some(coord_part) = cluster_line.split('@').nth(1) else {
        return false;
    };
    let Some(first_coord) = coord_part.split(',').next() else {
        return false;
    };
    let addresses: Vec<SocketAddr> = match first_coord.to_socket_addrs() {
        Ok(addresses) => addresses.collect(),
        Err(_) => return false,
    };
    let socket_reachable = addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok());
    socket_reachable && fdbcli_status_ok(&cluster_file, Duration::from_secs(3))
}

fn universe() -> UniverseId {
    UniverseId::from(Uuid::new_v4())
}

fn fdbcli_status_ok(cluster_file: &std::path::Path, timeout: Duration) -> bool {
    let mut child = match Command::new("fdbcli")
        .arg("-C")
        .arg(cluster_file)
        .arg("--exec")
        .arg("status minimal")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn world() -> WorldId {
    WorldId::from(Uuid::new_v4())
}

#[test]
fn fdb_world_persistence_end_to_end_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    if !cluster_is_reachable() {
        eprintln!("skipping FoundationDB end-to-end test because no local cluster is reachable");
        return Ok(());
    }

    let runtime = Arc::new(FdbRuntime::boot()?);
    let persistence = open_persistence(
        runtime,
        PersistenceConfig {
            cas: CasConfig {
                verify_reads: true,
                ..CasConfig::default()
            },
            inbox: InboxConfig {
                inline_payload_threshold_bytes: 8,
            },
            ..PersistenceConfig::default()
        },
    )?;

    let universe = universe();
    let world = world();

    let inline_blob = b"small";
    let external_blob = b"this inbox payload is large enough to externalize";
    let inline_hash = persistence.cas_put_verified(universe, inline_blob)?;
    let external_hash = persistence.cas_put_verified(universe, external_blob)?;
    let external_hash_hex = external_hash.to_hex();

    assert!(persistence.cas_has(universe, inline_hash)?);
    assert!(persistence.cas_has(universe, external_hash)?);
    assert_eq!(persistence.cas_get(universe, inline_hash)?, inline_blob);
    assert_eq!(persistence.cas_get(universe, external_hash)?, external_blob);

    let first = InboxItem::TimerFired(TimerFiredIngress {
        timer_id: "timer-1".into(),
        payload: CborPayload::inline(vec![1, 2, 3]),
        correlation_id: Some("corr-1".into()),
    });
    let second = InboxItem::Control(CommandIngress {
        command_id: "cmd-2".into(),
        command: "event-send".into(),
        actor: None,
        payload: CborPayload::inline(external_blob.to_vec()),
        submitted_at_ns: 2,
    });

    let seq1 = persistence.inbox_enqueue(universe, world, first.clone())?;
    let seq2 = persistence.inbox_enqueue(universe, world, second)?;
    assert!(seq1 < seq2);

    let items = persistence.inbox_read_after(universe, world, None, 8)?;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].0, seq1);
    assert_eq!(items[0].1, first);
    assert_eq!(items[1].0, seq2);
    match &items[1].1 {
        InboxItem::Control(control) => {
            assert_eq!(control.command, "event-send");
            assert!(control.payload.inline_cbor.is_none());
            assert_eq!(
                control.payload.cbor_ref.as_deref(),
                Some(external_hash_hex.as_str())
            );
            assert_eq!(
                control.payload.cbor_sha256.as_deref(),
                Some(external_hash_hex.as_str())
            );
            assert_eq!(control.payload.cbor_size, Some(external_blob.len() as u64));
        }
        other => panic!("expected control ingress, got {other:?}"),
    }

    assert_eq!(persistence.inbox_cursor(universe, world)?, None);
    persistence.inbox_commit_cursor(universe, world, None, seq1.clone())?;
    assert_eq!(
        persistence.inbox_cursor(universe, world)?,
        Some(seq1.clone())
    );

    assert_eq!(persistence.journal_head(universe, world)?, 0);
    persistence.journal_append_batch(universe, world, 0, &[b"journal-entry-0".to_vec()])?;
    persistence.drain_inbox_to_journal(
        universe,
        world,
        Some(seq1),
        seq2.clone(),
        1,
        &[b"journal-entry-1".to_vec()],
    )?;

    assert_eq!(persistence.journal_head(universe, world)?, 2);
    assert_eq!(
        persistence.journal_read_range(universe, world, 0, 8)?,
        vec![
            (0, b"journal-entry-0".to_vec()),
            (1, b"journal-entry-1".to_vec()),
        ]
    );
    assert_eq!(
        persistence.inbox_cursor(universe, world)?,
        Some(seq2.clone())
    );
    assert_eq!(
        persistence.inbox_read_after(universe, world, Some(seq2), 8)?,
        Vec::new()
    );

    let snapshot = SnapshotRecord {
        snapshot_ref: format!("cas:{}", external_hash.to_hex()),
        height: 2,
        logical_time_ns: 20,
        receipt_horizon_height: Some(2),
        manifest_hash: Some("sha256:manifest".into()),
    };
    persistence.snapshot_index(universe, world, snapshot.clone())?;
    persistence.snapshot_promote_baseline(universe, world, snapshot.clone())?;
    assert_eq!(
        persistence.snapshot_at_height(universe, world, 2)?,
        snapshot
    );
    assert_eq!(
        persistence.snapshot_active_baseline(universe, world)?,
        snapshot
    );

    let segment = SegmentIndexRecord {
        segment: SegmentId::new(0, 2)?,
        body_ref: external_hash.to_hex(),
        checksum: format!("sha256:{}", external_hash.to_hex()),
    };
    persistence.segment_index_put(universe, world, segment.clone())?;
    assert_eq!(
        persistence.segment_index_read_from(universe, world, 2, 8)?,
        vec![segment]
    );

    Ok(())
}
