//! Owner-local timer queue primitives.
//!
//! `timer.set` is not part of the shared async adapter runtime. Hosts own timer lifecycle and use
//! this queue to reconstruct pending deadlines from kernel open-work state and later emit
//! continuations back as `WorldInput`.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use aos_effects::builtins::TimerSetParams;
use aos_effects::{EffectIntent, EffectKind};
use aos_kernel::snapshot::WorkflowReceiptSnapshot;
use aos_kernel::{Kernel, Store};
use tracing::warn;

use super::error::RuntimeError;

/// A scheduled timer entry with persistable deadline.
///
/// Uses logical nanoseconds (`deliver_at_ns`) rather than `Instant`
/// so timers can be persisted across restarts.
#[derive(Debug, Clone)]
pub struct TimerEntry {
    /// Absolute logical deadline in nanoseconds.
    pub deliver_at_ns: u64,
    /// Intent hash for building the receipt.
    pub intent_hash: [u8; 32],
    /// Optional correlation key from TimerSetParams.
    pub key: Option<String>,
    /// Original params CBOR for context.
    pub params_cbor: Vec<u8>,
}

impl TimerEntry {
    /// Compute runtime `Instant` from absolute logical deadline.
    ///
    /// If the deadline is in the past, returns `Instant::now()` (fire immediately).
    pub fn deadline_instant(&self, now_ns: u64) -> Instant {
        if self.deliver_at_ns <= now_ns {
            Instant::now()
        } else {
            Instant::now() + Duration::from_nanos(self.deliver_at_ns - now_ns)
        }
    }
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deliver_at_ns == other.deliver_at_ns && self.intent_hash == other.intent_hash
    }
}

impl Eq for TimerEntry {}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.deliver_at_ns
            .cmp(&other.deliver_at_ns)
            .then_with(|| self.intent_hash.cmp(&other.intent_hash))
    }
}

#[derive(Debug, Default)]
pub struct TimerHeap {
    heap: BinaryHeap<Reverse<TimerEntry>>,
}

