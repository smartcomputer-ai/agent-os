use std::collections::{BTreeMap, VecDeque};

use aos_node::{
    PlaneError, RejectedSubmission, SubmissionEnvelope, SubmissionPlane, WorldId,
    WorldLogAppendResult, WorldLogFrame, WorldLogPlane, partition_for_world,
};

use super::ProjectionRecord;
use super::local_state::{append_frame_locally, append_projection_locally};
use super::types::{
    CommitFailpoint, KafkaConfig, PartitionLogEntry, ProjectionTopicEntry, SubmissionBatch,
    SubmissionCommit,
};

#[derive(Debug)]
pub struct EmbeddedKafkaPlanes {
    partition_count: u32,
    config: KafkaConfig,
    pending_submissions: VecDeque<SubmissionEnvelope>,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    projection_logs: BTreeMap<(String, u32), Vec<ProjectionTopicEntry>>,
    rejected_submissions: Vec<RejectedSubmission>,
    next_submission_offset: u64,
    failpoint: Option<CommitFailpoint>,
}

impl EmbeddedKafkaPlanes {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, PlaneError> {
        if partition_count == 0 {
            return Err(PlaneError::InvalidPartitionCount);
        }
        Ok(Self {
            partition_count,
            config,
            pending_submissions: VecDeque::new(),
            world_frames: BTreeMap::new(),
            partition_logs: BTreeMap::new(),
            projection_logs: BTreeMap::new(),
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
        self.pending_submissions.len()
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
            let value = record.value.as_ref().map(serde_cbor::to_vec).transpose()?;
            append_projection_locally(
                &self.config.projection_topic,
                &mut self.projection_logs,
                partition,
                key,
                value,
                None,
            );
        }
        Ok(())
    }

    pub fn drain_partition_submissions(
        &mut self,
        partition: u32,
    ) -> Result<SubmissionBatch, PlaneError> {
        let mut matching = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(submission) = self.pending_submissions.pop_front() {
            let submission_partition =
                partition_for_world(submission.world_id, self.partition_count);
            if submission_partition == partition {
                matching.push(submission);
            } else {
                remaining.push_back(submission);
            }
        }

        self.pending_submissions = remaining;
        Ok(SubmissionBatch {
            submissions: matching,
            commit: SubmissionCommit::Embedded { partition },
        })
    }

    pub fn commit_submission_batch(
        &mut self,
        batch: SubmissionBatch,
        frames: Vec<WorldLogFrame>,
    ) -> Result<(), PlaneError> {
        let SubmissionCommit::Embedded { partition } = batch.commit else {
            unreachable!("embedded runtime received non-embedded commit handle");
        };
        if self.failpoint.take() == Some(CommitFailpoint::AbortBeforeCommit) {
            self.requeue_partition_submissions(partition, batch.submissions);
            return Err(PlaneError::Persist(aos_node::PersistError::backend(
                "embedded Kafka failpoint: abort before commit",
            )));
        }
        for frame in frames {
            let _ = append_frame_locally(
                &self.config.journal_topic,
                &mut self.world_frames,
                &mut self.partition_logs,
                self.partition_count,
                frame,
                None,
            )?;
        }
        Ok(())
    }

    pub fn recover_from_broker(&mut self) -> Result<(), PlaneError> {
        Ok(())
    }

    pub fn fail_next_batch_commit(&mut self) {
        self.failpoint = Some(CommitFailpoint::AbortBeforeCommit);
    }

    pub fn append_frame_transactional(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, PlaneError> {
        self.append_frame(frame)
    }

    fn requeue_partition_submissions(
        &mut self,
        _partition: u32,
        submissions: Vec<SubmissionEnvelope>,
    ) {
        for submission in submissions.into_iter().rev() {
            self.pending_submissions.push_front(submission);
        }
    }
}

impl SubmissionPlane for EmbeddedKafkaPlanes {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, PlaneError> {
        let offset = self.next_submission_offset;
        self.next_submission_offset = self.next_submission_offset.saturating_add(1);
        self.pending_submissions.push_back(submission);
        Ok(offset)
    }
}

impl WorldLogPlane for EmbeddedKafkaPlanes {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, PlaneError> {
        append_frame_locally(
            &self.config.journal_topic,
            &mut self.world_frames,
            &mut self.partition_logs,
            self.partition_count,
            frame,
            None,
        )
    }

    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame] {
        self.world_frames(world_id)
    }
}
