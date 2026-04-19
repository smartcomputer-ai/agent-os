#[path = "../tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use aos_kernel::journal::{CustomRecord, JournalRecord};
use aos_node::kafka::{FlushCommit, HostedKafkaBackend};
use aos_node::{JournalBackend, JournalFlush, UniverseId, WorldId, WorldLogFrame};
use serial_test::serial;

use common::{broker_kafka_config, ensure_kafka_topics, kafka_broker_enabled};

const TEST_WAIT_SLEEP: Duration = Duration::from_millis(5);

fn test_frame(
    universe_id: UniverseId,
    world_id: WorldId,
    world_epoch: u64,
    world_seq: u64,
    tag: &str,
    data: &[u8],
) -> WorldLogFrame {
    WorldLogFrame {
        format_version: 1,
        universe_id,
        world_id,
        world_epoch,
        world_seq_start: world_seq,
        world_seq_end: world_seq,
        records: vec![JournalRecord::Custom(CustomRecord {
            tag: tag.into(),
            data: data.to_vec(),
        })],
    }
}

async fn wait_for_world_frames(
    kafka: &mut HostedKafkaBackend,
    partition: u32,
    world_id: WorldId,
    expected_len: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        kafka.recover_partition_from_broker(partition).unwrap();
        if kafka.world_frames(world_id).len() == expected_len {
            return;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    assert_eq!(kafka.world_frames(world_id).len(), expected_len);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_commit_flush_persists_frame() {
    if !kafka_broker_enabled() {
        return;
    }

    let Some(config) = broker_kafka_config("commit-flush-persists", 1) else {
        return;
    };
    ensure_kafka_topics(&config, 1).await.unwrap();
    let mut writer = HostedKafkaBackend::new(1, config.clone()).unwrap();

    let mut reader_config = config.clone();
    reader_config.transactional_id = format!("{}-reader", config.transactional_id);
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();

    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    let frame = test_frame(universe_id, world_id, 1, 0, "committed", b"ok");

    writer
        .commit_flush(JournalFlush {
            frames: vec![frame.clone()],
            dispositions: Vec::new(),
            source_acks: Vec::new(),
        })
        .unwrap();

    wait_for_world_frames(&mut reader, 0, world_id, 1).await;
    let journal_topic = reader.config().journal_topic.clone();
    assert_eq!(reader.partition_entries(&journal_topic, 0).len(), 1);
    assert_eq!(reader.world_frames(world_id), &[frame]);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_journal_recovery_replays_only_new_frames() {
    if !kafka_broker_enabled() {
        return;
    }

    let Some(writer_config) = broker_kafka_config("journal-writer", 1) else {
        return;
    };
    ensure_kafka_topics(&writer_config, 1).await.unwrap();
    let mut reader_config = writer_config.clone();
    reader_config.transactional_id = format!("{}-reader", writer_config.transactional_id);
    let mut writer = HostedKafkaBackend::new(1, writer_config).unwrap();
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    writer
        .append_frame(test_frame(universe_id, world_id, 1, 0, "frame-0", b"a"))
        .unwrap();
    writer
        .append_frame(test_frame(universe_id, world_id, 1, 1, "frame-1", b"b"))
        .unwrap();

    wait_for_world_frames(&mut reader, 0, world_id, 2).await;
    let journal_topic = reader.config().journal_topic.clone();
    assert_eq!(reader.partition_entries(&journal_topic, 0).len(), 2);

    writer
        .append_frame(test_frame(universe_id, world_id, 1, 2, "frame-2", b"c"))
        .unwrap();
    wait_for_world_frames(&mut reader, 0, world_id, 3).await;
    assert_eq!(reader.partition_entries(&journal_topic, 0).len(), 3);

    reader.recover_partition_from_broker(0).unwrap();
    assert_eq!(reader.world_frames(world_id).len(), 3);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_allows_forward_world_sequence_gaps() {
    if !kafka_broker_enabled() {
        return;
    }

    let Some(writer_config) = broker_kafka_config("journal-gap-writer", 1) else {
        return;
    };
    ensure_kafka_topics(&writer_config, 1).await.unwrap();
    let mut reader_config = writer_config.clone();
    reader_config.transactional_id = format!("{}-reader", writer_config.transactional_id);
    let mut writer = HostedKafkaBackend::new(1, writer_config).unwrap();
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    let frame_0 = test_frame(universe_id, world_id, 1, 0, "frame-0", b"a");
    let frame_3 = test_frame(universe_id, world_id, 1, 3, "frame-3", b"d");

    writer.append_frame(frame_0.clone()).unwrap();
    writer.append_frame(frame_3.clone()).unwrap();

    wait_for_world_frames(&mut reader, 0, world_id, 2).await;
    assert_eq!(reader.world_frames(world_id), &[frame_0, frame_3]);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_aborted_flush_stays_hidden_until_retried() {
    if !kafka_broker_enabled() {
        return;
    }

    let Some(config) = broker_kafka_config("abort-replay", 1) else {
        return;
    };
    ensure_kafka_topics(&config, 1).await.unwrap();
    let mut writer = HostedKafkaBackend::new(1, config.clone()).unwrap();

    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    let frame = test_frame(universe_id, world_id, 1, 0, "aborted", b"x");

    writer.debug_fail_next_batch_commit();
    assert!(
        writer
            .commit_flush_batch(FlushCommit {
                frames: vec![frame.clone()],
                dispositions: Vec::new(),
                offset_commits: BTreeMap::new(),
            })
            .is_err()
    );
    drop(writer);

    let mut reader_config = config.clone();
    reader_config.transactional_id = format!("{}-reader", config.transactional_id);
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();
    reader.recover_partition_from_broker(0).unwrap();
    assert!(reader.world_frames(world_id).is_empty());

    let mut recovered_writer = HostedKafkaBackend::new(1, config).unwrap();
    recovered_writer
        .commit_flush_batch(FlushCommit {
            frames: vec![frame],
            dispositions: Vec::new(),
            offset_commits: BTreeMap::new(),
        })
        .unwrap();

    wait_for_world_frames(&mut reader, 0, world_id, 1).await;
    let frame = &reader.world_frames(world_id)[0];
    assert_eq!(frame.world_seq_start, 0);
    assert_eq!(frame.world_seq_end, 0);
}
