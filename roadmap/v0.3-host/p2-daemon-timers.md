# P2: Daemon Mode + Real Timers

**Goal:** Turn batch WorldHost into a long-lived host with real timer delivery, control channel, and clean shutdown.

## Overview

Replace the stub timer with a real timer adapter that schedules OS timers. Implement a daemon loop that continuously drains the kernel, fires due timers, and responds to control-channel commands.

Restart safety note: daemon startup should call the P1 durable outbox rehydrate path (snapshot `queued_effects` + journal tail intents without receipts) to repopulate dispatch queues and the timer heap before entering the main loop. This ensures timers/effects pending at crash time are delivered exactly once after restart.

Clock source note: use a monotonic clock for `deliver_at_ns` computations and deadline comparisons to avoid wall-clock jumps (`SystemTime` leaps). Host helpers should wrap `Instant`/`CLOCK_MONOTONIC` and only translate to absolute ns for persistence.

## Critical Design Constraints

These constraints come from the existing kernel implementation and must be respected:

### 1. Timer params/receipts must use AIR types

The kernel's receipt handling ([receipts.rs:77-95](crates/aos-kernel/src/receipts.rs#L77-L95)) decodes `TimerSetParams` and `TimerSetReceipt` from [aos-effects/src/builtins/mod.rs](crates/aos-effects/src/builtins/mod.rs):

```rust
// MUST use these exact types - kernel decodes them to build sys/TimerFired@1
pub struct TimerSetParams {
    pub deliver_at_ns: u64,           // absolute monotonic time in nanoseconds
    pub key: Option<String>,          // optional correlation key
}

pub struct TimerSetReceipt {
    pub delivered_at_ns: u64,         // actual delivery time
    pub key: Option<String>,
}
```

**Do NOT** use `{ duration_ms, timer_id }` — diverging shapes cause receipt decoding failures.

If duration-style params are desired, compute `deliver_at_ns = now_ns + duration_ms * 1_000_000` in the reducer/plan before emitting the intent.

### 2. Timer receipt flow goes through `handle_receipt`

The kernel's `handle_receipt` ([world.rs:1190-1197](crates/aos-kernel/src/world.rs#L1190-L1197)):
1. Removes context from `pending_reducer_receipts`
2. Records receipt in journal
3. Builds `sys/TimerFired@1` event via `build_reducer_receipt_event()`
4. Pushes reducer event to scheduler

**Never** call `submit_domain_event("sys/TimerFired@1", ...)` directly — it bypasses journaling, leaves pending maps uncleared, and breaks duplicate handling.

### 3. Timer adapter must NOT return receipt at schedule time

If the adapter returns a receipt immediately when scheduling, `handle_receipt` processes it and emits `sys/TimerFired@1` **instantly** — the timer fires immediately instead of at the scheduled deadline.

**Correct flow:**
1. `timer.set` intent is drained from kernel
2. Adapter schedules in heap, **does not produce a receipt yet**
3. When deadline arrives, daemon builds `TimerSetReceipt` and calls `kernel.handle_receipt()`
4. Kernel builds `sys/TimerFired@1` and delivers to reducer

### 4. Use persistable deadlines, not `Instant`

`Instant` is process-local and not persistable across restarts. Store `deliver_at_ns` from the params and compute `Instant` at runtime:
- On schedule: `deadline_instant = Instant::now() + Duration::from_nanos(deliver_at_ns.saturating_sub(now_ns()))`
- On restart: rehydrate heap from pending timer contexts in snapshot/journal
- If `deliver_at_ns <= now_ns`: fire immediately on startup

## New Components

### Host Loop (daemon)

```
WorldDaemon {
  host: WorldHost,
  timer_heap: TimerHeap,
  control_rx: mpsc::Receiver<ControlMsg>,
  shutdown_rx: broadcast::Receiver<()>,
}

loop select {
  timer due  => build TimerSetReceipt, call handle_receipt, run_cycle_with_timers
  control msg=> apply command + run_cycle_with_timers
  shutdown   => snapshot + exit
}
```

**Layering note:** Keep `WorldDaemon` focused on timers, adapters, and shutdown. A separate `ControlServer` component (P3) will handle the control channel (Unix socket/stdio), translate JSON commands into `ExternalEvent`/governance events, and feed them into `WorldHost`.

- Ctrl-C triggers graceful shutdown (broadcast), final snapshot.

### TimerHeap

