use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::journal::{Journal, OwnedJournalEntry};
use aos_kernel::{KernelConfig, LoadedManifest, ManifestLoader, MemStore, Store};
use aos_runtime::{WorldConfig, WorldHost};

use crate::{CreateWorldRequest, UniverseId, WorldId, validate_create_world_request};

use super::decode::{
    latest_plane_snapshot_record, parse_plane_hash_like, resolve_plane_cbor_payload,
    submission_payload_to_external_event,
};
use super::govern::run_governance_plane_command;
use super::model::{
    CanonicalWorldRecord, DEFAULT_JOURNAL_TOPIC, PartitionCheckpoint, PromotableBaselineRef,
    RegisteredWorldSummary, RejectedSubmission, SubmissionEnvelope, SubmissionPayload,
    SubmissionRejection, WorldCheckpointRef, WorldLogFrame, partition_for_world,
};
use super::traits::{
    BlobPlane, CheckpointPlane, PlaneError, SubmissionPlane, WorldLogAppendResult, WorldLogPlane,
};
use super::world::create_plane_world_from_request;

struct RegisteredWorld {
    universe_id: UniverseId,
    store: Arc<MemStore>,
    loaded: LoadedManifest,
    host: WorldHost<MemStore>,
    world_epoch: u64,
    accepted_submission_ids: BTreeSet<String>,
}

struct QueuedSubmission {
    submission: SubmissionEnvelope,
}

#[derive(Debug, Clone)]
struct PartitionLogEntry {
    offset: u64,
    frame: WorldLogFrame,
}

pub struct MemoryLogRuntime {
    partition_count: u32,
    universe_stores: BTreeMap<UniverseId, Arc<MemStore>>,
    worlds: BTreeMap<WorldId, RegisteredWorld>,
    pending_submissions: VecDeque<QueuedSubmission>,
    world_frames: BTreeMap<WorldId, Vec<WorldLogFrame>>,
    partition_logs: BTreeMap<(String, u32), Vec<PartitionLogEntry>>,
    latest_checkpoints: BTreeMap<(String, u32), PartitionCheckpoint>,
    rejected_submissions: Vec<RejectedSubmission>,
    next_submission_offset: u64,
}

impl MemoryLogRuntime {
    pub fn new(partition_count: u32) -> Result<Self, PlaneError> {
        if partition_count == 0 {
            return Err(PlaneError::InvalidPartitionCount);
        }
        Ok(Self {
            partition_count,
            universe_stores: BTreeMap::new(),
            worlds: BTreeMap::new(),
            pending_submissions: VecDeque::new(),
            world_frames: BTreeMap::new(),
            partition_logs: BTreeMap::new(),
            latest_checkpoints: BTreeMap::new(),
            rejected_submissions: Vec::new(),
            next_submission_offset: 0,
        })
    }

    pub fn partition_count(&self) -> u32 {
        self.partition_count
    }

    pub fn register_world(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
        store: Arc<MemStore>,
        loaded: LoadedManifest,
    ) -> Result<u64, PlaneError> {
        let host = WorldHost::from_loaded_manifest_with_journal_replay(
            store.clone(),
            loaded.clone(),
            Journal::new(),
            WorldConfig::default(),
            EffectAdapterConfig::default(),
            KernelConfig::default(),
            None,
        )?;
        let world_epoch = 1;
        self.universe_stores
            .entry(universe_id)
            .or_insert(store.clone());
        self.worlds.insert(
            world_id,
            RegisteredWorld {
                universe_id,
                store,
                loaded,
                host,
                world_epoch,
                accepted_submission_ids: BTreeSet::new(),
            },
        );
        self.world_frames.entry(world_id).or_default();
        Ok(world_epoch)
    }

