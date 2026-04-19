use std::collections::BTreeMap;
use std::time::Duration;

use aos_cbor::to_canonical_cbor;
use aos_node::{
    BackendError, RejectedSubmission, WorldId, WorldLogAppendResult, WorldLogBackend,
    WorldLogFrame, partition_for_world,
};
use rdkafka::producer::Producer;
use rdkafka::util::Timeout;

use super::backend::{
    ProducerHandle, await_delivery, create_producer, fetch_partition_records,
    send_record_with_delivery, topic_partitions,
};
use super::local_state::{append_frame_locally, world_key_bytes};
use super::types::{
    CommitFailpoint, FlushCommit, HostedJournalRecord, KafkaConfig, PartitionLogEntry,
};

pub struct BrokerKafkaBackend {
    partition_count: u32,
    config: KafkaConfig,
    producer: ProducerHandle,
    shared_tx_producer: Option<ProducerHandle>,
    tx_producers: BTreeMap<u32, ProducerHandle>,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    recovered_journal_offsets: BTreeMap<u32, u64>,
    rejected_submissions: Vec<RejectedSubmission>,
    failpoint: Option<CommitFailpoint>,
}

impl std::fmt::Debug for BrokerKafkaBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerKafkaBackend")
            .field("partition_count", &self.partition_count)
            .field("config", &self.config)
            .field("world_frames", &self.world_frames.len())
            .field("partition_logs", &self.partition_logs.len())
            .field("rejected_submissions", &self.rejected_submissions.len())
            .finish()
    }
}

impl BrokerKafkaBackend {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, BackendError> {
        if partition_count == 0 {
            return Err(BackendError::InvalidPartitionCount);
        }

        let producer = create_producer(&config, false, None)?;
        Ok(Self {
            partition_count,
            config,
            producer,
            shared_tx_producer: None,
            tx_producers: BTreeMap::new(),
            world_frames: BTreeMap::new(),
            partition_logs: BTreeMap::new(),
            recovered_journal_offsets: BTreeMap::new(),
            rejected_submissions: Vec::new(),
            failpoint: None,
        })
    }

    pub fn partition_count(&self) -> u32 {
        self.partition_count
    }

    pub fn config(&self) -> &KafkaConfig {
        &self.config
    }

    pub fn world_ids(&self) -> Vec<WorldId> {
        self.world_frames.keys().copied().collect()
    }

    pub fn record_rejected(&mut self, rejected: RejectedSubmission) {
        self.rejected_submissions.push(rejected);
    }

    pub fn next_world_seq(&self, world_id: WorldId) -> u64 {
        self.world_frames
            .get(&world_id)
            .and_then(|frames| frames.last())
            .map(|frame| frame.world_seq_end.saturating_add(1))
            .unwrap_or(0)
    }

