use std::time::Duration;

use aos_kernel::journal::JournalRecord;
use aos_node::{
    CheckpointBackend, PartitionCheckpoint, PromotableBaselineRef, partition_for_world,
};

use super::core::{CheckpointCommit, CheckpointWorldCommit, CompletedSlice};
use super::types::HostedWorkerCore;
use super::types::WorkerError;
use super::util::{latest_snapshot_record, snapshot_record_from_checkpoint, unix_time_ns};

impl HostedWorkerCore {
    fn adopt_visible_partition_checkpoints(
        &mut self,
        partition: u32,
        refreshed_at_ns: u64,
    ) -> Result<(), WorkerError> {
        let world_keys = self
            .state
            .active_worlds
            .iter()
            .filter_map(|(&world_id, world)| {
                (partition_for_world(world_id, self.infra.kafka.partition_count()) == partition)
                    .then_some((world.universe_id, world_id))
            })
            .collect::<Vec<_>>();
        let journal_topic = self.infra.kafka.config().journal_topic.clone();

        for (universe_id, world_id) in world_keys {
            let latest = self
                .infra
                .blob_meta_for_domain_mut(universe_id)?
                .latest_checkpoint(&journal_topic, partition)
                .cloned();
            let Some(checkpoint) = latest else {
                continue;
            };
            let Some(entry) = checkpoint
                .worlds
                .iter()
                .find(|entry| entry.universe_id == universe_id && entry.world_id == world_id)
            else {
                continue;
            };
            let Some(world) = self.state.active_worlds.get_mut(&world_id) else {
                continue;
            };
            if entry.baseline.height <= world.last_checkpointed_head
                && !world.pending_create_checkpoint
            {
                continue;
            }

            world.pending_create_checkpoint = false;
            world.last_checkpointed_head = world.last_checkpointed_head.max(entry.baseline.height);
            world.last_checkpointed_at_ns = refreshed_at_ns.max(world.last_checkpointed_at_ns);
            world.next_world_seq = world
                .next_world_seq
                .max(entry.baseline.height.saturating_add(1));
            if entry.baseline.height >= world.active_baseline.height {
                world.active_baseline = snapshot_record_from_checkpoint(&entry.baseline);
            }
        }

        Ok(())
    }

    pub(super) fn publish_due_checkpoints(
        &mut self,
        checkpoint_interval: Duration,
        checkpoint_every_events: Option<u32>,
    ) -> Result<usize, WorkerError> {
        let created_at_ns = unix_time_ns();
        let max_age_ns = checkpoint_interval.as_nanos().min(u128::from(u64::MAX)) as u64;
        let partitions = self
            .state
            .assigned_partitions
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let mut published = 0usize;

        for partition in partitions {
            self.adopt_visible_partition_checkpoints(partition, created_at_ns)?;
            if let Some(slice) = self.stage_partition_checkpoint_slice(
                partition,
                created_at_ns,
                max_age_ns,
                checkpoint_every_events,
                "tick",
            )? {
                self.stage_completed_slice(slice)?;
                published = published.saturating_add(1);
            }
        }

        Ok(published)
    }

