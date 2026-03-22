use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use aos_cbor::to_canonical_cbor;
use aos_node::{
    PlaneError, RejectedSubmission, SubmissionEnvelope, SubmissionPlane, WorldId,
    WorldLogAppendResult, WorldLogFrame, WorldLogPlane, partition_for_world,
};
use rdkafka::consumer::Consumer;
use rdkafka::message::Message;
use rdkafka::producer::Producer;
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;

use super::backend::{
    ConsumerHandle, ProducerHandle, await_delivery, create_direct_consumer, create_group_consumer,
    create_producer, fetch_partition_records, send_record, send_record_with_delivery,
    send_tombstone_with_delivery, topic_partitions,
};
use super::local_state::{append_frame_locally, append_projection_locally, world_key_bytes};
use super::projection::ProjectionRecord;
use super::types::{
    CommitFailpoint, KafkaConfig, PartitionLogEntry, ProjectionTopicEntry, QueuedSubmission,
    SubmissionBatch, SubmissionCommit,
};

pub struct BrokerKafkaPlanes {
    partition_count: u32,
    config: KafkaConfig,
    producer: ProducerHandle,
    tx_producers: BTreeMap<u32, ProducerHandle>,
    consumer: Option<ConsumerHandle>,
    assigned_partitions: BTreeSet<u32>,
    pending_submissions: BTreeMap<u32, VecDeque<QueuedSubmission>>,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    projection_logs: BTreeMap<(String, u32), Vec<ProjectionTopicEntry>>,
    recovered_journal_offsets: BTreeMap<u32, u64>,
    rejected_submissions: Vec<RejectedSubmission>,
    next_submission_offset: u64,
    failpoint: Option<CommitFailpoint>,
}

impl std::fmt::Debug for BrokerKafkaPlanes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerKafkaPlanes")
            .field("partition_count", &self.partition_count)
            .field("config", &self.config)
            .field("assigned_partitions", &self.assigned_partitions)
            .field("pending_submissions", &self.pending_submission_count())
            .field("world_frames", &self.world_frames.len())
            .field("partition_logs", &self.partition_logs.len())
            .field("rejected_submissions", &self.rejected_submissions.len())
            .finish()
    }
}

impl BrokerKafkaPlanes {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, PlaneError> {
        if partition_count == 0 {
            return Err(PlaneError::InvalidPartitionCount);
        }

