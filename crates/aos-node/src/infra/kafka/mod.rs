mod backend;
mod broker;
mod embedded;
mod local_state;
mod types;

use std::collections::{BTreeMap, BTreeSet};

use aos_node::{
    BackendError, JournalBackend, JournalCommit, JournalDisposition, JournalFlush,
    JournalSourceAck, RejectedSubmission, WorldDurableHead, WorldId, WorldJournalCursor,
    WorldLogAppendResult, WorldLogBackend, WorldLogFrame, partition_for_world,
};

pub use self::backend::fetch_partition_records;
use self::broker::BrokerKafkaBackend;
use self::embedded::EmbeddedKafkaBackend;

pub use self::types::{
    DurableDisposition, FetchedRecord, FlushCommit, HostedJournalRecord, KafkaConfig,
    PartitionLogEntry,
};

#[derive(Debug)]
pub enum HostedKafkaBackend {
    Embedded(EmbeddedKafkaBackend),
    Broker(BrokerKafkaBackend),
}

impl HostedKafkaBackend {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, BackendError> {
        if config
            .bootstrap_servers
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::Broker(BrokerKafkaBackend::new(
                partition_count,
                config,
            )?));
        }
        Ok(Self::Embedded(EmbeddedKafkaBackend::new(
            partition_count,
            config,
        )?))
    }

    pub fn new_embedded(partition_count: u32, config: KafkaConfig) -> Result<Self, BackendError> {
        Ok(Self::Embedded(EmbeddedKafkaBackend::new(
            partition_count,
            config,
        )?))
    }

    pub fn is_broker(&self) -> bool {
        matches!(self, Self::Broker(_))
    }

    pub fn recover_from_broker(&mut self) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.recover_from_broker(),
            Self::Broker(inner) => inner.recover_from_broker(),
        }
    }

    pub fn recover_partition_from_broker(&mut self, partition: u32) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.recover_from_broker(),
            Self::Broker(inner) => inner.recover_partition_from_broker(partition),
        }
    }

    pub fn partition_count(&self) -> u32 {
        match self {
            Self::Embedded(inner) => inner.partition_count(),
            Self::Broker(inner) => inner.partition_count(),
        }
    }

    pub fn config(&self) -> &KafkaConfig {
        match self {
            Self::Embedded(inner) => inner.config(),
            Self::Broker(inner) => inner.config(),
        }
    }

    pub fn world_ids(&self) -> Vec<WorldId> {
        match self {
            Self::Embedded(inner) => inner.world_ids(),
            Self::Broker(inner) => inner.world_ids(),
        }
    }

    pub fn append_frame(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, BackendError> {
        match self {
            Self::Embedded(inner) => inner.append_frame(frame),
            Self::Broker(inner) => inner.append_frame(frame),
        }
    }

    pub fn append_frame_transactional(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, BackendError> {
        match self {
            Self::Embedded(inner) => inner.append_frame_transactional(frame),
            Self::Broker(inner) => inner.append_frame_transactional(frame),
        }
    }

    pub fn next_world_seq(&self, world_id: WorldId) -> u64 {
        match self {
            Self::Embedded(inner) => inner.next_world_seq(world_id),
            Self::Broker(inner) => inner.next_world_seq(world_id),
        }
    }

    pub fn partition_entries(&self, journal_topic: &str, partition: u32) -> &[PartitionLogEntry] {
        match self {
            Self::Embedded(inner) => inner.partition_entries(journal_topic, partition),
            Self::Broker(inner) => inner.partition_entries(journal_topic, partition),
        }
    }

    pub fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame] {
        match self {
            Self::Embedded(inner) => inner.world_frames(world_id),
            Self::Broker(inner) => inner.world_frames(world_id),
        }
    }

    pub fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<&WorldJournalCursor>,
    ) -> Vec<WorldLogFrame> {
        if let Some((journal_topic, partition, journal_offset)) =
            cursor.and_then(|cursor| match cursor {
                WorldJournalCursor::Kafka {
                    journal_topic,
                    partition,
                    journal_offset,
                } if journal_topic == &self.config().journal_topic
                    && *partition < self.partition_count() =>
                {
                    Some((journal_topic, *partition, *journal_offset))
                }
                _ => None,
            })
        {
            return self
                .partition_entries(journal_topic, partition)
                .iter()
                .filter(|entry| {
                    entry.offset > journal_offset
                        && entry.frame.world_id == world_id
                        && entry.frame.world_seq_end > after_world_seq
                })
                .map(|entry| entry.frame.clone())
                .collect();
        }
        self.world_frames(world_id)
            .iter()
            .filter(|frame| frame.world_seq_end > after_world_seq)
            .cloned()
            .collect()
    }

    pub fn record_rejected(&mut self, rejected: RejectedSubmission) {
        match self {
            Self::Embedded(inner) => inner.record_rejected(rejected),
            Self::Broker(inner) => inner.record_rejected(rejected),
        }
    }

    pub fn commit_flush_batch(&mut self, batch: FlushCommit) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.commit_flush_batch(batch),
            Self::Broker(inner) => inner.commit_flush_batch(batch),
        }
    }

    pub fn debug_fail_next_batch_commit(&mut self) {
        match self {
            Self::Embedded(inner) => inner.fail_next_batch_commit(),
            Self::Broker(inner) => inner.fail_next_batch_commit(),
        }
    }
}

