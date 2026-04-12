use aos_kernel::journal::JournalRecord;
use aos_node::{CheckpointPlane, WorldLogFrame, partition_for_world};
use std::collections::BTreeMap;

use super::types::HostedWorkerRuntimeInner;
use super::types::WorkerError;
use super::util::latest_snapshot_record;

const CHECKPOINT_HEAD_DELTA_THRESHOLD: u64 = 2_000;
const CHECKPOINT_MAX_AGE_NS: u64 = 4 * 60 * 60 * 1_000_000_000;

impl HostedWorkerRuntimeInner {
    pub(super) fn create_partition_checkpoint(
        &mut self,
        partition: u32,
        created_at_ns: u64,
        trigger: &'static str,
    ) -> Result<aos_node::PartitionCheckpoint, WorkerError> {
        let mut world_keys: Vec<_> = self
            .state
            .active_worlds
            .keys()
            .filter_map(|&world_id| {
                let effective = partition_for_world(world_id, self.infra.kafka.partition_count());
                if effective != partition {
                    return None;
                }
                self.state
                    .registered_worlds
                    .get(&world_id)
                    .map(|world| (world.universe_id, world_id))
            })
            .collect();
        world_keys.sort_by_key(|(universe_id, world_id)| (*universe_id, *world_id));

        let mut worlds_by_domain: BTreeMap<
            aos_node::UniverseId,
            BTreeMap<aos_node::WorldId, aos_node::WorldCheckpointRef>,
        > = BTreeMap::new();
        let mut journal_offset_by_domain: BTreeMap<aos_node::UniverseId, u64> = BTreeMap::new();
        let mut updated_domains = std::collections::BTreeSet::new();
        let mut compaction_targets = Vec::new();
        let journal_topic = self.infra.kafka.config().journal_topic.clone();
        let domain_ids: std::collections::BTreeSet<_> = world_keys
            .iter()
            .map(|(universe_id, _)| *universe_id)
            .collect();

        for universe_id in domain_ids {
            if let Some(checkpoint) = self
                .infra
                .blob_meta_for_domain_mut(universe_id)?
                .latest_checkpoint(&journal_topic, partition)
                .cloned()
            {
                journal_offset_by_domain.insert(universe_id, checkpoint.journal_offset);
                let worlds = worlds_by_domain.entry(universe_id).or_default();
                for world in checkpoint.worlds {
                    worlds.insert(world.world_id, world);
                }
            }
        }

        for (universe_id, world_id) in world_keys {
            let world_epoch = self
                .state
                .registered_worlds
                .get(&world_id)
                .map(|world| world.world_epoch)
                .unwrap_or(1);
            let expected_world_seq = self.infra.kafka.next_world_seq(world_id);

            let (frame, snapshot_record, manifest_hash) = {
                let world = self.state.active_worlds.get_mut(&world_id).ok_or(
                    WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    },
                )?;
                let current_head = world.host.heights().head;
                let head_delta = current_head.saturating_sub(world.last_checkpointed_head);
                let checkpoint_age_ns = created_at_ns.saturating_sub(world.last_checkpointed_at_ns);
                let checkpoint_due = head_delta >= CHECKPOINT_HEAD_DELTA_THRESHOLD
                    || (head_delta > 0 && checkpoint_age_ns >= CHECKPOINT_MAX_AGE_NS);
                if !checkpoint_due {
                    continue;
                }
                let journal_tail_start = world.host.journal_bounds().next_seq;
                world.host.snapshot()?;
                let tail = world.host.kernel().dump_journal_from(journal_tail_start)?;
                if tail.is_empty() {
                    continue;
                }

                let mut records = Vec::with_capacity(tail.len());
                for entry in &tail {
                    let record: JournalRecord = serde_cbor::from_slice(&entry.payload)?;
                    records.push(record);
                }

                if expected_world_seq > journal_tail_start {
                    tracing::warn!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        expected_world_seq,
                        journal_tail_start,
                        trigger,
                        "hosted checkpoint world sequence diverged from host journal tail; using host tail"
                    );
                } else if expected_world_seq < journal_tail_start {
                    tracing::debug!(
                        universe_id = %universe_id,
                        world_id = %world_id,
                        expected_world_seq,
                        journal_tail_start,
                        trigger,
                        "hosted checkpoint world sequence advanced ahead of persisted tail; using host tail"
                    );
                }

                let frame = WorldLogFrame {
                    format_version: 1,
                    universe_id,
                    world_id,
                    world_epoch,
                    world_seq_start: journal_tail_start,
                    world_seq_end: journal_tail_start + records.len() as u64 - 1,
                    records,
                };
                let snapshot_record = latest_snapshot_record(&tail).ok_or_else(|| {
                    WorkerError::Kernel(aos_kernel::KernelError::SnapshotUnavailable(
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
            let append = self.infra.kafka.append_frame_transactional(frame)?;
            journal_offset_by_domain
                .entry(universe_id)
                .and_modify(|offset| *offset = (*offset).max(append.journal_offset))
                .or_insert(append.journal_offset);
            compaction_targets.push((universe_id, world_id, snapshot_record.height));

            worlds_by_domain.entry(universe_id).or_default().insert(
                world_id,
                aos_node::WorldCheckpointRef {
                    universe_id,
                    world_id,
                    world_epoch,
                    checkpointed_at_ns: created_at_ns,
                    world_seq: frame_world_seq_end,
                    baseline: aos_node::PromotableBaselineRef {
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
                },
            );
            updated_domains.insert(universe_id);
        }

        let mut latest_checkpoint = aos_node::PartitionCheckpoint {
            journal_topic: journal_topic.clone(),
            partition,
            journal_offset: 0,
            created_at_ns,
            worlds: Vec::new(),
        };
        for (universe_id, worlds) in worlds_by_domain {
            if worlds.is_empty() || !updated_domains.contains(&universe_id) {
                continue;
            }
            let checkpoint = aos_node::PartitionCheckpoint {
                journal_topic: journal_topic.clone(),
                partition,
                journal_offset: journal_offset_by_domain
                    .get(&universe_id)
                    .copied()
                    .unwrap_or(0),
                created_at_ns,
                worlds: worlds.into_values().collect(),
            };
            self.infra
                .blob_meta_for_domain_mut(universe_id)?
                .commit_checkpoint(checkpoint.clone())?;
            latest_checkpoint = checkpoint;
        }
        for (universe_id, world_id, height) in compaction_targets {
            let world =
                self.state
                    .active_worlds
                    .get_mut(&world_id)
                    .ok_or(WorkerError::UnknownWorld {
                        universe_id,
                        world_id,
                    })?;
            world.last_checkpointed_head = height;
            world.last_checkpointed_at_ns = created_at_ns;
            world.host.compact_journal_through(height)?;
        }
        if !latest_checkpoint.worlds.is_empty() {
            tracing::info!(
                partition = latest_checkpoint.partition,
                worlds = latest_checkpoint.worlds.len(),
                trigger,
                "aos-node-hosted checkpoint published"
            );
        }
        Ok(latest_checkpoint)
    }
}
