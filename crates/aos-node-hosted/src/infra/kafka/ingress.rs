use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use aos_node::{BackendError, PersistError};
use rdkafka::consumer::Consumer;
use rdkafka::message::Message;

use super::backend::{ConsumerHandle, create_direct_consumer, create_group_consumer};
use super::broker::{compute_ingress_flow_control, ingress_partition_list, kafka_backend_err};
use super::types::{
    AssignmentSync, IngressPollBatch, IngressRecord, KafkaConfig, SharedConsumerGroupMetadata,
};

pub struct BrokerKafkaIngress {
    config: KafkaConfig,
    consumer_group_metadata: SharedConsumerGroupMetadata,
    consumer: Option<ConsumerHandle>,
    assigned_partitions: BTreeSet<u32>,
    paused_partitions: BTreeSet<u32>,
}

impl std::fmt::Debug for BrokerKafkaIngress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerKafkaIngress")
            .field("config", &self.config)
            .field("consumer_initialized", &self.consumer.is_some())
            .field("assigned_partitions", &self.assigned_partitions)
            .field("paused_partitions", &self.paused_partitions)
            .finish()
    }
}

impl BrokerKafkaIngress {
    pub fn new(config: KafkaConfig, consumer_group_metadata: SharedConsumerGroupMetadata) -> Self {
        Self {
            config,
            consumer_group_metadata,
            consumer: None,
            assigned_partitions: BTreeSet::new(),
            paused_partitions: BTreeSet::new(),
        }
    }

    pub fn poll(
        &mut self,
        backlog_by_partition: &BTreeMap<u32, usize>,
    ) -> Result<IngressPollBatch, BackendError> {
        self.sync_ingress_backpressure(backlog_by_partition)?;
        self.ensure_consumer()?;

        let mut batch = IngressPollBatch::default();
        self.poll_consumer_once(
            Duration::from_millis(u64::from(self.config.group_poll_wait_ms)),
            backlog_by_partition,
            &mut batch.records,
        )?;

        while self.poll_consumer_once(
            Duration::from_millis(0),
            backlog_by_partition,
            &mut batch.records,
        )? {
            let buffered_by_partition =
                batch
                    .records
                    .iter()
                    .fold(BTreeMap::<u32, usize>::new(), |mut acc, record| {
                        *acc.entry(record.partition).or_default() += 1;
                        acc
                    });
            let max_pending = self.config.max_pending_ingress_per_partition;
            if buffered_by_partition.iter().any(|(partition, buffered)| {
                backlog_by_partition
                    .get(partition)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(*buffered)
                    >= max_pending
            }) {
                break;
            }
        }

        let previous = self.assigned_partitions.clone();
        let assigned = if self.uses_direct_assignment() {
            self.config.direct_assigned_partitions.clone()
        } else {
            self.current_assignment()?
        };
        let newly_assigned = assigned.difference(&previous).copied().collect::<Vec<_>>();
        let revoked = previous.difference(&assigned).copied().collect::<Vec<_>>();
        self.assigned_partitions = assigned;
        self.paused_partitions
            .retain(|partition| self.assigned_partitions.contains(partition));

        batch.assignment = AssignmentSync {
            assigned: self.assigned_partitions.iter().copied().collect(),
            newly_assigned,
            revoked,
        };
        batch
            .records
            .retain(|record| self.assigned_partitions.contains(&record.partition));
        Ok(batch)
    }

    pub fn assigned_partitions(&self) -> Vec<u32> {
        self.assigned_partitions.iter().copied().collect()
    }

    fn uses_direct_assignment(&self) -> bool {
        !self.config.direct_assigned_partitions.is_empty()
    }