impl JournalBackend for HostedKafkaBackend {
    fn refresh_all(&mut self) -> Result<(), BackendError> {
        self.recover_from_broker()
    }

    fn refresh_world(&mut self, world_id: WorldId) -> Result<(), BackendError> {
        let partition = partition_for_world(world_id, self.partition_count());
        self.recover_partition_from_broker(partition)
    }

    fn world_ids(&self) -> Vec<WorldId> {
        self.world_ids()
    }

    fn durable_head(&self, world_id: WorldId) -> Result<WorldDurableHead, BackendError> {
        Ok(WorldDurableHead {
            next_world_seq: self.next_world_seq(world_id),
        })
    }

    fn world_frames(&self, world_id: WorldId) -> Result<Vec<WorldLogFrame>, BackendError> {
        Ok(self.world_frames(world_id).to_vec())
    }

    fn world_tail_frames(
        &self,
        world_id: WorldId,
        after_world_seq: u64,
        cursor: Option<&WorldJournalCursor>,
    ) -> Result<Vec<WorldLogFrame>, BackendError> {
        Ok(self.world_tail_frames(world_id, after_world_seq, cursor))
    }

    fn commit_flush(&mut self, flush: JournalFlush) -> Result<JournalCommit, BackendError> {
        let touched_worlds = flush
            .frames
            .iter()
            .map(|frame| frame.world_id)
            .chain(flush.dispositions.iter().map(disposition_world_id))
            .collect::<BTreeSet<_>>();
        let commit = FlushCommit {
            frames: flush.frames,
            dispositions: flush
                .dispositions
                .into_iter()
                .map(kafka_disposition)
                .collect(),
            offset_commits: kafka_offset_commits(&flush.source_acks),
        };
        self.commit_flush_batch(commit)?;
        let journal_topic = self.config().journal_topic.clone();
        let world_cursors = touched_worlds
            .into_iter()
            .filter_map(|world_id| {
                let partition = partition_for_world(world_id, self.partition_count());
                self.partition_entries(&journal_topic, partition)
                    .last()
                    .map(|entry| {
                        (
                            world_id,
                            WorldJournalCursor::Kafka {
                                journal_topic: journal_topic.clone(),
                                partition,
                                journal_offset: entry.offset,
                            },
                        )
                    })
            })
            .collect::<BTreeMap<_, _>>();
        Ok(JournalCommit { world_cursors })
    }
}

fn disposition_world_id(disposition: &JournalDisposition) -> WorldId {
    match disposition {
        JournalDisposition::RejectedSubmission { world_id, .. }
        | JournalDisposition::CommandFailure { world_id, .. } => *world_id,
    }
}

fn kafka_disposition(disposition: JournalDisposition) -> DurableDisposition {
    match disposition {
        JournalDisposition::RejectedSubmission {
            source_ack,
            world_id,
            reason,
        } => DurableDisposition::RejectedSubmission {
            partition: source_ack.and_then(journal_offset_partition),
            offset: source_ack.and_then(journal_offset_value),
            world_id,
            reason,
        },
        JournalDisposition::CommandFailure {
            source_ack,
            world_id,
            command_id,
            error_code,
        } => DurableDisposition::CommandFailure {
            partition: source_ack.and_then(journal_offset_partition),
            offset: source_ack.and_then(journal_offset_value),
            world_id,
            command_id,
            error_code,
        },
    }
}

fn kafka_offset_commits(acks: &[JournalSourceAck]) -> BTreeMap<u32, i64> {
    let mut commits = BTreeMap::new();
    for ack in acks {
        let JournalSourceAck::PartitionOffset { partition, offset } = *ack;
        commits
            .entry(partition)
            .and_modify(|last_offset: &mut i64| *last_offset = (*last_offset).max(offset))
            .or_insert(offset);
    }
    commits
}

fn journal_offset_partition(ack: JournalSourceAck) -> Option<u32> {
    match ack {
        JournalSourceAck::PartitionOffset { partition, .. } => Some(partition),
    }
}

fn journal_offset_value(ack: JournalSourceAck) -> Option<i64> {
    match ack {
        JournalSourceAck::PartitionOffset { offset, .. } => Some(offset),
    }
}