```rust
// adapters/timer.rs
use std::collections::BinaryHeap;
use std::time::Instant;

pub struct TimerHeap {
    heap: BinaryHeap<TimerEntry>,
}

/// Persistable timer entry - stores absolute ns deadline, not Instant
#[derive(Clone)]
pub struct TimerEntry {
    pub deliver_at_ns: u64,           // from TimerSetParams - persistable
    pub intent_hash: [u8; 32],        // for building receipt
    pub key: Option<String>,          // from TimerSetParams - for receipt
    pub params_cbor: Vec<u8>,         // original params for context
}

impl TimerEntry {
    /// Compute runtime Instant from absolute deadline
    pub fn deadline_instant(&self, now_ns: u64) -> Instant {
        if self.deliver_at_ns <= now_ns {
            Instant::now() // fire immediately
        } else {
            Instant::now() + Duration::from_nanos(self.deliver_at_ns - now_ns)
        }
    }

    /// Build receipt when timer fires
    pub fn build_receipt(&self, actual_ns: u64) -> TimerSetReceipt {
        TimerSetReceipt {
            delivered_at_ns: actual_ns,
            key: self.key.clone(),
        }
    }
}

impl TimerHeap {
    pub fn new() -> Self;

    /// Schedule a timer from a drained intent
    pub fn schedule(&mut self, intent: &EffectIntent) -> Result<(), HostError>;

    /// Get the next deadline as Instant (for tokio::time::sleep_until)
    pub fn next_deadline(&self, now_ns: u64) -> Option<Instant>;

    /// Pop all timers that are due
    pub fn pop_due(&mut self, now_ns: u64) -> Vec<TimerEntry>;

    /// Rehydrate from pending reducer receipt contexts on restart
    pub fn rehydrate_from_pending(&mut self, contexts: &[ReducerEffectContext]);
}
```

### Timer Scheduling (NOT an AsyncEffectAdapter)

Timers are special: they cannot use the standard `AsyncEffectAdapter` pattern because they must **not** return a receipt at schedule time. The receipt is produced later when the timer fires.

```rust
// adapters/timer.rs

/// Timer scheduling is handled specially by WorldDaemon, not via AdapterRegistry.
/// This is because:
/// 1. Timers must NOT return a receipt at schedule time
/// 2. The receipt is produced when the deadline arrives
/// 3. The daemon owns the timer heap and fires timers in its select loop
pub struct TimerScheduler {
    heap: TimerHeap,
}

impl TimerScheduler {
    pub fn new() -> Self;

    /// Schedule a timer.set intent. Does NOT produce a receipt.
    /// The receipt will be produced when fire_due_timers is called.
    pub fn schedule(&mut self, intent: &EffectIntent) -> Result<(), HostError> {
        let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;
        self.heap.schedule_entry(TimerEntry {
            deliver_at_ns: params.deliver_at_ns,
            intent_hash: intent.intent_hash,
            key: params.key,
            params_cbor: intent.params_cbor.clone(),
        });
        Ok(())
    }

    pub fn next_deadline(&self, now_ns: u64) -> Option<Instant>;
    pub fn pop_due(&mut self, now_ns: u64) -> Vec<TimerEntry>;
    pub fn rehydrate_from_pending(&mut self, contexts: &[ReducerEffectContext]);
}
```

### WorldHost Timer Integration

```rust
// host.rs additions
impl<S: Store + 'static> WorldHost<S> {
    /// Fire all due timers by building receipts and calling handle_receipt.
    /// This is the CORRECT way to fire timers - the kernel will:
    /// 1. Remove context from pending_reducer_receipts
    /// 2. Record receipt in journal
    /// 3. Build sys/TimerFired@1 via build_reducer_receipt_event()
    /// 4. Push reducer event to scheduler
    pub fn fire_due_timers(&mut self, scheduler: &mut TimerScheduler) -> Result<usize, HostError> {
        let now_ns = now_monotonic_ns(); // host's monotonic clock
        let due = scheduler.pop_due(now_ns);

        for entry in due {
            // Build the receipt with actual delivery time
            let timer_receipt = entry.build_receipt(now_ns);

            // Build EffectReceipt and feed through handle_receipt
            let receipt = EffectReceipt {
                intent_hash: entry.intent_hash,
                adapter_id: "host.timer".into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&timer_receipt)?,
                cost_cents: Some(0),
                signature: vec![0; 64], // TODO: real signing
            };

            // This triggers the full receipt flow in kernel
            self.kernel.handle_receipt(receipt)?;
        }

        Ok(due.len())
    }
}

/// Get current monotonic time in nanoseconds.
/// This should be consistent with whatever clock reducers use when computing deliver_at_ns.
fn now_monotonic_ns() -> u64 {
    // Use CLOCK_MONOTONIC or similar
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
```

