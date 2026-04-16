use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use aos_kernel::{OpenedEffect, WorldInput};
use aos_node::{
    PromotableBaselineRef, SubmissionEnvelope, SubmissionRejection, UniverseId, WorldId,
    WorldLogFrame,
};
use serde::{Deserialize, Serialize};

use crate::kafka::IngressRecord;

#[derive(Debug, Clone)]
pub(super) struct LocalInputMsg {
    pub world_id: WorldId,
    pub input: WorldInput,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AssignmentDelta {
    pub assigned: Vec<u32>,
    pub newly_assigned: Vec<u32>,
    pub revoked: Vec<u32>,
}

#[derive(Debug, Clone)]
pub(super) enum SchedulerMsg {
    Ingress(IngressRecord),
    LocalInput(LocalInputMsg),
    Assignment(AssignmentDelta),
    FlushTick,
    CheckpointTick,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct IngressToken {
    pub partition: u32,
    pub offset: i64,
}

#[derive(Debug, Clone)]
pub(super) enum WorkItem {
    Ingress {
        token: IngressToken,
        envelope: SubmissionEnvelope,
    },
    LocalInput(WorldInput),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum DurableDisposition {
    RejectedSubmission {
        token: IngressToken,
        world_id: WorldId,
        reason: SubmissionRejection,
    },
    CommandFailure {
        token: IngressToken,
        world_id: WorldId,
        command_id: String,
        error_code: String,
    },
}

pub(super) type SliceId = u64;

#[derive(Debug, Clone)]
pub(super) struct CheckpointWorldCommit {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    pub world_seq: u64,
    pub baseline: PromotableBaselineRef,
    pub compact_through: Option<u64>,
}

#[derive(Debug, Clone)]
pub(super) struct CheckpointCommit {
    pub partition: u32,
    pub created_at_ns: u64,
    pub trigger: &'static str,
    pub worlds: Vec<CheckpointWorldCommit>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletedSlice {
    pub id: SliceId,
    pub world_id: WorldId,
    pub affected_worlds: Vec<WorldId>,
    pub staged_at: Instant,
    pub source: Option<IngressToken>,
    pub original_item: Option<WorkItem>,
    pub frames: Vec<WorldLogFrame>,
    pub disposition: Option<DurableDisposition>,
    pub opened_effects: Vec<OpenedEffect>,
    pub checkpoint: Option<CheckpointCommit>,
    pub approx_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FlushLimits {
    pub max_slices: usize,
    pub max_bytes: usize,
    pub max_delay: Duration,
}

#[derive(Debug, Clone, Default)]
pub(super) struct FlushBatch {
    pub slice_ids: Vec<SliceId>,
    pub frames: Vec<WorldLogFrame>,
    pub dispositions: Vec<DurableDisposition>,
    pub offset_commits: BTreeMap<u32, i64>,
    pub bytes: usize,
}

#[derive(Debug)]
pub(super) struct FlushOutcome {
    pub committed_slices: Vec<CompletedSlice>,
    pub committed_offsets: BTreeMap<u32, i64>,
}

#[derive(Debug, Clone)]
pub(super) enum PendingState {
    Received,
    Serviced(SliceId),
}

#[derive(Debug, Clone)]
pub(super) struct PendingIngressEntry {
    pub token: IngressToken,
    pub envelope: SubmissionEnvelope,
    pub state: PendingState,
}

#[derive(Debug, Default)]
pub(super) struct SchedulerState {
    pub pending_by_partition: BTreeMap<u32, VecDeque<PendingIngressEntry>>,
    pub staged_slices: BTreeMap<SliceId, CompletedSlice>,
    pub local_ready_slices: VecDeque<SliceId>,
    pub flush_rr_cursor: usize,
}

impl SchedulerState {
    pub fn stage_slice(&mut self, slice: CompletedSlice) {
        if slice.source.is_none() {
            self.local_ready_slices.push_back(slice.id);
        }
        self.staged_slices.insert(slice.id, slice);
    }

    pub fn collect_flushable_slices(
        &self,
        limits: FlushLimits,
        max_local_continuation_slices_per_flush: usize,
        mut can_include_slice: impl FnMut(&CompletedSlice, &FlushBatch) -> bool,
    ) -> Option<FlushBatch> {
        let mut batch = FlushBatch::default();
        let partitions = self.partition_order_from(self.flush_rr_cursor);
        let mut scan_idx: BTreeMap<u32, usize> = BTreeMap::new();

        loop {
            let mut progressed = false;

            for partition in &partitions {
                let Some(queue) = self.pending_by_partition.get(partition) else {
                    continue;
                };
                let idx = scan_idx.entry(*partition).or_insert(0);
                let Some(entry) = queue.get(*idx) else {
                    continue;
                };

                let PendingState::Serviced(slice_id) = entry.state else {
                    continue;
                };

                let Some(slice) = self.staged_slices.get(&slice_id) else {
                    continue;
                };
                if !can_include_slice(slice, &batch) {
                    continue;
                }
                if !fits(&batch, slice, &limits) {
                    return (!batch.slice_ids.is_empty()).then_some(batch);
                }

                push_slice(&mut batch, slice);
                batch.offset_commits.insert(*partition, entry.token.offset);
                *idx += 1;
                progressed = true;
            }

            if !progressed {
                break;
            }
        }

        // Keep local continuation batching explicit: the current hosted design allows at most one
        // source-less slice per flush so ingress-backed prefixes remain the primary driver of
        // Kafka transactions and local followups cannot monopolize the journal fence.
        let mut local_continuations = 0usize;
        for slice_id in self.local_ready_slices.iter().copied() {
            let Some(slice) = self.staged_slices.get(&slice_id) else {
                continue;
            };
            let is_checkpoint = slice.checkpoint.is_some();
            if !is_checkpoint && local_continuations >= max_local_continuation_slices_per_flush {
                continue;
            };
            if !can_include_slice(slice, &batch) {
                continue;
            }
            if !fits(&batch, slice, &limits) {
                continue;
            }
            push_slice(&mut batch, slice);
            if !is_checkpoint {
                local_continuations = local_continuations.saturating_add(1);
            }
        }

        (!batch.slice_ids.is_empty()).then_some(batch)
    }

    pub fn advance_flush_rr_cursor(&mut self) {
        let partition_count = self.pending_by_partition.len();
        if partition_count == 0 {
            self.flush_rr_cursor = 0;
            return;
        }
        self.flush_rr_cursor = (self.flush_rr_cursor + 1) % partition_count;
    }

    fn partition_order_from(&self, cursor: usize) -> Vec<u32> {
        let mut partitions = self
            .pending_by_partition
            .keys()
            .copied()
            .collect::<Vec<_>>();
        if partitions.is_empty() {
            return partitions;
        }
        let rotate = cursor % partitions.len();
        partitions.rotate_left(rotate);
        partitions
    }
}

pub(super) fn fits(batch: &FlushBatch, slice: &CompletedSlice, limits: &FlushLimits) -> bool {
    if limits.max_slices > 0 && batch.slice_ids.len() >= limits.max_slices {
        return false;
    }
    if limits.max_bytes > 0
        && !batch.slice_ids.is_empty()
        && batch.bytes.saturating_add(slice.approx_bytes) > limits.max_bytes
    {
        return false;
    }
    true
}

pub(super) fn push_slice(batch: &mut FlushBatch, slice: &CompletedSlice) {
    batch.slice_ids.push(slice.id);
    batch.bytes = batch.bytes.saturating_add(slice.approx_bytes);

    for frame in &slice.frames {
        batch.frames.push(frame.clone());
    }
    if let Some(disposition) = &slice.disposition {
        batch.dispositions.push(disposition.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aos_kernel::WorldInput;
    use aos_node::{SubmissionEnvelope, SubmissionPayload, UniverseId, WorldId};

    fn test_world(id: u128) -> WorldId {
        WorldId::from(uuid::Uuid::from_u128(id))
    }

    fn input_submission(world_id: WorldId) -> SubmissionEnvelope {
        SubmissionEnvelope {
            submission_id: format!("sub-{world_id}"),
            universe_id: UniverseId::from(uuid::Uuid::nil()),
            world_id,
            world_epoch: 1,
            command: None,
            payload: SubmissionPayload::WorldInput {
                input: WorldInput::DomainEvent(aos_wasm_abi::DomainEvent {
                    schema: "test/Event@1".into(),
                    value: vec![1],
                    key: None,
                }),
            },
        }
    }

    fn slice(id: SliceId, world_id: WorldId, source: Option<IngressToken>) -> CompletedSlice {
        CompletedSlice {
            id,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            source,
            original_item: Some(WorkItem::LocalInput(WorldInput::DomainEvent(
                aos_wasm_abi::DomainEvent {
                    schema: "test/Event@1".into(),
                    value: vec![1],
                    key: None,
                },
            ))),
            frames: Vec::new(),
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        }
    }

    #[test]
    fn collect_flushable_slices_only_commits_contiguous_serviced_prefixes() {
        let world_a = test_world(1);
        let world_b = test_world(2);

        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(
            0,
            VecDeque::from(vec![
                PendingIngressEntry {
                    token: IngressToken {
                        partition: 0,
                        offset: 10,
                    },
                    envelope: input_submission(world_a),
                    state: PendingState::Serviced(1),
                },
                PendingIngressEntry {
                    token: IngressToken {
                        partition: 0,
                        offset: 11,
                    },
                    envelope: input_submission(world_a),
                    state: PendingState::Received,
                },
            ]),
        );
        state.pending_by_partition.insert(
            1,
            VecDeque::from(vec![
                PendingIngressEntry {
                    token: IngressToken {
                        partition: 1,
                        offset: 20,
                    },
                    envelope: input_submission(world_b),
                    state: PendingState::Serviced(2),
                },
                PendingIngressEntry {
                    token: IngressToken {
                        partition: 1,
                        offset: 21,
                    },
                    envelope: input_submission(world_b),
                    state: PendingState::Serviced(3),
                },
            ]),
        );
        state.stage_slice(slice(
            1,
            world_a,
            Some(IngressToken {
                partition: 0,
                offset: 10,
            }),
        ));
        state.stage_slice(slice(
            2,
            world_b,
            Some(IngressToken {
                partition: 1,
                offset: 20,
            }),
        ));
        state.stage_slice(slice(
            3,
            world_b,
            Some(IngressToken {
                partition: 1,
                offset: 21,
            }),
        ));

        let batch = state
            .collect_flushable_slices(
                FlushLimits {
                    max_slices: 16,
                    max_bytes: 16,
                    max_delay: Duration::from_millis(1),
                },
                1,
                |_, _| true,
            )
            .expect("batch");

        assert_eq!(batch.slice_ids, vec![1, 2, 3]);
        assert_eq!(batch.offset_commits.get(&0), Some(&10));
        assert_eq!(batch.offset_commits.get(&1), Some(&21));
    }

    #[test]
    fn collect_flushable_slices_can_include_source_less_local_slice() {
        let world_a = test_world(3);

        let mut state = SchedulerState::default();
        state.stage_slice(slice(10, world_a, None));

        let batch = state
            .collect_flushable_slices(
                FlushLimits {
                    max_slices: 4,
                    max_bytes: 4,
                    max_delay: Duration::from_millis(1),
                },
                1,
                |_, _| true,
            )
            .expect("batch");

        assert_eq!(batch.slice_ids, vec![10]);
        assert!(batch.offset_commits.is_empty());
    }

    #[test]
    fn collect_flushable_slices_limits_local_continuations_to_configured_cap() {
        let world_a = test_world(4);
        let world_b = test_world(5);
        let world_c = test_world(8);

        let mut state = SchedulerState::default();
        state.stage_slice(slice(10, world_a, None));
        state.stage_slice(slice(11, world_b, None));
        state.stage_slice(slice(12, world_c, None));

        let batch = state
            .collect_flushable_slices(
                FlushLimits {
                    max_slices: 8,
                    max_bytes: 8,
                    max_delay: Duration::from_millis(1),
                },
                2,
                |_, _| true,
            )
            .expect("batch");

        assert_eq!(batch.slice_ids, vec![10, 11]);
        assert!(batch.offset_commits.is_empty());
    }

    #[test]
    fn collect_flushable_slices_appends_one_local_continuation_after_ingress_prefix() {
        let ingress_world = test_world(6);
        let local_world = test_world(7);

        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(
            0,
            VecDeque::from(vec![PendingIngressEntry {
                token: IngressToken {
                    partition: 0,
                    offset: 42,
                },
                envelope: input_submission(ingress_world),
                state: PendingState::Serviced(1),
            }]),
        );
        state.stage_slice(slice(
            1,
            ingress_world,
            Some(IngressToken {
                partition: 0,
                offset: 42,
            }),
        ));
        state.stage_slice(slice(2, local_world, None));
        state.stage_slice(slice(3, local_world, None));

        let batch = state
            .collect_flushable_slices(
                FlushLimits {
                    max_slices: 8,
                    max_bytes: 8,
                    max_delay: Duration::from_millis(1),
                },
                1,
                |_, _| true,
            )
            .expect("batch");

        assert_eq!(batch.slice_ids, vec![1, 2]);
        assert_eq!(batch.offset_commits.get(&0), Some(&42));
    }

    #[test]
    fn collect_flushable_slices_can_skip_local_continuations_entirely() {
        let ingress_world = test_world(9);
        let local_world = test_world(10);

        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(
            0,
            VecDeque::from(vec![PendingIngressEntry {
                token: IngressToken {
                    partition: 0,
                    offset: 42,
                },
                envelope: input_submission(ingress_world),
                state: PendingState::Serviced(1),
            }]),
        );
        state.stage_slice(slice(
            1,
            ingress_world,
            Some(IngressToken {
                partition: 0,
                offset: 42,
            }),
        ));
        state.stage_slice(slice(2, local_world, None));

        let batch = state
            .collect_flushable_slices(
                FlushLimits {
                    max_slices: 8,
                    max_bytes: 8,
                    max_delay: Duration::from_millis(1),
                },
                0,
                |_, _| true,
            )
            .expect("batch");

        assert_eq!(batch.slice_ids, vec![1]);
        assert_eq!(batch.offset_commits.get(&0), Some(&42));
    }

    #[test]
    fn advance_flush_rr_cursor_rotates_partition_scan_order() {
        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(0, VecDeque::new());
        state.pending_by_partition.insert(1, VecDeque::new());
        state.pending_by_partition.insert(2, VecDeque::new());

        assert_eq!(
            state.partition_order_from(state.flush_rr_cursor),
            vec![0, 1, 2]
        );
        state.advance_flush_rr_cursor();
        assert_eq!(
            state.partition_order_from(state.flush_rr_cursor),
            vec![1, 2, 0]
        );
        state.advance_flush_rr_cursor();
        assert_eq!(
            state.partition_order_from(state.flush_rr_cursor),
            vec![2, 0, 1]
        );
    }
}