    pub fn effective_partition_for(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<u32, PlaneError> {
        let _ = universe_id;
        if !self.worlds.contains_key(&world_id) {
            return Err(PlaneError::UnknownWorld {
                universe_id,
                world_id,
            });
        }
        Ok(partition_for_world(world_id, self.partition_count))
    }

    pub fn rejected_submissions(&self) -> &[RejectedSubmission] {
        &self.rejected_submissions
    }

    pub fn registered_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<RegisteredWorldSummary> {
        let _ = universe_id;
        let world = self.worlds.get(&world_id)?;
        let next_world_seq = self
            .world_frames
            .get(&world_id)
            .and_then(|frames| frames.last())
            .map(|frame| frame.world_seq_end.saturating_add(1))
            .unwrap_or(0);
        Some(RegisteredWorldSummary {
            universe_id: world.universe_id,
            world_id,
            world_epoch: world.world_epoch,
            effective_partition: partition_for_world(world_id, self.partition_count),
            manifest_hash: world.host.kernel().manifest_hash().to_hex(),
            next_world_seq,
        })
    }

    pub fn registered_worlds(&self) -> Vec<RegisteredWorldSummary> {
        let mut summaries = self
            .worlds
            .keys()
            .filter_map(|&world_id| self.registered_world(UniverseId::nil(), world_id))
            .collect::<Vec<_>>();
        summaries.sort_by_key(|summary| summary.world_id);
        summaries
    }

    pub fn world_host(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Option<&WorldHost<MemStore>> {
        let _ = universe_id;
        self.worlds.get(&world_id).map(|world| &world.host)
    }

    pub fn pending_submission_count(&self) -> usize {
        self.pending_submissions.len()
    }

    pub fn bump_world_epoch(
        &mut self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<u64, PlaneError> {
        let world = self
            .worlds
            .get_mut(&world_id)
            .ok_or(PlaneError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        world.world_epoch = world.world_epoch.saturating_add(1);
        Ok(world.world_epoch)
    }

    pub fn process_partition(&mut self, partition: u32) -> Result<Vec<WorldLogFrame>, PlaneError> {
        let mut frames = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(queued) = self.pending_submissions.pop_front() {
            let known_world = self.worlds.contains_key(&queued.submission.world_id);
            if !known_world
                && !matches!(
                    &queued.submission.payload,
                    SubmissionPayload::CreateWorld { .. }
                )
            {
                return Err(PlaneError::UnknownWorld {
                    universe_id: queued.submission.universe_id,
                    world_id: queued.submission.world_id,
                });
            }
            let submission_partition =
                partition_for_world(queued.submission.world_id, self.partition_count);
            if submission_partition != partition {
                remaining.push_back(queued);
                continue;
            }

            if let Some(frame) = self.process_submission(queued.submission)? {
                self.append_frame(frame.clone())?;
                frames.push(frame);
            }
        }

        self.pending_submissions = remaining;
        Ok(frames)
    }

    pub fn create_partition_checkpoint(
        &mut self,
        partition: u32,
        created_at_ns: u64,
    ) -> Result<PartitionCheckpoint, PlaneError> {
        let mut world_keys: Vec<_> = self
            .worlds
            .iter()
            .filter_map(|(&world_id, world)| {
                let effective = partition_for_world(world_id, self.partition_count);
                (effective == partition).then_some((world.universe_id, world_id, world.world_epoch))
            })
            .collect();
        world_keys.sort_by_key(|(universe_id, world_id, _)| (*universe_id, *world_id));

        let mut worlds = Vec::new();
        let mut compaction_targets = Vec::new();
        let mut last_journal_offset = None;

        for (universe_id, world_id, world_epoch) in world_keys {
            let next_world_seq = self
                .world_frames
                .get(&world_id)
                .and_then(|frames| frames.last())
                .map(|frame| frame.world_seq_end.saturating_add(1))
                .unwrap_or(0);

            let (frame, snapshot_record, manifest_hash) = {
                let world = self
                    .worlds
                    .get_mut(&world_id)
                    .ok_or(PlaneError::UnknownWorld {
                        universe_id,
                        world_id,
                    })?;
                let journal_tail_start = world.host.journal_bounds().next_seq;
                world.host.snapshot()?;
                let tail = world.host.kernel().dump_journal_from(journal_tail_start)?;
                if tail.is_empty() {
                    continue;
                }

                let mut records = Vec::with_capacity(tail.len());
                for entry in &tail {
                    let record: CanonicalWorldRecord = serde_cbor::from_slice(&entry.payload)?;
                    records.push(record);
                }

                let frame = WorldLogFrame {
                    format_version: 1,
                    universe_id,
                    world_id,
                    world_epoch,
                    world_seq_start: next_world_seq,
                    world_seq_end: next_world_seq + records.len() as u64 - 1,
                    records,
                };
                let snapshot_record = latest_plane_snapshot_record(&tail).ok_or_else(|| {
                    PlaneError::Kernel(aos_kernel::KernelError::SnapshotUnavailable(
                        "checkpoint snapshot did not emit a snapshot record".into(),
                    ))
                })?;
                (
                    frame,
                    snapshot_record,
                    world.host.kernel().manifest_hash().to_hex(),
                )
            };

            let frame_world_seq_end = frame.world_seq_end;
            let append = self.append_frame(frame)?;
            last_journal_offset = Some(append.journal_offset);
            compaction_targets.push((universe_id, world_id, snapshot_record.height));

            worlds.push(WorldCheckpointRef {
                universe_id,
                world_id,
                world_epoch,
                checkpointed_at_ns: created_at_ns,
                world_seq: frame_world_seq_end,
                baseline: PromotableBaselineRef {
                    snapshot_ref: snapshot_record.snapshot_ref,
                    snapshot_manifest_ref: None,
                    manifest_hash: snapshot_record.manifest_hash.unwrap_or(manifest_hash),
                    height: snapshot_record.height,
                    universe_id: snapshot_record.universe_id.into(),
                    logical_time_ns: snapshot_record.logical_time_ns,
                    receipt_horizon_height: snapshot_record
                        .receipt_horizon_height
                        .unwrap_or(snapshot_record.height),
                },
            });
        }

        let checkpoint = PartitionCheckpoint {
            journal_topic: DEFAULT_JOURNAL_TOPIC.into(),
            partition,
            journal_offset: last_journal_offset.unwrap_or(0),
            created_at_ns,
            worlds,
        };
        self.commit_checkpoint(checkpoint.clone())?;
        for (universe_id, world_id, height) in compaction_targets {
            let world = self
                .worlds
                .get_mut(&world_id)
                .ok_or(PlaneError::UnknownWorld {
                    universe_id,
                    world_id,
                })?;
            world.host.compact_journal_through(height)?;
        }
        Ok(checkpoint)
    }

    pub fn replay_world(
        &self,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldHost<MemStore>, PlaneError> {
        let world = self.worlds.get(&world_id).ok_or(PlaneError::UnknownWorld {
            universe_id,
            world_id,
        })?;
        let mut entries = Vec::new();
        let mut expected_seq = 0u64;

        for frame in self.world_frames(world_id) {
            if frame.world_seq_start != expected_seq {
                return Err(PlaneError::NonContiguousWorldSeq {
                    universe_id,
                    world_id,
                    expected: expected_seq,
                    actual: frame.world_seq_start,
                });
            }
            for (offset, record) in frame.records.iter().enumerate() {
                entries.push(OwnedJournalEntry {
                    seq: frame.world_seq_start + offset as u64,
                    kind: record.kind(),
                    payload: serde_cbor::to_vec(record)?,
                });
            }
            expected_seq = frame.world_seq_end.saturating_add(1);
        }

        WorldHost::from_loaded_manifest_with_journal_replay(
            world.store.clone(),
            world.loaded.clone(),
            Journal::from_entries(&entries).map_err(|err| {
                PlaneError::Host(aos_runtime::HostError::External(err.to_string()))
            })?,
            WorldConfig::default(),
            EffectAdapterConfig::default(),
            KernelConfig::default(),
            None,
        )
        .map_err(PlaneError::from)
    }

    pub fn replay_world_from_checkpoint(
        &self,
        checkpoint: &PartitionCheckpoint,
        universe_id: UniverseId,
        world_id: WorldId,
    ) -> Result<WorldHost<MemStore>, PlaneError> {
        let checkpoint_world = checkpoint
            .worlds
            .iter()
            .find(|world| world.world_id == world_id)
            .ok_or(PlaneError::UnknownWorld {
                universe_id,
                world_id,
            })?;
        let world = self.worlds.get(&world_id).ok_or(PlaneError::UnknownWorld {
            universe_id,
            world_id,
        })?;
        let partition_entries = self
            .partition_logs
            .get(&(checkpoint.journal_topic.clone(), checkpoint.partition))
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let mut replay_entries = Vec::new();
        for entry in partition_entries
            .iter()
            .filter(|entry| entry.offset > checkpoint.journal_offset)
        {
            if entry.frame.universe_id != universe_id || entry.frame.world_id != world_id {
                continue;
            }
            for (offset, record) in entry.frame.records.iter().enumerate() {
                replay_entries.push(OwnedJournalEntry {
                    seq: entry.frame.world_seq_start + offset as u64,
                    kind: record.kind(),
                    payload: serde_cbor::to_vec(record)?,
                });
            }
        }

        let replay = aos_runtime::JournalReplayOpen {
            active_baseline: aos_kernel::journal::SnapshotRecord {
                snapshot_ref: checkpoint_world.baseline.snapshot_ref.clone(),
                height: checkpoint_world.baseline.height,
                universe_id: checkpoint_world.baseline.universe_id.as_uuid(),
                logical_time_ns: checkpoint_world.baseline.logical_time_ns,
                receipt_horizon_height: Some(checkpoint_world.baseline.receipt_horizon_height),
                manifest_hash: Some(checkpoint_world.baseline.manifest_hash.clone()),
            },
            replay_seed: None,
        };

        WorldHost::from_loaded_manifest_with_journal_replay(
            world.store.clone(),
            world.loaded.clone(),
            Journal::from_entries(&replay_entries).map_err(|err| {
                PlaneError::Host(aos_runtime::HostError::External(err.to_string()))
            })?,
            WorldConfig::default(),
            EffectAdapterConfig::default(),
            KernelConfig::default(),
            Some(replay),
        )
        .map_err(PlaneError::from)
    }

    fn process_submission(
        &mut self,
        submission: SubmissionEnvelope,
    ) -> Result<Option<WorldLogFrame>, PlaneError> {
        if let SubmissionPayload::CreateWorld { request } = submission.payload.clone() {
            return self.process_create_world_submission(submission, request);
        }

        let expected_world_epoch = match self.worlds.get(&submission.world_id) {
            Some(world) => world.world_epoch,
            None => {
                self.rejected_submissions.push(RejectedSubmission {
                    submission,
                    reason: SubmissionRejection::UnknownWorld,
                });
                return Ok(None);
            }
        };
        if submission.world_epoch != expected_world_epoch {
            let got = submission.world_epoch;
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::WorldEpochMismatch {
                    expected: expected_world_epoch,
                    got,
                },
            });
            return Ok(None);
        }

        let next_world_seq = self
            .world_frames
            .get(&submission.world_id)
            .and_then(|frames| frames.last())
            .map(|frame| frame.world_seq_end.saturating_add(1))
            .unwrap_or(0);

        let world = self
            .worlds
            .get_mut(&submission.world_id)
            .ok_or(PlaneError::UnknownWorld {
                universe_id: submission.universe_id,
                world_id: submission.world_id,
            })?;
        if !world
            .accepted_submission_ids
            .insert(submission.submission_id.clone())
        {
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::DuplicateSubmissionId,
            });
            return Ok(None);
        }

        let journal_tail_start = world.host.journal_bounds().next_seq;
        let process_result = match &submission.payload {
            SubmissionPayload::Command { command } => {
                let payload = resolve_plane_cbor_payload(world.store.as_ref(), &command.payload)?;
                run_governance_plane_command(&mut world.host, command, &payload)
            }
            _ => {
                let external_event = submission_payload_to_external_event(
                    world.store.as_ref(),
                    &submission.payload,
                )?;
                world.host.enqueue_external(external_event)?;
                world.host.drain().map(|_| ()).map_err(PlaneError::from)
            }
        };
        if let Err(err) = process_result {
            world
                .accepted_submission_ids
                .remove(&submission.submission_id);
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::InvalidSubmission {
                    message: err.to_string(),
                },
            });
            return Ok(None);
        }
        let tail = world.host.kernel().dump_journal_from(journal_tail_start)?;
        if tail.is_empty() {
            return Ok(None);
        }