**Important:** Never call `submit_domain_event("sys/TimerFired@1", ...)` directly. Always go through `handle_receipt` so the kernel properly manages `pending_reducer_receipts`, journaling, and duplicate detection.

### WorldDaemon

```rust
// modes/daemon.rs
use tokio::sync::broadcast;

/// ControlMsg is defined by the ControlServer (P3). Variants include:
/// - SendEvent(ExternalEvent)
/// - InjectReceipt(EffectReceipt)
/// - Snapshot
/// - Step
/// See p3-control-channel.md for the full control protocol.
pub struct WorldDaemon<S: Store + 'static> {
    host: WorldHost<S>,
    timer_scheduler: TimerScheduler,
    control_rx: mpsc::Receiver<ControlMsg>,  // fed by ControlServer (P3)
    shutdown_rx: broadcast::Receiver<()>,
}

impl<S: Store + 'static> WorldDaemon<S> {
    pub fn new(
        host: WorldHost<S>,
        control_rx: mpsc::Receiver<ControlMsg>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        let mut timer_scheduler = TimerScheduler::new();
        // Rehydrate pending timers from kernel's pending_reducer_receipts
        // (these survived in the snapshot)
        // TODO: expose pending contexts from kernel for rehydration
        Self {
            host,
            timer_scheduler,
            control_rx,
            shutdown_rx,
        }
    }

    pub async fn run(&mut self) -> Result<(), HostError> {
        tracing::info!("World daemon started");

        loop {
            // Calculate next wake time
            let now_ns = now_monotonic_ns();
            let next_deadline = self.timer_scheduler.next_deadline(now_ns);
            let sleep = match next_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline.into()),
                None => tokio::time::sleep(Duration::from_secs(60)), // idle timeout
            };

            tokio::select! {
                // Timer fired
                _ = sleep => {
                    let fired = self.host.fire_due_timers(&mut self.timer_scheduler)?;
                    if fired > 0 {
                        tracing::info!("Fired {} timer(s)", fired);
                    }
                }

                // Control message (from ControlServer)
                cmd = self.control_rx.recv() => {
                    if let Some(cmd) = cmd {
                        self.apply_control(cmd).await?;
                    } else {
                        tracing::warn!("control channel closed");
                    }
                }

                // Shutdown signal
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("Shutdown signal received");
                    break;
                }
            }

            // After any wake, run a cycle (which may produce more timer intents)
            self.run_cycle_with_timers().await?;
        }

        // Clean shutdown: snapshot
        self.host.snapshot()?;
        tracing::info!("World daemon stopped");
        Ok(())
    }

    /// Modified run_cycle that handles timer.set intents specially
    async fn run_cycle_with_timers(&mut self) -> Result<(), HostError> {
        loop {
            let drain_outcome = self.host.drain()?;

            // Drain effects from kernel (takes ownership)
            let intents = self.host.kernel_mut().drain_effects();

            if intents.is_empty() && drain_outcome.idle {
                break;
            }

            // Partition: timer.set intents go to scheduler, others to adapters
            let (timer_intents, other_intents): (Vec<_>, Vec<_>) = intents
                .into_iter()
                .partition(|i| i.kind == EffectKind::TIMER_SET);

            // Schedule timers (no receipts produced yet)
            for intent in timer_intents {
                self.timer_scheduler.schedule(&intent)?;
            }

            // Dispatch other intents via adapters (always returns receipts)
            let receipts = self.host.adapters.execute_batch(other_intents).await;

            // Feed receipts back to kernel
            for receipt in receipts {
                self.host.kernel_mut().handle_receipt(receipt)?;
            }
        }
        Ok(())
    }

    /// Applies a control command (from P3 ControlServer) and decides whether to run a cycle.
    async fn apply_control(&mut self, cmd: ControlMsg) -> Result<(), HostError> {
        match cmd {
            ControlMsg::SendEvent(evt) => {
                self.host.enqueue_external(evt)?;
            }
            ControlMsg::InjectReceipt(receipt) => {
                self.host.kernel_mut().handle_receipt(receipt)?;
            }
            ControlMsg::Snapshot => {
                self.host.snapshot()?;
            }
            ControlMsg::Step => {
                // the outer loop will run run_cycle_with_timers
            }
        }
        Ok(())
    }
}
```