    pub fn partition_entries(&self, journal_topic: &str, partition: u32) -> &[PartitionLogEntry] {
        self.partition_logs
            .get(&(journal_topic.to_owned(), partition))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame] {
        self.world_frames
            .get(&world_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn commit_flush_batch(&mut self, batch: FlushCommit) -> Result<(), BackendError> {
        if batch.frames.is_empty() && batch.dispositions.is_empty() {
            return Ok(());
        }

        let tx_producer = self.shared_tx_producer()?;
        tx_producer
            .begin_transaction()
            .map_err(|err| kafka_backend_err("begin Kafka journal flush transaction", err))?;

        let result = (|| -> Result<(), BackendError> {
            let mut delivered_frames = Vec::with_capacity(batch.frames.len());
            for frame in batch.frames {
                let partition = partition_for_world(frame.world_id, self.partition_count);
                let payload = to_canonical_cbor(&HostedJournalRecord::Frame(frame.clone()))?;
                let key = world_key_bytes(frame.world_id);
                let delivery = send_record_with_delivery(
                    &self.config,
                    &tx_producer,
                    &self.config.journal_topic,
                    partition as i32,
                    &key,
                    &payload,
                    "publish Kafka world frame",
                )?;
                delivered_frames.push((frame, partition, delivery));
            }

            for disposition in batch.dispositions {
                let partition = disposition_partition(&disposition, self.partition_count);
                let key = disposition_key_bytes(&disposition);
                let payload = to_canonical_cbor(&HostedJournalRecord::Disposition(disposition))?;
                let _delivery = send_record_with_delivery(
                    &self.config,
                    &tx_producer,
                    &self.config.journal_topic,
                    partition as i32,
                    &key,
                    &payload,
                    "publish Kafka durable disposition",
                )?;
            }

            let _ = batch.offset_commits;

            if self.failpoint.take() == Some(CommitFailpoint::AbortBeforeCommit) {
                return Err(BackendError::Persist(aos_node::PersistError::backend(
                    "broker Kafka failpoint: abort before commit",
                )));
            }

            tx_producer
                .commit_transaction(Timeout::After(Duration::from_millis(u64::from(
                    self.config.transaction_timeout_ms,
                ))))
                .map_err(|err| kafka_backend_err("commit Kafka journal flush transaction", err))?;

            for (frame, partition, delivery) in delivered_frames {
                let (_delivery_partition, offset) =
                    await_delivery(&self.config, delivery, "publish Kafka world frame")?;
                let offset = u64::try_from(offset).map_err(|_| {
                    BackendError::Persist(aos_node::PersistError::backend(format!(
                        "Kafka delivery offset for partition {partition} was negative: {offset}"
                    )))
                })?;
                let _ = append_frame_locally(
                    &self.config.journal_topic,
                    &mut self.world_frames,
                    &mut self.partition_logs,
                    self.partition_count,
                    frame,
                    Some(offset),
                )?;
                self.recovered_journal_offsets
                    .entry(partition)
                    .and_modify(|current| *current = (*current).max(offset))
                    .or_insert(offset);
            }
            Ok(())
        })();

        if let Err(err) = result {
            let _ = tx_producer.abort_transaction(Timeout::After(Duration::from_millis(
                u64::from(self.config.transaction_timeout_ms),
            )));
            return Err(err);
        }

        Ok(())
    }

    pub fn recover_partition_from_broker(&mut self, partition: u32) -> Result<(), BackendError> {
        let records = fetch_partition_records(
            &self.config,
            &self.config.journal_topic,
            partition as i32,
            self.recovered_journal_offsets
                .get(&partition)
                .map(|offset| offset.saturating_add(1) as i64),
            true,
        )?;
        for record in records {
            let offset = record.offset as u64;
            let Some(value) = record.value else {
                continue;
            };
            match decode_hosted_journal_record(&value)? {
                HostedJournalRecord::Frame(frame) => {
                    let _ = append_frame_locally(
                        &self.config.journal_topic,
                        &mut self.world_frames,
                        &mut self.partition_logs,
                        self.partition_count,
                        frame,
                        Some(offset),
                    )?;
                }
                HostedJournalRecord::Disposition(_disposition) => {}
            }
            self.recovered_journal_offsets.insert(partition, offset);
        }
        Ok(())
    }

    pub fn recover_from_broker(&mut self) -> Result<(), BackendError> {
        self.world_frames.clear();
        self.partition_logs.clear();
        self.recovered_journal_offsets.clear();
        let journal_partitions =
            topic_partitions(&self.config, &self.producer, &self.config.journal_topic)?;
        for partition in journal_partitions {
            self.recover_partition_from_broker(partition as u32)?;
        }
        Ok(())
    }

    pub fn fail_next_batch_commit(&mut self) {
        self.failpoint = Some(CommitFailpoint::AbortBeforeCommit);
    }

    pub fn append_frame_transactional(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, BackendError> {
        let partition = partition_for_world(frame.world_id, self.partition_count);
        let tx_producer = self.tx_producer_for_partition(partition)?;
        tx_producer
            .begin_transaction()
            .map_err(|err| kafka_backend_err("begin Kafka checkpoint transaction", err))?;

        let result = (|| -> Result<WorldLogAppendResult, BackendError> {
            let payload = to_canonical_cbor(&HostedJournalRecord::Frame(frame.clone()))?;
            let key = world_key_bytes(frame.world_id);
            let delivery = send_record_with_delivery(
                &self.config,
                &tx_producer,
                &self.config.journal_topic,
                partition as i32,
                &key,
                &payload,
                "publish Kafka checkpoint frame",
            )?;
            tx_producer
                .commit_transaction(Timeout::After(Duration::from_millis(u64::from(
                    self.config.transaction_timeout_ms,
                ))))
                .map_err(|err| kafka_backend_err("commit Kafka checkpoint transaction", err))?;
            let (_delivery_partition, offset) =
                await_delivery(&self.config, delivery, "publish Kafka checkpoint frame")?;
            let offset = u64::try_from(offset).map_err(|_| {
                BackendError::Persist(aos_node::PersistError::backend(format!(
                    "Kafka delivery offset for partition {partition} was negative: {offset}"
                )))
            })?;
            let append = append_frame_locally(
                &self.config.journal_topic,
                &mut self.world_frames,
                &mut self.partition_logs,
                self.partition_count,
                frame,
                Some(offset),
            )?;
            self.recovered_journal_offsets.insert(partition, offset);
            Ok(append)
        })();

        if let Err(err) = result {
            let _ = tx_producer.abort_transaction(Timeout::After(Duration::from_millis(
                u64::from(self.config.transaction_timeout_ms),
            )));
            return Err(err);
        }

        result
    }

    fn tx_producer_for_partition(
        &mut self,
        partition: u32,
    ) -> Result<ProducerHandle, BackendError> {
        if !self.tx_producers.contains_key(&partition) {
            let producer = create_producer(&self.config, true, Some(partition))?;
            producer
                .init_transactions(Timeout::After(Duration::from_millis(u64::from(
                    self.config.transaction_timeout_ms,
                ))))
                .map_err(|err| {
                    kafka_backend_err(
                        format!("initialize Kafka transactions for partition {partition}"),
                        err,
                    )
                })?;
            self.tx_producers.insert(partition, producer);
        }
        self.tx_producers.get(&partition).cloned().ok_or_else(|| {
            BackendError::Persist(aos_node::PersistError::backend(format!(
                "missing transactional producer for partition {partition}"
            )))
        })
    }

    fn shared_tx_producer(&mut self) -> Result<ProducerHandle, BackendError> {
        if self.shared_tx_producer.is_none() {
            let producer = create_producer(&self.config, true, None)?;
            producer
                .init_transactions(Timeout::After(Duration::from_millis(u64::from(
                    self.config.transaction_timeout_ms,
                ))))
                .map_err(|err| {
                    kafka_backend_err("initialize Kafka journal flush transactions", err)
                })?;
            self.shared_tx_producer = Some(producer);
        }
        self.shared_tx_producer.clone().ok_or_else(|| {
            BackendError::Persist(aos_node::PersistError::backend(
                "missing shared Kafka journal transactional producer".to_owned(),
            ))
        })
    }
}

fn decode_hosted_journal_record(payload: &[u8]) -> Result<HostedJournalRecord, BackendError> {
    match serde_cbor::from_slice::<HostedJournalRecord>(payload) {
        Ok(record) => Ok(record),
        Err(_) => {
            if let Ok(value) = serde_cbor::from_slice::<serde_cbor::Value>(payload)
                && let Some(record) = decode_hosted_journal_value(value)?
            {
                return Ok(record);
            }
            Err(BackendError::from(
                serde_cbor::from_slice::<HostedJournalRecord>(payload)
                    .expect_err("already handled successful HostedJournalRecord decode"),
            ))
        }
    }
}

fn decode_hosted_journal_value(
    value: serde_cbor::Value,
) -> Result<Option<HostedJournalRecord>, BackendError> {
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
) -> Result<Option<HostedJournalRecord>, BackendError> {
    match tag {
        "Frame" => serde_cbor::value::from_value::<WorldLogFrame>(value)
            .map(HostedJournalRecord::Frame)
            .map(Some)
            .map_err(BackendError::from),
        "Disposition" => serde_cbor::value::from_value::<super::types::DurableDisposition>(value)
            .map(HostedJournalRecord::Disposition)
            .map(Some)
            .map_err(BackendError::from),
        _ => Ok(None),
    }
}

fn disposition_partition(
    disposition: &super::types::DurableDisposition,
    partition_count: u32,
) -> u32 {
    match disposition {
        super::types::DurableDisposition::RejectedSubmission { world_id, .. }
        | super::types::DurableDisposition::CommandFailure { world_id, .. } => {
            partition_for_world(*world_id, partition_count)
        }
    }
}

fn disposition_key_bytes(disposition: &super::types::DurableDisposition) -> Vec<u8> {
    match disposition {
        super::types::DurableDisposition::RejectedSubmission {
            partition,
            offset,
            world_id,
            ..
        } => format!(
            "reject:{world_id}:{}:{}",
            partition
                .map(|value| value.to_string())
                .unwrap_or_else(|| "direct".to_owned()),
            offset
                .map(|value| value.to_string())
                .unwrap_or_else(|| "direct".to_owned())
        )
        .into_bytes(),
        super::types::DurableDisposition::CommandFailure {
            partition,
            offset,
            world_id,
            command_id,
            ..
        } => format!(
            "command:{world_id}:{command_id}:{}:{}",
            partition
                .map(|value| value.to_string())
                .unwrap_or_else(|| "direct".to_owned()),
            offset
                .map(|value| value.to_string())
                .unwrap_or_else(|| "direct".to_owned())
        )
        .into_bytes(),
    }
}

impl WorldLogBackend for BrokerKafkaBackend {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, BackendError> {
        let partition = partition_for_world(frame.world_id, self.partition_count);
        let payload = to_canonical_cbor(&HostedJournalRecord::Frame(frame.clone()))?;
        let key = world_key_bytes(frame.world_id);
        let delivery = send_record_with_delivery(
            &self.config,
            &self.producer,
            &self.config.journal_topic,
            partition as i32,
            &key,
            &payload,
            "publish Kafka world frame",
        )?;
        let (_delivery_partition, offset) =
            await_delivery(&self.config, delivery, "publish Kafka world frame")?;
        let offset = u64::try_from(offset).map_err(|_| {
            BackendError::Persist(aos_node::PersistError::backend(format!(
                "Kafka delivery offset for partition {partition} was negative: {offset}"
            )))
        })?;
        let append = append_frame_locally(
            &self.config.journal_topic,
            &mut self.world_frames,
            &mut self.partition_logs,
            self.partition_count,
            frame,
            Some(offset),
        )?;
        self.recovered_journal_offsets.insert(partition, offset);
        Ok(append)
    }

    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame] {
        self.world_frames(world_id)
    }
}

pub(super) fn kafka_backend_err<T: Into<String>>(
    label: T,
    err: impl std::fmt::Display,
) -> BackendError {
    BackendError::Persist(aos_node::PersistError::backend(format!(
        "{}: {err}",
        label.into()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_flush_batch_with_only_offset_commits_is_a_noop() {
        let mut backend = BrokerKafkaBackend::new(
            1,
            KafkaConfig {
                bootstrap_servers: Some("localhost:9092".to_owned()),
                ..KafkaConfig::default()
            },
        )
        .expect("broker backend");
        backend
            .commit_flush_batch(FlushCommit {
                frames: Vec::new(),
                dispositions: Vec::new(),
                offset_commits: BTreeMap::from([(0_u32, 1_i64)]),
            })
            .expect("offset-only commit is ignored");
    }
}
