use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aos_node::PersistError;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer};
use rdkafka::error::KafkaError;
use rdkafka::message::Message;
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use thiserror::Error;

use crate::bootstrap::MaterializerDeps;
use crate::kafka::{
    FetchedRecord, HostedJournalRecord, KafkaConfig, PartitionLogEntry, ProjectionKey,
    ProjectionRecord, ProjectionValue, fetch_partition_records,
};
use crate::worker::WorkerError;

use super::{Materializer, MaterializerError};

const MATERIALIZER_CONSUMER_POLL_WAIT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct HostedMaterializerConfig {
    pub retained_journal_entries_per_world: u64,
}

impl Default for HostedMaterializerConfig {
    fn default() -> Self {
        Self {
            retained_journal_entries_per_world: std::env::var(
                "AOS_MATERIALIZER_RETAINED_JOURNAL_ENTRIES_PER_WORLD",
            )
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(2048),
        }
    }
}

#[derive(Debug, Error)]
pub enum HostedMaterializerError {
    #[error(transparent)]
    Worker(#[from] WorkerError),
    #[error(transparent)]
    Materializer(#[from] MaterializerError),
}

#[derive(Clone)]
pub struct HostedMaterializer {
    deps: Arc<MaterializerDeps>,
    config: HostedMaterializerConfig,
}

impl HostedMaterializer {
    pub fn new(deps: MaterializerDeps, config: HostedMaterializerConfig) -> Self {
        Self {
            deps: Arc::new(deps),
            config,
        }
    }

    pub fn config(&self) -> &HostedMaterializerConfig {
        &self.config
    }

    pub async fn serve_forever(self) {
        let mut materializer = match self.open_materializer() {
            Ok(materializer) => materializer,
            Err(err) => {
                tracing::error!(error = %err, "materializer failed to initialize");
                return;
            }
        };

        match self
            .serve_broker_consumer_forever(&mut materializer, self.deps.kafka_config.clone())
            .await
        {
            Ok(()) => {}
            Err(err) => {
                tracing::error!(error = %err, "materializer consumer failed");
            }
        }
    }

    async fn serve_broker_consumer_forever<S: aos_kernel::Store + 'static>(
        &self,
        materializer: &mut Materializer<S>,
        kafka_config: KafkaConfig,
    ) -> Result<(), HostedMaterializerError> {
        let projection_topic = materializer.config().projection_topic.clone();
        let journal_topic = materializer.config().journal_topic.clone();
        let consumer = open_materializer_consumer(
            &kafka_config,
            &[projection_topic.clone(), journal_topic.clone()],
        )?;
        let mut assigned = BTreeSet::new();

        loop {
            let polled = consumer.poll(MATERIALIZER_CONSUMER_POLL_WAIT);
            let newly_assigned = sync_consumer_assignments(
                &consumer,
                &[projection_topic.clone(), journal_topic.clone()],
                &mut assigned,
                materializer,
                &kafka_config,
            )?;
            if !newly_assigned.is_empty() {
                tracing::info!(assignments = ?newly_assigned, "materializer consumer assignment updated");
            }

            let Some(message) = polled.transpose().map_err(kafka_consumer_err)? else {
                continue;
            };
            let topic = message.topic().to_owned();
            let partition = u32::try_from(message.partition()).map_err(|_| {
                WorkerError::Persist(PersistError::backend(format!(
                    "materializer consumed negative partition {}",
                    message.partition()
                )))
            })?;
            if newly_assigned.contains(&(topic.clone(), partition)) {
                continue;
            }

            let outcome = if topic == projection_topic {
                let (offset, record) = decode_projection_entry(&message)?;
                materializer.apply_projection_record(partition, offset, &record)?
            } else if topic == journal_topic {
                let Some(entry) = decode_journal_entry(&message)? else {
                    commit_partition_offset(
                        &consumer,
                        &topic,
                        partition,
                        (message.offset() as u64).saturating_add(1),
                    )?;
                    continue;
                };
                materializer.index_journal_entry(partition, &entry)?
            } else {
                continue;
            };

            commit_partition_offset(
                &consumer,
                &topic,
                partition,
                outcome.last_offset.unwrap_or_default().saturating_add(1),
            )?;
            tracing::debug!(
                topic = %topic,
                partition,
                last_offset = ?outcome.last_offset,
                processed_entries = outcome.processed_entries,
                touched_worlds = outcome.touched_worlds,
                cells_materialized = outcome.cells_materialized,
                workspaces_materialized = outcome.workspaces_materialized,
                journal_entries_indexed = outcome.journal_entries_indexed,
                "materializer consumed topic entry"
            );
        }
    }

    fn open_materializer(
        &self,
    ) -> Result<Materializer<crate::blobstore::HostedCas>, HostedMaterializerError> {
        let journal_topic = self.deps.journal.journal_topic()?;
        let mut config = super::MaterializerConfig::from_paths(&self.deps.paths, journal_topic);
        config.projection_topic = self.deps.kafka_config.projection_topic.clone();
        config.retained_journal_entries_per_world =
            Some(self.config.retained_journal_entries_per_world);
        config.kernel_config.secret_resolver = Some(self.deps.secret_resolver.clone());
        Materializer::from_config(config).map_err(Into::into)
    }
}

fn open_materializer_consumer(
    config: &KafkaConfig,
    topics: &[String],
) -> Result<BaseConsumer, HostedMaterializerError> {
    let bootstrap_servers = config
        .bootstrap_servers
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            WorkerError::Persist(PersistError::backend(
                "AOS_KAFKA_BOOTSTRAP_SERVERS must be set for broker-backed materialization",
            ))
        })?;
    let mut client = ClientConfig::new();
    client
        .set("bootstrap.servers", bootstrap_servers)
        .set(
            "group.id",
            format!("{}-materializer", config.submission_group_prefix),
        )
        .set("enable.auto.commit", "false")
        .set("enable.auto.offset.store", "false")
        .set("enable.partition.eof", "false")
        .set("auto.offset.reset", "earliest")
        .set("isolation.level", "read_committed")
        .set(
            "session.timeout.ms",
            config.group_session_timeout_ms.to_string(),
        )
        .set(
            "heartbeat.interval.ms",
            config.group_heartbeat_interval_ms.to_string(),
        );
    let consumer: BaseConsumer = client.create().map_err(kafka_consumer_err)?;
    if config.direct_assigned_partitions.is_empty() {
        let refs = topics.iter().map(String::as_str).collect::<Vec<_>>();
        consumer.subscribe(&refs).map_err(kafka_consumer_err)?;
    } else {
        let mut assignment = TopicPartitionList::new();
        for topic in topics {
            for partition in &config.direct_assigned_partitions {
                assignment
                    .add_partition_offset(topic, *partition as i32, Offset::Beginning)
                    .map_err(|err| kafka_plane_err("assign materializer partition", err))?;
            }
        }
        consumer.assign(&assignment).map_err(kafka_consumer_err)?;
    }
    Ok(consumer)
}