    fn sync_ingress_backpressure(
        &mut self,
        backlog_by_partition: &BTreeMap<u32, usize>,
    ) -> Result<(), BackendError> {
        let max_pending = self.config.max_pending_ingress_per_partition;
        let assigned = if self.uses_direct_assignment() {
            self.config.direct_assigned_partitions.clone()
        } else {
            self.assigned_partitions.clone()
        };
        let (to_pause, to_resume, next_paused) = compute_ingress_flow_control(
            &assigned,
            &self.paused_partitions,
            backlog_by_partition,
            max_pending,
        );
        if assigned.is_empty() {
            self.paused_partitions = next_paused;
            return Ok(());
        }

        self.ensure_consumer()?;
        let consumer = self.consumer()?;
        if !to_pause.is_empty() {
            let paused = ingress_partition_list(&self.config.ingress_topic, &to_pause);
            consumer
                .pause(&paused)
                .map_err(|err| kafka_backend_err("pause Kafka ingress partitions", err))?;
        }
        if !to_resume.is_empty() {
            let resumed = ingress_partition_list(&self.config.ingress_topic, &to_resume);
            consumer
                .resume(&resumed)
                .map_err(|err| kafka_backend_err("resume Kafka ingress partitions", err))?;
        }
        self.paused_partitions = next_paused;
        Ok(())
    }

    fn poll_consumer_once(
        &mut self,
        timeout: Duration,
        backlog_by_partition: &BTreeMap<u32, usize>,
        records: &mut Vec<IngressRecord>,
    ) -> Result<bool, BackendError> {
        self.ensure_consumer()?;
        let message = match self
            .consumer
            .as_ref()
            .expect("consumer initialized")
            .poll(timeout)
        {
            Some(Ok(message)) => message,
            Some(Err(rdkafka::error::KafkaError::PartitionEOF(_))) | None => {
                self.refresh_group_metadata()?;
                return Ok(false);
            }
            Some(Err(err)) => {
                return Err(kafka_backend_err("poll Kafka ingress consumer", err));
            }
        };

        self.refresh_group_metadata()?;
        let partition = message.partition();
        if partition < 0 {
            return Ok(false);
        }
        let partition = partition as u32;

        let payload = message.payload().ok_or_else(|| {
            BackendError::Persist(PersistError::backend(format!(
                "Kafka ingress message at partition {} offset {} had no payload",
                partition,
                message.offset()
            )))
        })?;
        let envelope = serde_cbor::from_slice(payload)?;
        records.push(IngressRecord {
            partition,
            offset: message.offset(),
            envelope,
        });

        let current = records
            .iter()
            .filter(|record| record.partition == partition)
            .count()
            .saturating_add(
                backlog_by_partition
                    .get(&partition)
                    .copied()
                    .unwrap_or_default(),
            );
        Ok(current < self.config.max_pending_ingress_per_partition)
    }

    fn current_assignment(&self) -> Result<BTreeSet<u32>, BackendError> {
        let assignment = self
            .consumer
            .as_ref()
            .ok_or_else(|| {
                BackendError::Persist(PersistError::backend(
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

    fn refresh_group_metadata(&self) -> Result<(), BackendError> {
        let metadata = if self.uses_direct_assignment() {
            None
        } else {
            self.consumer
                .as_ref()
                .and_then(|consumer| consumer.group_metadata())
        };
        let mut slot = self.consumer_group_metadata.lock().map_err(|_| {
            BackendError::Persist(PersistError::backend(
                "Kafka consumer-group metadata mutex poisoned".to_owned(),
            ))
        })?;
        *slot = metadata;
        Ok(())
    }

    fn ensure_consumer(&mut self) -> Result<(), BackendError> {
        if self.consumer.is_none() {
            self.consumer = Some(if self.uses_direct_assignment() {
                create_direct_consumer(&self.config, &self.config.direct_assigned_partitions)?
            } else {
                create_group_consumer(&self.config)?
            });
            self.refresh_group_metadata()?;
        }
        Ok(())
    }

    fn consumer(&self) -> Result<&ConsumerHandle, BackendError> {
        self.consumer.as_ref().ok_or_else(|| {
            BackendError::Persist(PersistError::backend(
                "Kafka ingress consumer is not initialized".to_owned(),
            ))
        })
    }
}
