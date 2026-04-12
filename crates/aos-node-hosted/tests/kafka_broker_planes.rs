mod common;

use std::time::{Duration, Instant};

use aos_cbor::to_canonical_cbor;
use aos_kernel::journal::{CustomRecord, JournalRecord};
use aos_node::{SubmissionEnvelope, UniverseId, WorldId, WorldLogFrame};
use aos_node_hosted::kafka::{HostedKafkaBackend, SubmissionBatch};
use serial_test::serial;

use common::{
    blobstore_bucket_enabled, broker_kafka_config, kafka_broker_enabled, wait_for_kafka_assignment,
    wait_for_kafka_pending_submissions,
};

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
    _universe_id: UniverseId,
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

async fn wait_for_nonempty_batch(
    kafka: &mut HostedKafkaBackend,
    partition: u32,
) -> SubmissionBatch {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        wait_for_kafka_pending_submissions(kafka, 1).await.unwrap();
        let batch = kafka.drain_partition_submissions(partition).unwrap();
        if !batch.submissions.is_empty() {
            return batch;
        }
        tokio::time::sleep(TEST_WAIT_SLEEP).await;
    }
    let batch = kafka.drain_partition_submissions(partition).unwrap();
    assert!(
        !batch.submissions.is_empty(),
        "timed out waiting for a non-empty Kafka submission batch"
    );
    batch
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_submit_poll_commit_and_recover_committed_frame() {
    if !kafka_broker_enabled() {
        return;
    }

    let Some(config) = broker_kafka_config("submit-commit-recover", 1) else {
        return;
    };
    let mut worker = HostedKafkaBackend::new(1, config.clone()).unwrap();
    let assigned = wait_for_kafka_assignment(&mut worker).await.unwrap();
    assert_eq!(assigned, vec![0]);

    let mut reader_config = config.clone();
    reader_config.submission_group_prefix = format!("{}-reader", config.submission_group_prefix);
    reader_config.transactional_id = format!("{}-reader", config.transactional_id);
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();

    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    let submission_id = format!("submission-{}", uuid::Uuid::new_v4());
    worker
        .submit(SubmissionEnvelope::domain_event(
            submission_id.clone(),
            universe_id,
            world_id,
            1,
            "demo/Test@1",
            to_canonical_cbor(&42u64).unwrap(),
        ))
        .unwrap();

    let batch = wait_for_nonempty_batch(&mut worker, 0).await;
    assert_eq!(batch.submissions.len(), 1);
    assert_eq!(batch.submissions[0].submission_id, submission_id);
    assert_eq!(batch.submissions[0].universe_id, universe_id);
    assert_eq!(batch.submissions[0].world_id, world_id);

    let frame = test_frame(universe_id, world_id, 1, 0, "committed", b"ok");
    worker
        .commit_submission_batch(batch, vec![frame.clone()])
        .unwrap();

    wait_for_world_frames(&mut reader, 0, universe_id, world_id, 1).await;
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
    let mut reader_config = writer_config.clone();
    reader_config.submission_group_prefix =
        format!("{}-reader", writer_config.submission_group_prefix);
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

    wait_for_world_frames(&mut reader, 0, universe_id, world_id, 2).await;
    let journal_topic = reader.config().journal_topic.clone();
    assert_eq!(reader.partition_entries(&journal_topic, 0).len(), 2);

    writer
        .append_frame(test_frame(universe_id, world_id, 1, 2, "frame-2", b"c"))
        .unwrap();
    wait_for_world_frames(&mut reader, 0, universe_id, world_id, 3).await;
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
    let mut reader_config = writer_config.clone();
    reader_config.submission_group_prefix =
        format!("{}-reader", writer_config.submission_group_prefix);
    reader_config.transactional_id = format!("{}-reader", writer_config.transactional_id);
    let mut writer = HostedKafkaBackend::new(1, writer_config).unwrap();
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();
    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    let frame_0 = test_frame(universe_id, world_id, 1, 0, "frame-0", b"a");
    let frame_3 = test_frame(universe_id, world_id, 1, 3, "frame-3", b"d");

    writer.append_frame(frame_0.clone()).unwrap();
    writer.append_frame(frame_3.clone()).unwrap();

    wait_for_world_frames(&mut reader, 0, universe_id, world_id, 2).await;
    assert_eq!(reader.world_frames(world_id), &[frame_0, frame_3]);
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn kafka_aborted_transaction_stays_hidden_until_replayed() {
    if !kafka_broker_enabled() || !blobstore_bucket_enabled() {
        return;
    }

    let Some(config) = broker_kafka_config("abort-replay", 1) else {
        return;
    };
    let mut worker = HostedKafkaBackend::new(1, config.clone()).unwrap();
    let assigned = wait_for_kafka_assignment(&mut worker).await.unwrap();
    assert!(assigned.contains(&0));

    let universe_id = UniverseId::from(uuid::Uuid::new_v4());
    let world_id = WorldId::from(uuid::Uuid::new_v4());
    worker
        .submit(SubmissionEnvelope::domain_event(
            format!("submission-{}", uuid::Uuid::new_v4()),
            universe_id,
            world_id,
            1,
            "demo/Test@1",
            to_canonical_cbor(&1u64).unwrap(),
        ))
        .unwrap();
    wait_for_kafka_pending_submissions(&mut worker, 1)
        .await
        .unwrap();
    assert!(worker.pending_submission_count() >= 1);

    let batch = wait_for_nonempty_batch(&mut worker, 0).await;
    worker.debug_fail_next_batch_commit();
    let frame = test_frame(universe_id, world_id, 1, 0, "aborted", b"x");
    assert!(
        worker
            .commit_submission_batch(batch, vec![frame.clone()])
            .is_err()
    );
    drop(worker);

    let mut reader_config = config.clone();
    reader_config.submission_group_prefix = format!("{}-reader", config.submission_group_prefix);
    reader_config.transactional_id = format!("{}-reader", config.transactional_id);
    let mut reader = HostedKafkaBackend::new(1, reader_config).unwrap();
    reader.recover_partition_from_broker(0).unwrap();
    assert!(reader.world_frames(world_id).is_empty());

    let mut recovered_worker = HostedKafkaBackend::new(1, config).unwrap();
    let assigned = wait_for_kafka_assignment(&mut recovered_worker)
        .await
        .unwrap();
    assert!(assigned.contains(&0));
    wait_for_kafka_pending_submissions(&mut recovered_worker, 1)
        .await
        .unwrap();
    assert!(recovered_worker.pending_submission_count() >= 1);
    let batch = wait_for_nonempty_batch(&mut recovered_worker, 0).await;
    recovered_worker
        .commit_submission_batch(batch, vec![frame])
        .unwrap();

    wait_for_world_frames(&mut reader, 0, universe_id, world_id, 1).await;
    let frame = &reader.world_frames(world_id)[0];
    assert_eq!(frame.world_seq_start, 0);
    assert_eq!(frame.world_seq_end, 0);
}