fn sync_consumer_assignments<S: aos_kernel::Store + 'static>(
    consumer: &BaseConsumer,
    topics: &[String],
    assigned: &mut BTreeSet<(String, u32)>,
    materializer: &mut Materializer<S>,
    kafka_config: &KafkaConfig,
) -> Result<Vec<(String, u32)>, HostedMaterializerError> {
    let current = current_assignment(consumer, topics)?;
    let newly_assigned = current.difference(assigned).cloned().collect::<Vec<_>>();
    for (topic, partition) in &newly_assigned {
        bootstrap_projection_partition_if_needed(materializer, kafka_config, topic, *partition)?;
        let next_offset = materializer
            .load_source_offset(topic, *partition)?
            .map(|offset| offset.saturating_add(1));
        seek_partition_offset(consumer, topic, *partition, next_offset)?;
    }
    *assigned = current;
    Ok(newly_assigned)
}

fn bootstrap_projection_partition_if_needed<S: aos_kernel::Store + 'static>(
    materializer: &mut Materializer<S>,
    kafka_config: &KafkaConfig,
    topic: &str,
    partition: u32,
) -> Result<(), HostedMaterializerError> {
    if topic != materializer.config().projection_topic {
        return Ok(());
    }
    if materializer.load_source_offset(topic, partition)?.is_some() {
        return Ok(());
    }

    let retained = fetch_partition_records(kafka_config, topic, partition as i32, None, true)
        .map_err(WorkerError::from)
        .map_err(HostedMaterializerError::Worker)?;
    let mut entries = Vec::with_capacity(retained.len());
    for record in retained {
        entries.push(decode_projection_fetched_record(partition, record)?);
    }
    let outcome = materializer.bootstrap_projection_partition(partition, &entries)?;
    tracing::info!(
        topic = %topic,
        partition,
        last_offset = ?outcome.last_offset,
        processed_entries = outcome.processed_entries,
        touched_worlds = outcome.touched_worlds,
        cells_materialized = outcome.cells_materialized,
        workspaces_materialized = outcome.workspaces_materialized,
        "materializer bootstrapped projection partition from retained topic state"
    );
    Ok(())
}

