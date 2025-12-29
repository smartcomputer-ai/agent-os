//! Timer scheduling for daemon mode.
//!
//! Timers are special: they cannot use the standard `AsyncEffectAdapter` pattern because:
//! 1. Adapters return receipts immediately after execution
//! 2. Timer receipts must be produced later when the deadline arrives
//! 3. The daemon owns the timer lifecycle, not the adapter registry
//!
//! This module provides:
//! - `TimerEntry`: a persistable timer with logical-time deadline
//! - `TimerHeap`: min-heap ordered by deadline
//! - `TimerScheduler`: schedules timer.set intents without producing receipts

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use aos_effects::EffectIntent;
use aos_effects::builtins::TimerSetParams;
use aos_kernel::snapshot::ReducerReceiptSnapshot;
use tracing::warn;

use crate::error::HostError;

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

// Order by deadline (earliest first)
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
        // Primary: deadline (earlier is "greater" for min-heap via Reverse)
        // Secondary: intent_hash for determinism
        self.deliver_at_ns
            .cmp(&other.deliver_at_ns)
            .then_with(|| self.intent_hash.cmp(&other.intent_hash))
    }
}

/// Min-heap of timer entries ordered by deadline.
#[derive(Debug, Default)]
pub struct TimerHeap {
    heap: BinaryHeap<Reverse<TimerEntry>>,
}

impl TimerHeap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a timer entry onto the heap.
    pub fn push(&mut self, entry: TimerEntry) {
        self.heap.push(Reverse(entry));
    }

    /// Peek at the earliest deadline without removing.
    pub fn peek(&self) -> Option<&TimerEntry> {
        self.heap.peek().map(|Reverse(e)| e)
    }

    /// Pop the earliest timer entry.
    pub fn pop(&mut self) -> Option<TimerEntry> {
        self.heap.pop().map(|Reverse(e)| e)
    }

    /// Check if heap is empty.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Number of pending timers.
    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

/// Timer scheduler that handles timer.set intents without producing immediate receipts.
///
/// The scheduler:
/// 1. Parses `timer.set` intents to extract logical deadline
/// 2. Stores entries in a min-heap
/// 3. Provides methods to query next deadline and pop due timers
/// 4. Can rehydrate from pending reducer receipts on restart
#[derive(Debug, Default)]
pub struct TimerScheduler {
    heap: TimerHeap,
}

impl TimerScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedule a `timer.set` intent. Does NOT produce a receipt.
    ///
    /// The receipt will be produced later when `pop_due` returns the entry
    /// and the daemon builds+applies the receipt.
    pub fn schedule(&mut self, intent: &EffectIntent) -> Result<(), HostError> {
        let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)
            .map_err(|e| HostError::Timer(format!("failed to decode TimerSetParams: {}", e)))?;

        let entry = TimerEntry {
            deliver_at_ns: params.deliver_at_ns,
            intent_hash: intent.intent_hash,
            key: params.key,
            params_cbor: intent.params_cbor.clone(),
        };

        self.heap.push(entry);
        Ok(())
    }

    /// Get the next deadline as `Instant` for use with `tokio::time::sleep_until`.
    ///
    /// Returns `None` if no timers are scheduled.
    pub fn next_deadline(&self, now_ns: u64) -> Option<Instant> {
        self.heap.peek().map(|entry| entry.deadline_instant(now_ns))
    }

    /// Pop all timers that are due (deadline <= now_ns).
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

    /// Rehydrate timers from pending reducer receipt contexts on restart.
    ///
    /// This should be called after opening a world to restore any timers
    /// that were pending when the daemon last shut down.
    pub fn rehydrate_from_pending(&mut self, contexts: &[ReducerReceiptSnapshot]) {
        for ctx in contexts {
            if ctx.effect_kind == "timer.set" {
                // Try to decode params to get deadline
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
                        reducer = %ctx.reducer,
                        "failed to decode TimerSetParams while rehydrating timer; dropping entry"
                    );
                }
            }
        }
    }

    /// Check if any timers are scheduled.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Number of pending timers.
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

        // Should pop in order: 100, 200, 300
        assert_eq!(heap.pop().unwrap().deliver_at_ns, 100);
        assert_eq!(heap.pop().unwrap().deliver_at_ns, 200);
        assert_eq!(heap.pop().unwrap().deliver_at_ns, 300);
        assert!(heap.is_empty());
    }

    #[test]
    fn heap_deterministic_with_same_deadline() {
        let mut heap = TimerHeap::new();
        // Same deadline, different hashes
        heap.push(make_entry(100, 5));
        heap.push(make_entry(100, 1));
        heap.push(make_entry(100, 3));

        // Should pop in hash order: 1, 3, 5
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

        // At time 150, only first timer is due
        let due = scheduler.pop_due(150);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].deliver_at_ns, 100);

        // At time 250, second timer is due
        let due = scheduler.pop_due(250);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].deliver_at_ns, 200);

        // At time 400, third timer is due
        let due = scheduler.pop_due(400);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].deliver_at_ns, 300);

        assert!(scheduler.is_empty());
    }

    #[test]
    fn scheduler_pop_due_multiple() {
        let mut scheduler = TimerScheduler::new();
        scheduler.heap.push(make_entry(100, 1));
        scheduler.heap.push(make_entry(150, 2));
        scheduler.heap.push(make_entry(300, 3));

        // At time 200, first two timers are due
        let due = scheduler.pop_due(200);
        assert_eq!(due.len(), 2);
        // Should be in order
        assert_eq!(due[0].deliver_at_ns, 100);
        assert_eq!(due[1].deliver_at_ns, 150);
    }

    #[test]
    fn scheduler_next_deadline() {
        let mut scheduler = TimerScheduler::new();
        assert!(scheduler.next_deadline(0).is_none());

        scheduler.heap.push(make_entry(1_000_000_000, 1)); // 1 second from epoch

        // If now is 500ms, deadline should be ~500ms from now
        let now_ns = 500_000_000u64;
        let deadline = scheduler.next_deadline(now_ns);
        assert!(deadline.is_some());

        // If now is past deadline, should return Instant::now()
        let now_ns = 2_000_000_000u64;
        let deadline = scheduler.next_deadline(now_ns);
        assert!(deadline.is_some());
    }

    #[test]
    fn scheduler_rehydrate() {
        let mut scheduler = TimerScheduler::new();

        // Simulate pending reducer receipts
        let params = TimerSetParams {
            deliver_at_ns: 12345,
            key: Some("test-key".into()),
        };
        let params_cbor = serde_cbor::to_vec(&params).unwrap();

        let contexts = vec![
            ReducerReceiptSnapshot {
                intent_hash: [1; 32],
                reducer: "demo/Timer@1".into(),
                effect_kind: "timer.set".into(),
                params_cbor: params_cbor.clone(),
            },
            // Non-timer effect should be ignored
            ReducerReceiptSnapshot {
                intent_hash: [2; 32],
                reducer: "demo/Other@1".into(),
                effect_kind: "blob.put".into(),
                params_cbor: vec![],
            },
        ];

        scheduler.rehydrate_from_pending(&contexts);

        assert_eq!(scheduler.len(), 1);
        let entry = scheduler.heap.pop().unwrap();
        assert_eq!(entry.deliver_at_ns, 12345);
        assert_eq!(entry.key, Some("test-key".into()));
    }
}