impl TimerHeap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TimerEntry) {
        self.heap.push(Reverse(entry));
    }

    pub fn peek(&self) -> Option<&TimerEntry> {
        self.heap.peek().map(|Reverse(e)| e)
    }

    pub fn pop(&mut self) -> Option<TimerEntry> {
        self.heap.pop().map(|Reverse(e)| e)
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

#[derive(Debug, Default)]
pub struct TimerScheduler {
    heap: TimerHeap,
}

impl TimerScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&mut self, intent: &EffectIntent) -> Result<(), RuntimeError> {
        let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| RuntimeError::Timer(format!("failed to decode TimerSetParams: {}", e)))?;

        let entry = TimerEntry {
            deliver_at_ns: params.deliver_at_ns,
            intent_hash: intent.intent_hash,
            key: params.key,
            params_cbor: intent.params_cbor.clone(),
        };

        self.heap.push(entry);
        Ok(())
    }

    pub fn next_deadline(&self, now_ns: u64) -> Option<Instant> {
        self.heap.peek().map(|entry| entry.deadline_instant(now_ns))
    }

    pub fn next_due_at_ns(&self) -> Option<u64> {
        self.heap.peek().map(|entry| entry.deliver_at_ns)
    }

    pub fn pop_due(&mut self, now_ns: u64) -> Vec<TimerEntry> {
        let mut due = Vec::new();
        while let Some(entry) = self.heap.peek() {
            if entry.deliver_at_ns <= now_ns {
                due.push(self.heap.pop().unwrap());
            } else {
                break;
            }
        }
        due
    }

    pub fn rehydrate_from_pending(&mut self, contexts: &[WorkflowReceiptSnapshot]) {
        for ctx in contexts {
            if ctx.effect_kind == EffectKind::TIMER_SET {
                if let Ok(params) = serde_cbor::from_slice::<TimerSetParams>(&ctx.params_cbor) {
                    let entry = TimerEntry {
                        deliver_at_ns: params.deliver_at_ns,
                        intent_hash: ctx.intent_hash,
                        key: params.key,
                        params_cbor: ctx.params_cbor.clone(),
                    };
                    self.heap.push(entry);
                } else {
                    warn!(
                        intent_hash = ?ctx.intent_hash,
                        workflow = %ctx.origin_module_id,
                        "failed to decode TimerSetParams while rehydrating timer; dropping entry"
                    );
                }
            }
        }
    }

    pub fn rehydrate_from_kernel<S: Store + 'static>(&mut self, kernel: &Kernel<S>) {
        let pending = kernel.pending_workflow_receipts_snapshot();
        self.rehydrate_from_pending(&pending);
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(deliver_at_ns: u64, hash_byte: u8) -> TimerEntry {
        TimerEntry {
            deliver_at_ns,
            intent_hash: [hash_byte; 32],
            key: None,
            params_cbor: vec![],
        }
    }

    #[test]
    fn heap_orders_by_deadline() {
        let mut heap = TimerHeap::new();
        heap.push(make_entry(300, 3));
        heap.push(make_entry(100, 1));
        heap.push(make_entry(200, 2));

        assert_eq!(heap.pop().unwrap().deliver_at_ns, 100);
        assert_eq!(heap.pop().unwrap().deliver_at_ns, 200);
        assert_eq!(heap.pop().unwrap().deliver_at_ns, 300);
        assert!(heap.is_empty());
    }

    #[test]
    fn heap_deterministic_with_same_deadline() {
        let mut heap = TimerHeap::new();
        heap.push(make_entry(100, 5));
        heap.push(make_entry(100, 1));
        heap.push(make_entry(100, 3));

        assert_eq!(heap.pop().unwrap().intent_hash[0], 1);
        assert_eq!(heap.pop().unwrap().intent_hash[0], 3);
        assert_eq!(heap.pop().unwrap().intent_hash[0], 5);
    }

    #[test]
    fn scheduler_pop_due() {
        let mut scheduler = TimerScheduler::new();
        scheduler.heap.push(make_entry(100, 1));
        scheduler.heap.push(make_entry(200, 2));
        scheduler.heap.push(make_entry(300, 3));

        let due = scheduler.pop_due(150);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].deliver_at_ns, 100);

        let due = scheduler.pop_due(300);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].deliver_at_ns, 200);
        assert_eq!(due[1].deliver_at_ns, 300);
    }

    #[test]
    fn scheduler_next_deadline() {
        let mut scheduler = TimerScheduler::new();
        assert!(scheduler.next_deadline(0).is_none());

        scheduler.heap.push(make_entry(100, 1));
        let now_ns = 50;
        let deadline = scheduler.next_deadline(now_ns);
        assert!(deadline.is_some());

        let now_ns = 150;
        let deadline = scheduler.next_deadline(now_ns);
        assert!(deadline.is_some());
    }

    #[test]
    fn rehydrate_from_pending_only_loads_timers() {
        let mut scheduler = TimerScheduler::new();
        let contexts = vec![
            WorkflowReceiptSnapshot {
                intent_hash: [1; 32],
                effect_kind: EffectKind::TIMER_SET.to_string(),
                origin_instance_key: None,
                params_cbor: serde_cbor::to_vec(&TimerSetParams {
                    deliver_at_ns: 42,
                    key: Some("retry".into()),
                })
                .unwrap(),
                idempotency_key: [0; 32],
                issuer_ref: None,
                origin_module_id: "demo/Timer@1".into(),
                emitted_at_seq: 1,
                module_version: None,
            },
            WorkflowReceiptSnapshot {
                intent_hash: [2; 32],
                effect_kind: EffectKind::HTTP_REQUEST.to_string(),
                origin_instance_key: None,
                params_cbor: Vec::new(),
                idempotency_key: [0; 32],
                issuer_ref: None,
                origin_module_id: "demo/Http@1".into(),
                emitted_at_seq: 2,
                module_version: None,
            },
        ];

        scheduler.rehydrate_from_pending(&contexts);
        assert_eq!(scheduler.len(), 1);
        assert_eq!(scheduler.next_due_at_ns(), Some(42));
    }
}