fn current_assignment(
    consumer: &BaseConsumer,
    topics: &[String],
) -> Result<BTreeSet<(String, u32)>, HostedMaterializerError> {
    let topic_set = topics.iter().cloned().collect::<BTreeSet<_>>();
    let assignment = consumer.assignment().map_err(kafka_consumer_err)?;
    Ok(assignment
        .elements()
        .iter()
        .filter(|entry| topic_set.contains(entry.topic()))
        .filter_map(|entry| {
            u32::try_from(entry.partition())
                .ok()
                .map(|partition| (entry.topic().to_owned(), partition))
        })
        .collect())
}

fn seek_partition_offset(
    consumer: &BaseConsumer,
    topic: &str,
    partition: u32,
    offset: Option<i64>,
) -> Result<(), HostedMaterializerError> {
    if offset.is_none() {
        return Ok(());
    }
    let offset = Offset::Offset(offset.expect("checked is_some above"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match consumer.seek(topic, partition as i32, offset, Duration::from_secs(1)) {
            Ok(()) => return Ok(()),
            Err(KafkaError::Seek(message))
                if message.contains("Erroneous state") && Instant::now() < deadline =>
            {
                let _ = consumer.poll(Duration::from_millis(50));
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(kafka_consumer_err(err)),
        }
    }
}

fn commit_partition_offset(
    consumer: &BaseConsumer,
    topic: &str,
    partition: u32,
    next_offset: u64,
) -> Result<(), HostedMaterializerError> {
    let mut offsets = TopicPartitionList::new();
    offsets
        .add_partition_offset(topic, partition as i32, Offset::Offset(next_offset as i64))
        .map_err(|err| kafka_plane_err("build materializer offset commit", err))?;
    consumer
        .commit(&offsets, CommitMode::Sync)
        .map_err(kafka_consumer_err)
}

fn decode_journal_entry(
    message: &rdkafka::message::BorrowedMessage<'_>,
) -> Result<Option<PartitionLogEntry>, HostedMaterializerError> {
    let payload = message.payload().ok_or_else(|| {
        WorkerError::Persist(PersistError::backend(format!(
            "journal message at partition {} offset {} had no payload",
            message.partition(),
            message.offset()
        )))
    })?;
    let Some(frame) = decode_journal_frame(payload).map_err(MaterializerError::from)? else {
        return Ok(None);
    };
    Ok(Some(PartitionLogEntry {
        offset: message.offset() as u64,
        frame,
    }))
}

fn decode_journal_frame(
    payload: &[u8],
) -> Result<Option<aos_node::WorldLogFrame>, serde_cbor::Error> {
    match serde_cbor::from_slice::<HostedJournalRecord>(payload) {
        Ok(HostedJournalRecord::Frame(frame)) => Ok(Some(frame)),
        Ok(HostedJournalRecord::Disposition(_)) => Ok(None),
        Err(_) => {
            if let Ok(value) = serde_cbor::from_slice::<serde_cbor::Value>(payload)
                && let Some(record) = decode_hosted_journal_value(value)?
            {
                return Ok(match record {
                    HostedJournalRecord::Frame(frame) => Some(frame),
                    HostedJournalRecord::Disposition(_) => None,
                });
            }
            Err(serde_cbor::from_slice::<HostedJournalRecord>(payload)
                .expect_err("already handled successful HostedJournalRecord decode"))
        }
    }
}

fn decode_hosted_journal_value(
    value: serde_cbor::Value,
) -> Result<Option<HostedJournalRecord>, serde_cbor::Error> {
    match value {
        serde_cbor::Value::Map(entries) if entries.len() == 1 => {
            let Some((serde_cbor::Value::Text(tag), value)) = entries.into_iter().next() else {
                return Ok(None);
            };
            decode_hosted_journal_tagged_value(&tag, value)
        }
        serde_cbor::Value::Array(mut values) if values.len() == 2 => {
            let tag = values.remove(0);
            let value = values.remove(0);
            let serde_cbor::Value::Text(tag) = tag else {
                return Ok(None);
            };
            decode_hosted_journal_tagged_value(&tag, value)
        }
        _ => Ok(None),
    }
}

fn decode_hosted_journal_tagged_value(
    tag: &str,
    value: serde_cbor::Value,
) -> Result<Option<HostedJournalRecord>, serde_cbor::Error> {
    match tag {
        "Frame" => serde_cbor::value::from_value::<aos_node::WorldLogFrame>(value)
            .map(HostedJournalRecord::Frame)
            .map(Some),
        "Disposition" => serde_cbor::value::from_value::<crate::kafka::DurableDisposition>(value)
            .map(HostedJournalRecord::Disposition)
            .map(Some),
        _ => Ok(None),
    }
}

fn decode_projection_entry(
    message: &rdkafka::message::BorrowedMessage<'_>,
) -> Result<(u64, ProjectionRecord), HostedMaterializerError> {
    decode_projection_parts(
        message.partition() as u32,
        message.offset() as u64,
        message.key().map(|key| key.to_vec()),
        message.payload().map(|payload| payload.to_vec()),
    )
}

fn decode_projection_fetched_record(
    partition: u32,
    record: FetchedRecord,
) -> Result<(u64, ProjectionRecord), HostedMaterializerError> {
    let offset = u64::try_from(record.offset).map_err(|_| {
        WorkerError::Persist(PersistError::backend(format!(
            "projection fetch returned negative offset for partition {partition}: {}",
            record.offset
        )))
    })?;
    decode_projection_parts(partition, offset, record.key, record.value)
}

fn decode_projection_parts(
    partition: u32,
    offset: u64,
    key: Option<Vec<u8>>,
    value: Option<Vec<u8>>,
) -> Result<(u64, ProjectionRecord), HostedMaterializerError> {
    let key = key.ok_or_else(|| {
        WorkerError::Persist(PersistError::backend(format!(
            "projection message at partition {partition} offset {offset} had no key",
        )))
    })?;
    let key: ProjectionKey = serde_cbor::from_slice(&key).map_err(MaterializerError::from)?;
    let value = value
        .as_deref()
        .map(serde_cbor::from_slice::<ProjectionValue>)
        .transpose()
        .map_err(MaterializerError::from)?;
    Ok((offset, ProjectionRecord { key, value }))
}

fn kafka_consumer_err(err: KafkaError) -> HostedMaterializerError {
    HostedMaterializerError::Worker(WorkerError::Persist(PersistError::backend(format!(
        "materializer Kafka consumer error: {err}"
    ))))
}

fn kafka_plane_err(label: &str, err: impl std::fmt::Display) -> HostedMaterializerError {
    HostedMaterializerError::Worker(WorkerError::Persist(PersistError::backend(format!(
        "{label}: {err}"
    ))))
}

#[cfg(test)]
mod tests {
    use aos_cbor::to_canonical_cbor;
    use aos_node::{SnapshotRecord, UniverseId, WorldId, WorldLogFrame};
    use uuid::Uuid;

    use super::{
        HostedMaterializerError, MaterializerError, decode_journal_frame, decode_projection_parts,
    };
    use crate::kafka::{HostedJournalRecord, ProjectionKey, ProjectionValue, WorldMetaProjection};

    fn sample_projection_key() -> ProjectionKey {
        ProjectionKey::WorldMeta {
            world_id: WorldId::new(Uuid::nil()),
        }
    }

    fn sample_projection_value() -> ProjectionValue {
        ProjectionValue::WorldMeta(WorldMetaProjection {
            universe_id: UniverseId::nil(),
            projection_token: "token-1".into(),
            world_epoch: 1,
            journal_head: 3,
            manifest_hash: "sha256:manifest".into(),
            active_baseline: SnapshotRecord {
                snapshot_ref: "sha256:snapshot".into(),
                height: 2,
                universe_id: UniverseId::nil(),
                logical_time_ns: 0,
                receipt_horizon_height: Some(2),
                manifest_hash: Some("sha256:manifest".into()),
            },
            updated_at_ns: 0,
        })
    }

    #[test]
    fn decode_projection_parts_accepts_projection_value_enum_bytes() {
        let key = serde_cbor::to_vec(&sample_projection_key()).expect("encode projection key");
        let value =
            serde_cbor::to_vec(&sample_projection_value()).expect("encode projection value");

        let (_, record) =
            decode_projection_parts(0, 0, Some(key), Some(value)).expect("decode projection");

        assert_eq!(record.key, sample_projection_key());
        assert_eq!(record.value, Some(sample_projection_value()));
    }

    #[test]
    fn decode_projection_parts_rejects_canonical_projection_value_bytes() {
        let key = serde_cbor::to_vec(&sample_projection_key()).expect("encode projection key");
        let value = to_canonical_cbor(&sample_projection_value())
            .expect("encode canonical projection value");

        let err = decode_projection_parts(0, 0, Some(key), Some(value)).unwrap_err();
        assert!(matches!(
            err,
            HostedMaterializerError::Materializer(MaterializerError::Cbor(_))
        ));
    }

    fn sample_frame() -> WorldLogFrame {
        WorldLogFrame {
            format_version: 1,
            universe_id: UniverseId::nil(),
            world_id: WorldId::new(Uuid::nil()),
            world_epoch: 1,
            world_seq_start: 1,
            world_seq_end: 1,
            records: Vec::new(),
        }
    }

    #[test]
    fn decode_journal_frame_accepts_wrapped_frame_payloads() {
        let payload = to_canonical_cbor(&HostedJournalRecord::Frame(sample_frame()))
            .expect("encode wrapped hosted journal frame");

        let decoded = decode_journal_frame(&payload)
            .expect("decode wrapped hosted journal frame")
            .expect("frame variant");

        assert_eq!(decoded, sample_frame());
    }

    #[test]
    fn decode_journal_frame_ignores_wrapped_dispositions() {
        let payload = to_canonical_cbor(&HostedJournalRecord::Disposition(
            crate::kafka::DurableDisposition::RejectedSubmission {
                partition: 0,
                offset: 0,
                world_id: WorldId::new(Uuid::nil()),
                reason: aos_node::SubmissionRejection::UnknownWorld,
            },
        ))
        .expect("encode wrapped hosted journal disposition");

        let decoded =
            decode_journal_frame(&payload).expect("decode wrapped hosted journal disposition");

        assert!(decoded.is_none());
    }

    #[test]
    fn decode_journal_frame_rejects_bare_frames() {
        let payload = to_canonical_cbor(&sample_frame()).expect("encode bare world log frame");

        let _err = decode_journal_frame(&payload).expect_err("bare frame should be rejected");
    }
}