        let mut records = Vec::with_capacity(tail.len());
        for entry in tail {
            let record: CanonicalWorldRecord = serde_cbor::from_slice(&entry.payload)?;
            records.push(record);
        }

        let world_seq_start = next_world_seq;
        let world_seq_end = world_seq_start + records.len() as u64 - 1;
        Ok(Some(WorldLogFrame {
            format_version: 1,
            universe_id: world.universe_id,
            world_id: submission.world_id,
            world_epoch: expected_world_epoch,
            world_seq_start,
            world_seq_end,
            records,
        }))
    }

    fn process_create_world_submission(
        &mut self,
        submission: SubmissionEnvelope,
        request: CreateWorldRequest,
    ) -> Result<Option<WorldLogFrame>, PlaneError> {
        if self.worlds.contains_key(&submission.world_id) {
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::WorldAlreadyExists,
            });
            return Ok(None);
        }

        if let Err(err) = validate_create_world_request(&request) {
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::InvalidSubmission {
                    message: err.to_string(),
                },
            });
            return Ok(None);
        }

        if request.world_id != Some(submission.world_id) {
            self.rejected_submissions.push(RejectedSubmission {
                submission,
                reason: SubmissionRejection::InvalidSubmission {
                    message: "create-world submission world_id mismatch".into(),
                },
            });
            return Ok(None);
        }

        let store = self
            .universe_stores
            .entry(request.universe_id)
            .or_insert_with(|| Arc::new(MemStore::new()))
            .clone();
        let created = create_plane_world_from_request(
            store.clone(),
            &request,
            request.universe_id,
            submission.world_id,
            1,
            WorldConfig::default(),
            EffectAdapterConfig::default(),
            KernelConfig::default(),
        )?;
        let loaded = ManifestLoader::load_from_hash(
            store.as_ref(),
            parse_plane_hash_like(&created.initial_manifest_hash, "manifest_hash")?,
        )
        .map_err(PlaneError::Kernel)?;

        let mut accepted_submission_ids = BTreeSet::new();
        accepted_submission_ids.insert(submission.submission_id.clone());
        self.worlds.insert(
            submission.world_id,
            RegisteredWorld {
                universe_id: request.universe_id,
                store,
                loaded,
                host: created.host,
                world_epoch: 1,
                accepted_submission_ids,
            },
        );
        self.world_frames.entry(submission.world_id).or_default();

        Ok(created.initial_frame)
    }
}