        let producer = create_producer(&config, false, None)?;
        Ok(Self {
            partition_count,
            config,
            producer,
            tx_producers: BTreeMap::new(),
            consumer: None,
            assigned_partitions: BTreeSet::new(),
            pending_submissions: BTreeMap::new(),
            world_frames: BTreeMap::new(),
            partition_logs: BTreeMap::new(),
            projection_logs: BTreeMap::new(),
            recovered_journal_offsets: BTreeMap::new(),
            rejected_submissions: Vec::new(),
            next_submission_offset: 0,
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

    pub fn pending_submission_count(&self) -> usize {
        self.pending_submissions.values().map(VecDeque::len).sum()
    }

    fn uses_direct_assignment(&self) -> bool {
        !self.config.direct_assigned_partitions.is_empty()
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

    pub fn projection_entries(
        &self,
        projection_topic: &str,
        partition: u32,
    ) -> &[ProjectionTopicEntry] {
        self.projection_logs
            .get(&(projection_topic.to_owned(), partition))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn publish_projection_records(
        &mut self,
        records: Vec<ProjectionRecord>,
    ) -> Result<(), PlaneError> {
        for record in records {
            let partition = partition_for_world(record.key.world_id(), self.partition_count);
            let key = serde_cbor::to_vec(&record.key)?;
            let delivery = if let Some(value) = &record.value {
                let payload = serde_cbor::to_vec(value)?;
                send_record_with_delivery(
                    &self.config,
                    &self.producer,
                    &self.config.projection_topic,
                    partition as i32,
                    &key,
                    &payload,
                    "publish Kafka projection record",
                )?
            } else {
                send_tombstone_with_delivery(
                    &self.config,
                    &self.producer,
                    &self.config.projection_topic,
                    partition as i32,
                    &key,
                    "publish Kafka projection tombstone",
                )?
            };
            let (_delivery_partition, offset) =
                await_delivery(&self.config, delivery, "publish Kafka projection record")?;
            let offset = u64::try_from(offset).map_err(|_| {
                PlaneError::Persist(aos_node::PersistError::backend(format!(
                    "Kafka delivery offset for projection partition {partition} was negative: {offset}"
                )))
            })?;
            let value = record.value.as_ref().map(serde_cbor::to_vec).transpose()?;
            append_projection_locally(
                &self.config.projection_topic,
                &mut self.projection_logs,
                partition,
                key,
                value,
                Some(offset),
            );
        }
        Ok(())
    }

    pub fn drain_partition_submissions(
        &mut self,
        partition: u32,
    ) -> Result<SubmissionBatch, PlaneError> {
        let mut submissions = Vec::new();
        let mut last_offset = None;
        let queued = self.pending_submissions.entry(partition).or_default();
        while let Some(item) = queued.pop_front() {
            last_offset = Some(item.offset);
            submissions.push(item.submission);
        }

        Ok(SubmissionBatch {
            submissions,
            commit: if self.uses_direct_assignment() {
                SubmissionCommit::DirectKafka { partition }
            } else {
                SubmissionCommit::Kafka {
                    topic: self.config.ingress_topic.clone(),
                    partition,
                    last_offset,
                }
            },
        })
    }

    pub fn commit_submission_batch(
        &mut self,
        batch: SubmissionBatch,
        frames: Vec<WorldLogFrame>,
    ) -> Result<(), PlaneError> {
        let (partition, topic, last_offset, direct_mode, submissions) = match batch.commit {
            SubmissionCommit::Kafka {
                topic,
                partition,
                last_offset,
            } => (
                partition,
                Some(topic),
                last_offset,
                false,
                batch.submissions,
            ),
            SubmissionCommit::DirectKafka { partition } => {
                (partition, None, None, true, batch.submissions)
            }
            SubmissionCommit::Embedded { .. } => {
                unreachable!("broker runtime received non-broker commit handle")
            }
        };
        if !direct_mode && last_offset.is_none() {
            return Ok(());
        }
        let last_offset = last_offset.unwrap_or_default();
        let topic = topic.unwrap_or_default();

        if !direct_mode {
            self.ensure_consumer()?;
        }
        let tx_producer = self.tx_producer_for_partition(partition)?;
        tx_producer
            .begin_transaction()
            .map_err(|err| kafka_backend_err("begin Kafka transaction", err))?;

        let mut delivered_frames = Vec::with_capacity(frames.len());
        let result = (|| -> Result<(), PlaneError> {
            for frame in frames {
                let frame_partition = partition_for_world(frame.world_id, self.partition_count);
                let payload = to_canonical_cbor(&frame)?;
                let key = world_key_bytes(frame.world_id);
                let delivery = send_record_with_delivery(
                    &self.config,
                    &tx_producer,
                    &self.config.journal_topic,
                    frame_partition as i32,
                    &key,
                    &payload,
                    "publish Kafka world frame",
                )?;
                delivered_frames.push((frame, delivery));
            }

            if !direct_mode {
                let mut offsets = TopicPartitionList::new();
                offsets
                    .add_partition_offset(&topic, partition as i32, Offset::Offset(last_offset + 1))
                    .map_err(|err| kafka_backend_err("build transactional ingress offsets", err))?;
                let metadata = self.consumer()?.group_metadata().ok_or_else(|| {
                    PlaneError::Persist(aos_node::PersistError::backend(format!(
                        "missing Kafka consumer group metadata for partition {partition}"
                    )))
                })?;
                tx_producer
                    .send_offsets_to_transaction(
                        &offsets,
                        &metadata,
                        Timeout::After(Duration::from_millis(u64::from(
                            self.config.transaction_timeout_ms,
                        ))),
                    )
                    .map_err(|err| {
                        kafka_backend_err(
                            format!(
                                "send Kafka ingress offsets to transaction for partition {partition}"
                            ),
                            err,
                        )
                    })?;
            }

            if self.failpoint.take() == Some(CommitFailpoint::AbortBeforeCommit) {
                return Err(PlaneError::Persist(aos_node::PersistError::backend(
                    "broker Kafka failpoint: abort before commit",
                )));
            }

            tx_producer
                .commit_transaction(Timeout::After(Duration::from_millis(u64::from(
                    self.config.transaction_timeout_ms,
                ))))
                .map_err(|err| kafka_backend_err("commit Kafka transaction", err))?;
            let mut max_offset = self.recovered_journal_offsets.get(&partition).copied();
            for (frame, delivery) in delivered_frames {
                let (_delivery_partition, offset) =
                    await_delivery(&self.config, delivery, "publish Kafka world frame")?;
                let offset = u64::try_from(offset).map_err(|_| {
                    PlaneError::Persist(aos_node::PersistError::backend(format!(
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
                max_offset = Some(max_offset.map_or(offset, |current| current.max(offset)));
            }
            if let Some(max_offset) = max_offset {
                self.recovered_journal_offsets.insert(partition, max_offset);
            }
            Ok(())
        })();

        if let Err(err) = result {
            let _ = tx_producer.abort_transaction(Timeout::After(Duration::from_millis(
                u64::from(self.config.transaction_timeout_ms),
            )));
            if direct_mode {
                self.requeue_partition_submissions(partition, submissions);
            }
            return Err(err);
        }

        self.recover_partition_from_broker(partition)?;
        Ok(())
    }

    pub fn sync_assignments_and_poll(&mut self) -> Result<(Vec<u32>, Vec<u32>), PlaneError> {
        if self.uses_direct_assignment() {
            let previous = self.assigned_partitions.clone();
            self.ensure_consumer()?;
            self.poll_consumer_once(Duration::from_millis(0))?;
            while self.poll_consumer_once(Duration::from_millis(0))? {}
            self.assigned_partitions = self.config.direct_assigned_partitions.clone();
            let newly_assigned = self
                .assigned_partitions
                .difference(&previous)
                .copied()
                .collect::<Vec<_>>();
            let revoked = previous
                .difference(&self.assigned_partitions)
                .copied()
                .collect::<Vec<_>>();
            return Ok((newly_assigned, revoked));
        }
        self.ensure_consumer()?;
        let previous = self.assigned_partitions.clone();
        self.poll_consumer_once(Duration::from_millis(u64::from(
            self.config.group_poll_wait_ms,
        )))?;
        while self.poll_consumer_once(Duration::from_millis(0))? {}
        let assigned = self.current_assignment()?;
        let newly_assigned = assigned.difference(&previous).copied().collect::<Vec<_>>();
        let revoked = previous.difference(&assigned).copied().collect::<Vec<_>>();
        for partition in &revoked {
            self.pending_submissions.remove(partition);
        }
        self.assigned_partitions = assigned;
        Ok((newly_assigned, revoked))
    }

    pub fn assigned_partitions(&self) -> Vec<u32> {
        self.assigned_partitions.iter().copied().collect()
    }

    pub fn recover_partition_from_broker(&mut self, partition: u32) -> Result<(), PlaneError> {
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
            let frame: WorldLogFrame = serde_cbor::from_slice(&value)?;
            let _ = append_frame_locally(
                &self.config.journal_topic,
                &mut self.world_frames,
                &mut self.partition_logs,
                self.partition_count,
                frame,
                Some(offset),
            )?;
            self.recovered_journal_offsets.insert(partition, offset);
        }
        Ok(())
    }

    pub fn recover_from_broker(&mut self) -> Result<(), PlaneError> {
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
    ) -> Result<WorldLogAppendResult, PlaneError> {
        let partition = partition_for_world(frame.world_id, self.partition_count);
        let tx_producer = self.tx_producer_for_partition(partition)?;
        tx_producer
            .begin_transaction()
            .map_err(|err| kafka_backend_err("begin Kafka checkpoint transaction", err))?;

        let result = (|| -> Result<WorldLogAppendResult, PlaneError> {
            let payload = to_canonical_cbor(&frame)?;
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
                PlaneError::Persist(aos_node::PersistError::backend(format!(
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

    fn poll_consumer_once(&mut self, timeout: Duration) -> Result<bool, PlaneError> {
        self.ensure_consumer()?;
        let message = match self
            .consumer
            .as_ref()
            .expect("consumer initialized")
            .poll(timeout)
        {
            Some(Ok(message)) => message,
            Some(Err(rdkafka::error::KafkaError::PartitionEOF(_))) | None => {
                return Ok(false);
            }
            Some(Err(err)) => {
                return Err(kafka_backend_err("poll Kafka ingress consumer", err));
            }
        };
        let partition = message.partition();
        if partition < 0 {
            return Ok(false);
        }
        let payload = message.payload().ok_or_else(|| {
            PlaneError::Persist(aos_node::PersistError::backend(format!(
                "Kafka ingress message at partition {} offset {} had no payload",
                partition,
                message.offset()
            )))
        })?;
        let submission: SubmissionEnvelope = serde_cbor::from_slice(payload)?;
        self.pending_submissions
            .entry(partition as u32)
            .or_default()
            .push_back(QueuedSubmission {
                offset: message.offset(),
                submission,
            });
        Ok(true)
    }

    fn current_assignment(&self) -> Result<BTreeSet<u32>, PlaneError> {
        let assignment = self
            .consumer
            .as_ref()
            .ok_or_else(|| {
                PlaneError::Persist(aos_node::PersistError::backend(
                    "Kafka ingress consumer is not initialized".to_owned(),
                ))
            })?
            .assignment()
            .map_err(|err| kafka_backend_err("read Kafka consumer assignment", err))?;
        let partitions = assignment
            .elements()
            .iter()
            .filter(|entry| entry.topic() == self.config.ingress_topic)
            .filter_map(|entry| u32::try_from(entry.partition()).ok())
            .collect();
        Ok(partitions)
    }

    fn tx_producer_for_partition(&mut self, partition: u32) -> Result<ProducerHandle, PlaneError> {
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
            PlaneError::Persist(aos_node::PersistError::backend(format!(
                "missing transactional producer for partition {partition}"
            )))
        })
    }

    fn ensure_consumer(&mut self) -> Result<(), PlaneError> {
        if self.consumer.is_none() {
            self.consumer = Some(if self.uses_direct_assignment() {
                create_direct_consumer(&self.config, &self.config.direct_assigned_partitions)?
            } else {
                create_group_consumer(&self.config)?
            });
        }
        Ok(())
    }

    fn consumer(&mut self) -> Result<&ConsumerHandle, PlaneError> {
        self.ensure_consumer()?;
        self.consumer.as_ref().ok_or_else(|| {
            PlaneError::Persist(aos_node::PersistError::backend(
                "Kafka ingress consumer is not initialized".to_owned(),
            ))
        })
    }

    fn requeue_partition_submissions(
        &mut self,
        partition: u32,
        submissions: Vec<SubmissionEnvelope>,
    ) {
        let queued = self.pending_submissions.entry(partition).or_default();
        for submission in submissions.into_iter().rev() {
            queued.push_front(QueuedSubmission {
                offset: -1,
                submission,
            });
        }
    }
}

impl SubmissionPlane for BrokerKafkaPlanes {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, PlaneError> {
        let partition = partition_for_world(submission.world_id, self.partition_count);
        let payload = to_canonical_cbor(&submission)?;
        let key = world_key_bytes(submission.world_id);
        send_record(
            &self.config,
            &self.producer,
            &self.config.ingress_topic,
            partition as i32,
            &key,
            &payload,
            "publish Kafka submission",
        )?;
        let offset = self.next_submission_offset;
        self.next_submission_offset = self.next_submission_offset.saturating_add(1);
        Ok(offset)
    }
}

impl WorldLogPlane for BrokerKafkaPlanes {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, PlaneError> {
        let partition = partition_for_world(frame.world_id, self.partition_count);
        let payload = to_canonical_cbor(&frame)?;
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
            PlaneError::Persist(aos_node::PersistError::backend(format!(
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

fn kafka_backend_err<T: Into<String>>(label: T, err: impl std::fmt::Display) -> PlaneError {
    PlaneError::Persist(aos_node::PersistError::backend(format!(
        "{}: {err}",
        label.into()
    )))
}