**Key changes from original P2:**
1. Timer intents are partitioned out and scheduled without producing receipts
2. Timer receipts are produced later in `fire_due_timers` when deadlines arrive
3. Uses `run_cycle_with_timers` instead of the generic `run_cycle` to handle timer special-casing
4. No `pending_effects()` — uses `drain_effects()` which takes ownership

## CLI Command

```rust
// cli/commands.rs additions
#[derive(Subcommand)]
pub enum WorldCommands {
    // ... existing commands ...

    /// Run world in daemon mode
    Run {
        #[arg()]
        path: PathBuf,
    },
}

// Implementation
async fn run_daemon(path: &Path) -> Result<()> {
    let store = Arc::new(FsStore::open(path)?);
    let config = HostConfig::default();

    let mut host = WorldHost::open(store, &path.join("manifest.air.json"), config)?;
    // Register non-timer adapters (http, llm, blob)
    // Timer is handled specially by WorldDaemon, not via AdapterRegistry

    let (control_tx, control_rx) = mpsc::channel(128); // filled by ControlServer (P3)
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Handle Ctrl-C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_tx.send(()).ok();
    });

    let mut daemon = WorldDaemon::new(host, control_rx, shutdown_rx);
    daemon.run().await?; // control messages can request Step; daemon uses run_cycle_with_timers()

    Ok(())
}
```

## Pretty Logging

```rust
// Add tracing-subscriber for nice output
fn setup_logging() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_level(true)
        .init();
}

// Example output:
// INFO  World daemon started
// INFO  Event: demo/StartTimer@1
// INFO  Effect: timer.set (5000ms)
// INFO  Fired 1 timer(s)
// INFO  Event: sys/TimerFired@1
// INFO  Shutdown signal received
// INFO  World daemon stopped
```

## Tasks

1. Implement `TimerHeap` with persistable `deliver_at_ns` deadlines (not `Instant`)
2. Implement `TimerScheduler` that schedules intents without producing receipts
3. Implement `TimerEntry::build_receipt()` using AIR types (`TimerSetReceipt`)
4. Add `fire_due_timers()` to `WorldHost` that builds receipts and calls `handle_receipt`
5. Implement `WorldDaemon` select loop with timer firing + graceful shutdown
6. Implement `run_cycle_with_timers()` that partitions timer intents from other effects
7. Add timer rehydration on restart from `pending_reducer_receipts` in snapshot
8. Wire `aos world run` CLI; Ctrl-C triggers snapshot and exit
9. Set up `tracing-subscriber` for readable logs
10. Test with `examples/01-hello-timer`:
    - Timer fires at correct wall-clock time
    - `sys/TimerFired@1` event delivered to reducer
    - Replay after restart works correctly
    - Pending timers fire immediately if deadline passed during downtime

## Dependencies (additions)

```toml
tokio = { version = "1", features = ["full", "signal"] }
tracing-subscriber = { version = "0.3", features = ["fmt"] }
```

## Success Criteria

- `aos world run examples/01-hello-timer` starts daemon
- Timer fires at the correct absolute time (based on `deliver_at_ns`)
- Receipt goes through `handle_receipt` → kernel builds `sys/TimerFired@1`
- Events/effects logged in real-time
- Ctrl-C triggers clean shutdown with snapshot
- Replay from snapshot works correctly
- On restart, pending timers that missed their deadline fire immediately

## Design Decisions (resolved)

### Timer is not an AsyncEffectAdapter

Unlike HTTP/LLM/blob effects, timers cannot use the standard adapter pattern because:
1. Adapters return receipts immediately after execution
2. Timer receipts must be produced later when the deadline arrives
3. The daemon owns the timer lifecycle, not the adapter registry

### Timer intents are partitioned in the run cycle

`run_cycle_with_timers()` partitions drained intents:
- `timer.set` → scheduled in `TimerScheduler` (no receipt yet)
- Other effects → dispatched via `AdapterRegistry` (receipts returned immediately)

This means `WorldHost::run_cycle()` (from P1) is for batch mode only. Daemon mode uses `run_cycle_with_timers()`.

### Control channel is separate from daemon

`WorldDaemon` handles only: timers, adapter dispatch, shutdown.
`ControlServer` (P3) handles: Unix socket/stdio, JSON commands, governance events; `Step` commands should invoke the daemon's `run_cycle_with_timers` so timer partitioning semantics are preserved.

This keeps concerns separated and allows daemon to be tested without control channel complexity.