impl BlobPlane for MemoryLogRuntime {
    fn put_blob(
        &self,
        universe_id: UniverseId,
        bytes: &[u8],
    ) -> Result<aos_cbor::Hash, PlaneError> {
        let store = self
            .universe_stores
            .get(&universe_id)
            .ok_or(PlaneError::UnknownUniverseStore(universe_id))?;
        Ok(store.put_blob(bytes)?)
    }

    fn get_blob(
        &self,
        universe_id: UniverseId,
        hash: aos_cbor::Hash,
    ) -> Result<Vec<u8>, PlaneError> {
        let store = self
            .universe_stores
            .get(&universe_id)
            .ok_or(PlaneError::UnknownUniverseStore(universe_id))?;
        Ok(store.get_blob(hash)?)
    }

    fn has_blob(&self, universe_id: UniverseId, hash: aos_cbor::Hash) -> Result<bool, PlaneError> {
        let store = self
            .universe_stores
            .get(&universe_id)
            .ok_or(PlaneError::UnknownUniverseStore(universe_id))?;
        Ok(store.has_blob(hash)?)
    }
}

impl SubmissionPlane for MemoryLogRuntime {
    fn submit(&mut self, submission: SubmissionEnvelope) -> Result<u64, PlaneError> {
        let known_world = self.worlds.contains_key(&submission.world_id);
        if !known_world && !matches!(&submission.payload, SubmissionPayload::CreateWorld { .. }) {
            return Err(PlaneError::UnknownWorld {
                universe_id: submission.universe_id,
                world_id: submission.world_id,
            });
        }
        let offset = self.next_submission_offset;
        self.next_submission_offset = self.next_submission_offset.saturating_add(1);
        self.pending_submissions
            .push_back(QueuedSubmission { submission });
        Ok(offset)
    }
}

