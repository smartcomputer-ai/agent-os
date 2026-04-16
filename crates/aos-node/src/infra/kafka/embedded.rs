use std::collections::BTreeMap;

use aos_node::{
    BackendError, RejectedSubmission, WorldId, WorldLogAppendResult, WorldLogBackend, WorldLogFrame,
};

use super::local_state::append_frame_locally;
use super::types::{CommitFailpoint, FlushCommit, KafkaConfig, PartitionLogEntry};

#[derive(Debug)]
pub struct EmbeddedKafkaBackend {
    partition_count: u32,
    config: KafkaConfig,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    rejected_submissions: Vec<RejectedSubmission>,
    failpoint: Option<CommitFailpoint>,
}

impl EmbeddedKafkaBackend {
    pub fn new(partition_count: u32, config: KafkaConfig) -> Result<Self, BackendError> {
        if partition_count == 0 {
            return Err(BackendError::InvalidPartitionCount);
        }
        Ok(Self {
            partition_count,
            config,
            world_frames: BTreeMap::new(),
            partition_logs: BTreeMap::new(),
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
