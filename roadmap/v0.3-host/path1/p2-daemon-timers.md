# P2: Daemon Mode + Timer Adapter

**Goal:** Long-lived world runner with real timer delivery.

## Overview

Replace stub timer with a real timer adapter that schedules OS timers. Implement a daemon loop that continuously drains the kernel and fires due timers.

## New Components

### TimerHeap

```rust
// adapters/timer.rs
use std::collections::BinaryHeap;
use std::time::Instant;

pub struct TimerHeap {
    heap: BinaryHeap<TimerEntry>,
}

#[derive(Eq, PartialEq)]
struct TimerEntry {
    deadline: Instant,
    intent_hash: [u8; 32],
    // Original params for building the fired event
    timer_id: Option<String>,
    duration_ms: u64,
}

impl TimerHeap {
    pub fn new() -> Self;

    /// Schedule a timer to fire after duration
    pub fn schedule(&mut self, duration: Duration, intent_hash: [u8; 32], timer_id: Option<String>);

    /// Get the next deadline (for tokio::time::sleep_until)
    pub fn next_deadline(&self) -> Option<Instant>;

    /// Pop all timers that are due
    pub fn pop_due(&mut self, now: Instant) -> Vec<TimerEntry>;
}
```

### Real TimerAdapter

```rust
// adapters/timer.rs
pub struct TimerAdapter {
    heap: Arc<Mutex<TimerHeap>>,
}

impl TimerAdapter {
    pub fn new() -> Self;

    /// Get shared heap for daemon to poll
    pub fn heap(&self) -> Arc<Mutex<TimerHeap>>;
}

#[async_trait]
impl AsyncEffectAdapter for TimerAdapter {
    fn kind(&self) -> &str { "timer.set" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Parse params
        let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;

        // Schedule in heap
        let duration = Duration::from_millis(params.duration_ms);
        self.heap.lock().unwrap().schedule(
            duration,
            intent.intent_hash,
            params.timer_id.clone(),
        );

        // Return immediate success receipt (timer is scheduled)
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "host.timer".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&TimerSetReceipt {
                timer_id: params.timer_id,
                scheduled_at_ms: now_ms(),
            })?,
            cost_cents: Some(0),
            signature: vec![0; 64], // TODO: real signing
        })
    }
}
```

### WorldRuntime Timer Integration

```rust
// runtime.rs additions
impl<S: Store + 'static> WorldRuntime<S> {
    /// Fire all due timers, injecting TimerFired events into the kernel
    pub fn fire_due_timers(&mut self, timer_heap: &mut TimerHeap) -> Result<usize, HostError> {
        let now = Instant::now();
        let due = timer_heap.pop_due(now);

        for entry in &due {
            // Build sys/TimerFired@1 event
            let event_value = TimerFiredEvent {
                timer_id: entry.timer_id.clone(),
                scheduled_duration_ms: entry.duration_ms,
            };
            let payload = serde_cbor::to_vec(&event_value)?;
            self.kernel.submit_domain_event("sys/TimerFired@1".into(), payload);
        }

        Ok(due.len())
    }
}
```

### WorldDaemon

```rust
// modes/daemon.rs
use tokio::sync::broadcast;

pub struct WorldDaemon<S: Store + 'static> {
    runtime: WorldRuntime<S>,
    timer_heap: Arc<Mutex<TimerHeap>>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl<S: Store + 'static> WorldDaemon<S> {
    pub fn new(runtime: WorldRuntime<S>, timer_heap: Arc<Mutex<TimerHeap>>) -> Self;

    pub async fn run(&mut self) -> Result<(), HostError> {
        tracing::info!("World daemon started");

        loop {
            // Calculate next wake time
            let next_deadline = self.timer_heap.lock().unwrap().next_deadline();
            let sleep = match next_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline.into()),
                None => tokio::time::sleep(Duration::from_secs(60)), // idle timeout
            };

            tokio::select! {
                // Timer fired
                _ = sleep => {
                    let mut heap = self.timer_heap.lock().unwrap();
                    let fired = self.runtime.fire_due_timers(&mut heap)?;
                    if fired > 0 {
                        tracing::info!("Fired {} timer(s)", fired);
                    }
                }

                // Shutdown signal
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("Shutdown signal received");
                    break;
                }
            }

            // After any wake, drain and execute
            self.drain_and_execute().await?;
        }

        // Clean shutdown: snapshot
        self.runtime.snapshot()?;
        tracing::info!("World daemon stopped");
        Ok(())
    }

    async fn drain_and_execute(&mut self) -> Result<(), HostError> {
        loop {
            let outcome = self.runtime.drain(None)?;
            let receipts = self.runtime.execute_effects().await?;

            if receipts.is_empty() && outcome.idle {
                break;
            }

            // Feed receipts back
            for receipt in receipts {
                self.runtime.enqueue_event(ExternalEvent::Receipt(receipt))?;
            }
        }
        Ok(())
    }
}
```

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
    let config = RuntimeConfig::default();

    let timer_adapter = TimerAdapter::new();
    let timer_heap = timer_adapter.heap();

    let mut runtime = WorldRuntime::open(store, &path.join("manifest.air.json"), config)?;
    runtime.adapters.register(Box::new(timer_adapter));
    // Register other adapters...

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Handle Ctrl-C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_tx.send(()).ok();
    });

    let mut daemon = WorldDaemon::new(runtime, timer_heap);
    daemon.run().await?;

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

1. Implement `TimerHeap` with min-heap ordering
2. Implement real `TimerAdapter` that schedules into heap
3. Add `fire_due_timers()` to `WorldRuntime`
4. Implement `WorldDaemon` with tokio select loop
5. Add `aos world run` CLI command
6. Add Ctrl-C shutdown handling
7. Set up tracing for pretty logging
8. Test with `examples/01-hello-timer`

## Dependencies (additions)

```toml
tokio = { version = "1", features = ["full", "signal"] }
tracing-subscriber = { version = "0.3", features = ["fmt"] }
```

## Success Criteria

- `aos world run examples/01-hello-timer` starts daemon
- Timer fires after configured delay
- Events/effects logged in real-time
- Ctrl-C triggers clean shutdown with snapshot
- Replay from snapshot works correctly