impl WorldLogPlane for MemoryLogRuntime {
    fn append_frame(&mut self, frame: WorldLogFrame) -> Result<WorldLogAppendResult, PlaneError> {
        if !self.worlds.contains_key(&frame.world_id) {
            return Err(PlaneError::UnknownWorld {
                universe_id: frame.universe_id,
                world_id: frame.world_id,
            });
        }
        let partition = partition_for_world(frame.world_id, self.partition_count);
        let frames = self.world_frames.entry(frame.world_id).or_default();
        let expected = frames
            .last()
            .map(|last| last.world_seq_end.saturating_add(1))
            .unwrap_or(0);
        if frame.world_seq_start != expected {
            return Err(PlaneError::NonContiguousWorldSeq {
                universe_id: frame.universe_id,
                world_id: frame.world_id,
                expected,
                actual: frame.world_seq_start,
            });
        }
        frames.push(frame);
        let partition_entries = self
            .partition_logs
            .entry((DEFAULT_JOURNAL_TOPIC.to_owned(), partition))
            .or_default();
        let offset = partition_entries.len() as u64;
        partition_entries.push(PartitionLogEntry {
            offset,
            frame: frames.last().expect("frame just pushed").clone(),
        });
        Ok(WorldLogAppendResult {
            journal_offset: offset,
        })
    }

