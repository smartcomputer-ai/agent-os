mod backend;
mod broker;
mod embedded;
mod local_state;
mod projection;
mod types;

use aos_node::{
    PlaneError, RejectedSubmission, SubmissionEnvelope, SubmissionPlane, WorldId,
    WorldLogAppendResult, WorldLogFrame, WorldLogPlane,
};

use self::broker::BrokerKafkaPlanes;
use self::embedded::EmbeddedKafkaPlanes;

pub(crate) use self::backend::fetch_partition_records;
pub(crate) use self::types::FetchedRecord;
pub use self::types::{KafkaConfig, PartitionLogEntry, ProjectionTopicEntry, SubmissionBatch};
pub use projection::{
    CellProjectionUpsert, ProjectionKey, ProjectionRecord, ProjectionValue,
    WorkspaceProjectionUpsert, WorldMetaProjection,
};

#[derive(Debug)]
pub enum HostedKafkaBackend {
    Embedded(EmbeddedKafkaPlanes),
    Broker(BrokerKafkaPlanes),
}

impl HostedKafkaBackend {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, PlaneError> {
        if config
            .bootstrap_servers
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Ok(Self::Broker(BrokerKafkaPlanes::new(
                partition_count,
                config,
            )?));
        }
        Ok(Self::Embedded(EmbeddedKafkaPlanes::new(
            partition_count,
            config,
        )?))
    }

    pub fn new_embedded(partition_count: u32, config: KafkaConfig) -> Result<Self, PlaneError> {
        Ok(Self::Embedded(EmbeddedKafkaPlanes::new(
            partition_count,
            config,
        )?))
    }

    pub fn is_broker(&self) -> bool {
        matches!(self, Self::Broker(_))
    }

    pub fn recover_from_broker(&mut self) -> Result<(), PlaneError> {
        match self {
            Self::Embedded(inner) => inner.recover_from_broker(),
            Self::Broker(inner) => inner.recover_from_broker(),
        }
    }

    pub fn recover_partition_from_broker(&mut self, partition: u32) -> Result<(), PlaneError> {
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

    pub fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, PlaneError> {
        match self {
            Self::Embedded(inner) => inner.submit(submission),
            Self::Broker(inner) => inner.submit(submission),
        }
    }

    pub fn append_frame(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, PlaneError> {
        match self {
            Self::Embedded(inner) => inner.append_frame(frame),
            Self::Broker(inner) => inner.append_frame(frame),
        }
    }

    pub fn append_frame_transactional(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, PlaneError> {
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
            Self::Broker(inner) => inner.pending_submission_count(),
        }
    }

    pub fn sync_assignments_and_poll(&mut self) -> Result<(Vec<u32>, Vec<u32>), PlaneError> {
        match self {
            Self::Embedded(inner) => {
                let assigned = (0..inner.partition_count()).collect::<Vec<_>>();
                Ok((assigned, Vec::new()))
            }
            Self::Broker(inner) => inner.sync_assignments_and_poll(),
        }
    }

    pub fn assigned_partitions(&self) -> Vec<u32> {
        match self {
            Self::Embedded(inner) => (0..inner.partition_count()).collect(),
            Self::Broker(inner) => inner.assigned_partitions(),
        }
    }

    pub fn record_rejected(&mut self, rejected: RejectedSubmission) {
        match self {
            Self::Embedded(inner) => inner.record_rejected(rejected),
            Self::Broker(inner) => inner.record_rejected(rejected),
        }
    }

    pub fn drain_partition_submissions(
        &mut self,
        partition: u32,
    ) -> Result<SubmissionBatch, PlaneError> {
        match self {
            Self::Embedded(inner) => inner.drain_partition_submissions(partition),
            Self::Broker(inner) => inner.drain_partition_submissions(partition),
        }
    }

    pub fn commit_submission_batch(
        &mut self,
        batch: SubmissionBatch,
        frames: Vec<WorldLogFrame>,
    ) -> Result<(), PlaneError> {
        match self {
            Self::Embedded(inner) => inner.commit_submission_batch(batch, frames),
            Self::Broker(inner) => inner.commit_submission_batch(batch, frames),
        }
    }

    pub fn publish_projection_records(
        &mut self,
        records: Vec<ProjectionRecord>,
    ) -> Result<(), PlaneError> {
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