    pub(super) fn create_partition_checkpoint(
        &mut self,
        partition: u32,
        created_at_ns: u64,
        max_age_ns: u64,
        checkpoint_every_events: Option<u32>,
        trigger: &'static str,
    ) -> Result<Option<PartitionCheckpoint>, WorkerError> {
        let Some(slice) = self.stage_partition_checkpoint_slice(
            partition,
            created_at_ns,
            max_age_ns,
            checkpoint_every_events,
            trigger,
        )?
        else {
            return Ok(None);
        };
        let checkpoint_universes = slice
            .checkpoint
            .as_ref()
            .map(|checkpoint| {
                checkpoint
                    .worlds
                    .iter()
                    .map(|world| world.universe_id)
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default();
        let checkpoint_partition = slice
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.partition)
            .unwrap_or(partition);
        let target_slice_id = slice.id;
        self.stage_completed_slice(slice)?;

        while self
            .state
            .scheduler
            .staged_slices
            .contains_key(&target_slice_id)
        {
            let Some(outcome) = self.flush_ready_batch()? else {
                return Err(WorkerError::Persist(aos_node::PersistError::backend(
                    format!("checkpoint slice {target_slice_id} never became flushable"),
                )));
            };
            self.finalize_flush_success(outcome)?;
        }

        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        let mut latest_checkpoint = None;
        for universe_id in checkpoint_universes {
            latest_checkpoint = self
                .infra
                .blob_meta_for_domain_mut(universe_id)?
                .latest_checkpoint(&journal_topic, checkpoint_partition)
                .cloned()
                .or(latest_checkpoint);
        }
        Ok(latest_checkpoint)
    }

    fn stage_partition_checkpoint_slice(
        &mut self,
        partition: u32,
        created_at_ns: u64,
        max_age_ns: u64,
        checkpoint_every_events: Option<u32>,
        trigger: &'static str,
    ) -> Result<Option<CompletedSlice>, WorkerError> {
        let require_runtime_quiescent = trigger == "tick";
        let mut world_keys = self
            .state
            .active_worlds
            .iter()
            .filter_map(|(&world_id, world)| {
                let effective = partition_for_world(world_id, self.infra.kafka.partition_count());
                let quiescence = world.kernel.quiescence_status();
                let scheduled_timers = self
                    .state
                    .async_worlds
                    .get(&world_id)
                    .map(|state| !state.scheduled_timers.is_empty())
                    .unwrap_or(false);
                if effective != partition
                    || world.running
                    || world.commit_blocked
                    || world.pending_slice.is_some()
                    || !world.mailbox.is_empty()
                    || (require_runtime_quiescent
                        && (!quiescence.runtime_quiescent || scheduled_timers))
                    || !checkpoint_due(world, created_at_ns, max_age_ns, checkpoint_every_events)
                {
                    return None;
                }
                Some((world.universe_id, world_id))
            })
            .collect::<Vec<_>>();
        world_keys.sort_by_key(|(universe_id, world_id)| (*universe_id, *world_id));

        if world_keys.is_empty() {
            return Ok(None);
        }

        let mut frames = Vec::new();
        let mut checkpoint_worlds = Vec::new();
        let mut affected_worlds = Vec::new();
        let mut approx_bytes = 0usize;

        for (universe_id, world_id) in world_keys {
            let durable_next_world_seq = self.infra.kafka.next_world_seq(world_id);
            let (frame, baseline, world_epoch, manifest_hash) = {
                let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                let tail_start = world.kernel.journal_bounds().next_seq;
                let world_seq_start = world.next_world_seq.max(durable_next_world_seq);
                let pending_create_checkpoint = world.pending_create_checkpoint;
                world.kernel.create_snapshot()?;
                let tail = world.kernel.dump_journal_from(tail_start)?;
                if tail.is_empty() {
                    if pending_create_checkpoint {
                        (
                            None,
                            world.active_baseline.clone(),
                            world.world_epoch,
                            world.kernel.manifest_hash().to_hex(),
                        )
                    } else {
                        continue;
                    }
                } else {
                    if world_seq_start > tail_start {
                        tracing::warn!(
                            universe_id = %universe_id,
                            world_id = %world_id,
                            durable_next_world_seq,
                            in_memory_next_world_seq = world.next_world_seq,
                            journal_tail_start = tail_start,
                            world_seq_start,
                            trigger,
                            "aos-node-hosted checkpoint world sequence diverged from active journal tail; using stored world sequence"
                        );
                    } else if world_seq_start < tail_start {
                        tracing::debug!(
                            universe_id = %universe_id,
                            world_id = %world_id,
                            durable_next_world_seq,
                            in_memory_next_world_seq = world.next_world_seq,
                            journal_tail_start = tail_start,
                            world_seq_start,
                            trigger,
                            "aos-node-hosted checkpoint active journal tail advanced ahead of stored world sequence"
                        );
                    }
                    let records = tail
                        .iter()
                        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload))
                        .collect::<Result<Vec<_>, _>>()?;
                    let baseline = latest_snapshot_record(&tail).ok_or_else(|| {
                        WorkerError::Kernel(aos_kernel::KernelError::SnapshotUnavailable(
                            "checkpoint snapshot did not emit a snapshot record".into(),
                        ))
                    })?;
                    let frame = aos_node::WorldLogFrame {
                        format_version: 1,
                        universe_id,
                        world_id,
                        world_epoch: world.world_epoch,
                        world_seq_start,
                        world_seq_end: world_seq_start + records.len() as u64 - 1,
                        records,
                    };
                    (
                        Some(frame),
                        aos_node::SnapshotRecord {
                            snapshot_ref: baseline.snapshot_ref,
                            height: baseline.height,
                            universe_id: baseline.universe_id.into(),
                            logical_time_ns: baseline.logical_time_ns,
                            receipt_horizon_height: baseline.receipt_horizon_height,
                            manifest_hash: baseline.manifest_hash,
                        },
                        world.world_epoch,
                        world.kernel.manifest_hash().to_hex(),
                    )
                }
            };
            let baseline = PromotableBaselineRef {
                snapshot_ref: baseline.snapshot_ref,
                snapshot_manifest_ref: None,
                manifest_hash: baseline.manifest_hash.clone().unwrap_or(manifest_hash),
                height: baseline.height,
                universe_id: baseline.universe_id,
                logical_time_ns: baseline.logical_time_ns,
                receipt_horizon_height: baseline.receipt_horizon_height.unwrap_or(baseline.height),
            };
            let world_seq = frame
                .as_ref()
                .map(|frame| frame.world_seq_end)
                .unwrap_or(baseline.height);
            let compact_through = frame.as_ref().map(|_| baseline.height);
            if let Some(frame) = frame {
                approx_bytes = approx_bytes.saturating_add(serde_cbor::to_vec(&frame)?.len());
                frames.push(frame);
            }
            checkpoint_worlds.push(CheckpointWorldCommit {
                universe_id,
                world_id,
                world_epoch,
                world_seq,
                baseline,
                compact_through,
            });
            affected_worlds.push(world_id);
        }

        if checkpoint_worlds.is_empty() {
            return Ok(None);
        }

        approx_bytes = approx_bytes.saturating_add(checkpoint_worlds.len());
        Ok(Some(CompletedSlice {
            id: self.next_slice_id(),
            world_id: affected_worlds[0],
            affected_worlds,
            staged_at: std::time::Instant::now(),
            source: None,
            original_item: None,
            frames,
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: Some(CheckpointCommit {
                partition,
                created_at_ns,
                trigger,
                worlds: checkpoint_worlds,
            }),
            approx_bytes,
        }))
    }
}

fn checkpoint_due(
    world: &super::types::ActiveWorld,
    created_at_ns: u64,
    max_age_ns: u64,
    checkpoint_every_events: Option<u32>,
) -> bool {
    if world.pending_create_checkpoint {
        return true;
    }

    let current_head = world.kernel.journal_head().saturating_sub(1);
    let head_delta = current_head.saturating_sub(world.last_checkpointed_head);
    let checkpoint_age_ns = created_at_ns.saturating_sub(world.last_checkpointed_at_ns);

    if checkpoint_every_events.is_some_and(|threshold| head_delta >= u64::from(threshold)) {
        return true;
    }

    head_delta > 0 && checkpoint_age_ns >= max_age_ns
}