    fn world_frames(&self, world_id: WorldId) -> &[WorldLogFrame] {
        self.world_frames
            .get(&world_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

impl CheckpointPlane for MemoryLogRuntime {
    fn commit_checkpoint(&mut self, checkpoint: PartitionCheckpoint) -> Result<(), PlaneError> {
        self.latest_checkpoints.insert(
            (checkpoint.journal_topic.clone(), checkpoint.partition),
            checkpoint,
        );
        Ok(())
    }

    fn latest_checkpoint(
        &self,
        journal_topic: &str,
        partition: u32,
    ) -> Option<&PartitionCheckpoint> {
        self.latest_checkpoints
            .get(&(journal_topic.to_string(), partition))
    }
}

pub struct MemoryShardWorker {
    partition: u32,
}

impl MemoryShardWorker {
    pub fn new(partition: u32) -> Self {
        Self { partition }
    }

    pub fn partition(&self) -> u32 {
        self.partition
    }

    pub fn run_once(
        &mut self,
        runtime: &mut MemoryLogRuntime,
    ) -> Result<Vec<WorldLogFrame>, PlaneError> {
        runtime.process_partition(self.partition)
    }

    pub fn publish_checkpoint(
        &mut self,
        runtime: &mut MemoryLogRuntime,
        created_at_ns: u64,
    ) -> Result<PartitionCheckpoint, PlaneError> {
        runtime.create_partition_checkpoint(self.partition, created_at_ns)
    }
}
