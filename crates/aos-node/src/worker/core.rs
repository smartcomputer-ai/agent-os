use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use aos_kernel::{OpenedEffect, WorldInput};
use aos_node::{
    JournalCommit, PromotableBaselineRef, SubmissionEnvelope, SubmissionRejection, UniverseId,
    WorldId, WorldLogFrame,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub(super) struct LocalInputMsg {
    pub world_id: WorldId,
    pub input: WorldInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct KafkaOffsetAck {
    pub partition: u32,
    pub offset: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum AckRef {
    KafkaOffset(KafkaOffsetAck),
    DirectAccept { accept_token: u64 },
}

#[derive(Debug, Clone)]
pub(super) struct AcceptedSubmission {
    pub ack_ref: AckRef,
    pub envelope: SubmissionEnvelope,
}

#[derive(Debug, Clone)]
pub(super) enum SchedulerMsg {
    Accepted(AcceptedSubmission),
    LocalInput(LocalInputMsg),
    FlushTick,
    CheckpointTick,
    Shutdown,
}

#[derive(Debug, Clone)]
pub(super) enum WorkItem {
    Accepted {
        ack_ref: AckRef,
        envelope: SubmissionEnvelope,
    },
    LocalInput(WorldInput),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum DurableDisposition {
    RejectedSubmission {
        ack_ref: Option<AckRef>,
        world_id: WorldId,
        reason: SubmissionRejection,
    },
    CommandFailure {
        ack_ref: Option<AckRef>,
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
    pub ack_ref: Option<AckRef>,
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
    pub ack_refs: Vec<AckRef>,
    pub bytes: usize,
}

#[derive(Debug)]
pub(super) struct FlushOutcome {
    pub committed_slices: Vec<CompletedSlice>,
    pub ack_refs: Vec<AckRef>,
    pub journal_commit: JournalCommit,
}

#[derive(Debug, Clone)]
pub(super) enum PendingState {
    Received,
    Serviced(SliceId),
}

#[derive(Debug, Clone)]
pub(super) struct PendingIngressEntry {
    pub ack: KafkaOffsetAck,
    pub envelope: SubmissionEnvelope,
    pub state: PendingState,
}

#[derive(Debug, Default)]
pub(super) struct SchedulerState {
    pub pending_by_partition: BTreeMap<u32, VecDeque<PendingIngressEntry>>,
    pub staged_slices: BTreeMap<SliceId, CompletedSlice>,
    pub ready_non_kafka_slices: VecDeque<SliceId>,
    pub flush_rr_cursor: usize,
}

impl SchedulerState {
    pub fn stage_slice(&mut self, slice: CompletedSlice) {
        if !matches!(slice.ack_ref, Some(AckRef::KafkaOffset(_))) {
            self.ready_non_kafka_slices.push_back(slice.id);
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
        for slice_id in self.ready_non_kafka_slices.iter().copied() {
            let Some(slice) = self.staged_slices.get(&slice_id) else {
                continue;
            };
            let is_checkpoint = slice.checkpoint.is_some();
            let is_local_continuation = !is_checkpoint && slice_is_local_continuation(slice);
            if is_local_continuation
                && local_continuations >= max_local_continuation_slices_per_flush
            {
                continue;
            };
            if !can_include_slice(slice, &batch) {
                continue;
            }
            if !fits(&batch, slice, &limits) {
                continue;
            }
            push_slice(&mut batch, slice);
            if is_local_continuation {
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
    if let Some(ack_ref) = slice.ack_ref {
        batch.ack_refs.push(ack_ref);
    }

    for frame in &slice.frames {
        batch.frames.push(frame.clone());
    }
    if let Some(disposition) = &slice.disposition {
        batch.dispositions.push(disposition.clone());
    }
}

fn slice_is_local_continuation(slice: &CompletedSlice) -> bool {
    matches!(slice.original_item, Some(WorkItem::LocalInput(_)))
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

    fn slice(id: SliceId, world_id: WorldId, ack_ref: Option<AckRef>) -> CompletedSlice {
        CompletedSlice {
            id,
            world_id,
            affected_worlds: vec![world_id],
            staged_at: Instant::now(),
            ack_ref,
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
                    ack: KafkaOffsetAck {
                        partition: 0,
                        offset: 10,
                    },
                    envelope: input_submission(world_a),
                    state: PendingState::Serviced(1),
                },
                PendingIngressEntry {
                    ack: KafkaOffsetAck {
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
                    ack: KafkaOffsetAck {
                        partition: 1,
                        offset: 20,
                    },
                    envelope: input_submission(world_b),
                    state: PendingState::Serviced(2),
                },
                PendingIngressEntry {
                    ack: KafkaOffsetAck {
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
            Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 10,
            })),
        ));
        state.stage_slice(slice(
            2,
            world_b,
            Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 1,
                offset: 20,
            })),
        ));
        state.stage_slice(slice(
            3,
            world_b,
            Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 1,
                offset: 21,
            })),
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
        assert_eq!(
            batch.ack_refs,
            vec![
                AckRef::KafkaOffset(KafkaOffsetAck {
                    partition: 0,
                    offset: 10
                }),
                AckRef::KafkaOffset(KafkaOffsetAck {
                    partition: 1,
                    offset: 20
                }),
                AckRef::KafkaOffset(KafkaOffsetAck {
                    partition: 1,
                    offset: 21
                }),
            ]
        );
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
        assert!(batch.ack_refs.is_empty());
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
        assert!(batch.ack_refs.is_empty());
    }

    #[test]
    fn collect_flushable_slices_appends_one_local_continuation_after_ingress_prefix() {
        let ingress_world = test_world(6);
        let local_world = test_world(7);

        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(
            0,
            VecDeque::from(vec![PendingIngressEntry {
                ack: KafkaOffsetAck {
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
            Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 42,
            })),
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
        assert_eq!(
            batch.ack_refs,
            vec![AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 42
            })]
        );
    }

    #[test]
    fn collect_flushable_slices_can_skip_local_continuations_entirely() {
        let ingress_world = test_world(9);
        let local_world = test_world(10);

        let mut state = SchedulerState::default();
        state.pending_by_partition.insert(
            0,
            VecDeque::from(vec![PendingIngressEntry {
                ack: KafkaOffsetAck {
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
            Some(AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 42,
            })),
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
        assert_eq!(
            batch.ack_refs,
            vec![AckRef::KafkaOffset(KafkaOffsetAck {
                partition: 0,
                offset: 42
            })]
        );
    }

    #[test]
    fn collect_flushable_slices_does_not_throttle_direct_accepts_as_local_continuations() {
        let direct_world = test_world(11);
        let local_world = test_world(12);

        let mut state = SchedulerState::default();
        state.stage_slice(CompletedSlice {
            id: 1,
            world_id: direct_world,
            affected_worlds: vec![direct_world],
            staged_at: Instant::now(),
            ack_ref: Some(AckRef::DirectAccept { accept_token: 1 }),
            original_item: Some(WorkItem::Accepted {
                ack_ref: AckRef::DirectAccept { accept_token: 1 },
                envelope: input_submission(direct_world),
            }),
            frames: Vec::new(),
            disposition: None,
            opened_effects: Vec::new(),
            checkpoint: None,
            approx_bytes: 1,
        });
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
        assert_eq!(
            batch.ack_refs,
            vec![AckRef::DirectAccept { accept_token: 1 }]
        );
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
