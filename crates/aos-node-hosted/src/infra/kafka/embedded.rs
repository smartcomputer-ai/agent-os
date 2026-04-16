use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use aos_node::{
    BackendError, RejectedSubmission, SubmissionBackend, SubmissionEnvelope, WorldId,
    WorldLogAppendResult, WorldLogBackend, WorldLogFrame, partition_for_world,
};
use tokio::sync::Notify;

use super::ProjectionRecord;
use super::local_state::{append_frame_locally, append_projection_locally};
use super::types::{
    CommitFailpoint, FlushCommit, IngressRecord, KafkaConfig, PartitionLogEntry,
    ProjectionTopicEntry, QueuedSubmission,
};

#[derive(Debug)]
pub struct EmbeddedKafkaBackend {
    partition_count: u32,
    config: KafkaConfig,
    pending_submissions: VecDeque<QueuedSubmission>,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    projection_logs: BTreeMap<(String, u32), Vec<ProjectionTopicEntry>>,
    rejected_submissions: Vec<RejectedSubmission>,
    next_submission_offset: u64,
    failpoint: Option<CommitFailpoint>,
    ingress_notify: Arc<Notify>,
}

impl EmbeddedKafkaBackend {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, BackendError> {
        if partition_count == 0 {
            return Err(BackendError::InvalidPartitionCount);
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
            ingress_notify: Arc::new(Notify::new()),
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

    pub fn ingress_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.ingress_notify)
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
    ) -> Result<(), BackendError> {
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

    pub fn drain_pending_ingress(&mut self, partition: u32) -> Vec<IngressRecord> {
        let mut matching = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(submission) = self.pending_submissions.pop_front() {
            let submission_partition =
                partition_for_world(submission.submission.world_id, self.partition_count);
            if submission_partition == partition {
                matching.push(IngressRecord {
                    partition,
                    offset: submission.offset,
                    envelope: submission.submission,
                });
            } else {
                remaining.push_back(submission);
            }
        }

        self.pending_submissions = remaining;
        matching
    }

    pub fn commit_flush_batch(&mut self, batch: FlushCommit) -> Result<(), BackendError> {
        if self.failpoint.take() == Some(CommitFailpoint::AbortBeforeCommit) {
            return Err(BackendError::Persist(aos_node::PersistError::backend(
                "embedded Kafka failpoint: abort before commit",
            )));
        }
        for frame in batch.frames {
            let _ = append_frame_locally(
                &self.config.journal_topic,
                &mut self.world_frames,
                &mut self.partition_logs,
                self.partition_count,
                frame,
                None,
            )?;
        }
        let _ = batch.dispositions;
        let _ = batch.offset_commits;
        Ok(())
    }

    pub fn recover_from_broker(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    pub fn fail_next_batch_commit(&mut self) {
        self.failpoint = Some(CommitFailpoint::AbortBeforeCommit);
    }

    pub fn append_frame_transactional(
        &mut self,
        frame: WorldLogFrame,
    ) -> Result<WorldLogAppendResult, BackendError> {
        self.append_frame(frame)
    }
}

impl SubmissionBackend for EmbeddedKafkaBackend {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, BackendError> {
        let offset = self.next_submission_offset;
        self.next_submission_offset = self.next_submission_offset.saturating_add(1);
        self.pending_submissions.push_back(QueuedSubmission {
            offset: offset as i64,
            submission,
        });
        self.ingress_notify.notify_one();
        Ok(offset)
    }
}

impl WorldLogBackend for EmbeddedKafkaBackend {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, BackendError> {
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
