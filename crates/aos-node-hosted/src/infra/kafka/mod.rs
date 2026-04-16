mod backend;
mod broker;
mod embedded;
mod ingress;
mod local_state;
mod projection;
mod types;

use std::sync::Arc;

use aos_node::{
    BackendError, RejectedSubmission, SubmissionBackend, SubmissionEnvelope, WorldId,
    WorldLogAppendResult, WorldLogBackend, WorldLogFrame,
};
use tokio::sync::Notify;

pub use self::backend::fetch_partition_records;
use self::broker::BrokerKafkaBackend;
use self::embedded::EmbeddedKafkaBackend;
pub use self::ingress::BrokerKafkaIngress;

pub use self::types::{
    AssignmentSync, DurableDisposition, FetchedRecord, FlushCommit, HostedJournalRecord,
    IngressPollBatch, IngressRecord, KafkaConfig, PartitionLogEntry, ProjectionTopicEntry,
};
pub use projection::{
    CellProjectionUpsert, CellStateProjectionRecord, ProjectionKey, ProjectionRecord,
    ProjectionValue, WorkspaceProjectionUpsert, WorkspaceRegistryProjectionRecord,
    WorkspaceVersionProjectionRecord, WorldMetaProjection,
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

    pub fn projection_topic(&self) -> String {
        self.config().projection_topic.clone()
    }

    pub fn world_ids(&self) -> Vec<WorldId> {
        match self {
            Self::Embedded(inner) => inner.world_ids(),
            Self::Broker(inner) => inner.world_ids(),
        }
    }

    pub fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, BackendError> {
        match self {
            Self::Embedded(inner) => inner.submit(submission),
            Self::Broker(inner) => inner.submit(submission),
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

    pub fn projection_entries(
        &self,
        projection_topic: &str,
        partition: u32,
    ) -> &[ProjectionTopicEntry] {
        match self {
            Self::Embedded(inner) => inner.projection_entries(projection_topic, partition),
            Self::Broker(inner) => inner.projection_entries(projection_topic, partition),
        }
    }

    pub fn pending_submission_count(&self) -> usize {
        match self {
            Self::Embedded(inner) => inner.pending_submission_count(),
            Self::Broker(_) => 0,
        }
    }

    pub fn embedded_ingress_notify(&self) -> Option<Arc<Notify>> {
        match self {
            Self::Embedded(inner) => Some(inner.ingress_notify()),
            Self::Broker(_) => None,
        }
    }

    pub fn broker_ingress_driver(&self) -> Option<BrokerKafkaIngress> {
        match self {
            Self::Embedded(_) => None,
            Self::Broker(inner) => Some(inner.broker_ingress_driver()),
        }
    }

    pub fn record_rejected(&mut self, rejected: RejectedSubmission) {
        match self {
            Self::Embedded(inner) => inner.record_rejected(rejected),
            Self::Broker(inner) => inner.record_rejected(rejected),
        }
    }

    pub fn drain_pending_ingress(&mut self, partition: u32) -> Vec<IngressRecord> {
        match self {
            Self::Embedded(inner) => inner.drain_pending_ingress(partition),
            Self::Broker(_) => Vec::new(),
        }
    }

    pub fn commit_flush_batch(&mut self, batch: FlushCommit) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.commit_flush_batch(batch),
            Self::Broker(inner) => inner.commit_flush_batch(batch),
        }
    }

    pub fn publish_projection_records(
        &mut self,
        records: Vec<ProjectionRecord>,
    ) -> Result<(), BackendError> {
        match self {
            Self::Embedded(inner) => inner.publish_projection_records(records),
            Self::Broker(inner) => inner.publish_projection_records(records),
        }
    }

    pub fn debug_fail_next_batch_commit(&mut self) {
        match self {
            Self::Embedded(inner) => inner.fail_next_batch_commit(),
            Self::Broker(inner) => inner.fail_next_batch_commit(),
        }
    }
}
