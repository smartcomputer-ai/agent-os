# P2: Daemon Mode + Real Timers

**Goal:** Turn batch WorldHost into a long-lived host with real timer delivery, control channel, and clean shutdown.

## Overview

Replace the stub timer with a real timer adapter that schedules OS timers. Implement a daemon loop that continuously drains the kernel, fires due timers, and responds to control-channel commands.

## New Components

### Host Loop (daemon)

```
WorldDaemon {
  host: WorldHost,
  timer_heap: TimerHeap,
  control_rx: mpsc::Receiver<ControlMsg>,
}

loop select {
  ctrl msg   => enqueue event/receipt/proposal, trigger drain
  timer due  => inject TimerFired, drain
  idle tick  => drain if work pending
  shutdown   => snapshot + exit
}
```

- Control channel MVP: JSON over stdin/stdout or Unix socket with commands like `send-event`, `inject-receipt`, later `shadow/apply`.
- Ctrl-C triggers graceful shutdown (broadcast), final snapshot.

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

### WorldHost Timer Integration

```rust
// runtime.rs additions
impl<S: Store + 'static> WorldHost<S> {
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
    host: WorldHost<S>,
    timer_heap: Arc<Mutex<TimerHeap>>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl<S: Store + 'static> WorldDaemon<S> {
    pub fn new(host: WorldHost<S>, timer_heap: Arc<Mutex<TimerHeap>>) -> Self;

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
                    let fired = self.host.fire_due_timers(&mut heap)?;
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
        self.host.snapshot()?;
        tracing::info!("World daemon stopped");
        Ok(())
    }

    async fn drain_and_execute(&mut self) -> Result<(), HostError> {
        loop {
            let outcome = self.host.drain()?;
            let intents = self.host.pending_effects();
            let receipts = self.host.dispatch_effects(intents).await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?;

            if receipts.is_empty() && outcome.idle {
                break;
            }

            // Feed receipts back
            for receipt in receipts {
                self.host.enqueue(ExternalEvent::Receipt(receipt))?;
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
    let config = HostConfig::default();

    let timer_adapter = TimerAdapter::new();
    let timer_heap = timer_adapter.heap();

    let mut host = WorldHost::open(store, &path.join("manifest.air.json"), config)?;
    host.adapters.register(Box::new(timer_adapter));
    // Register other adapters...

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Handle Ctrl-C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_tx.send(()).ok();
    });

    let mut daemon = WorldDaemon::new(host, timer_heap);
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

1) Implement `TimerHeap` + `TimerAdapter` (real deadlines, not stub).
2) Add `fire_due_timers()` to `WorldHost`.
3) Implement `WorldDaemon` select loop with control channel + graceful shutdown.
4) Wire `aos world run` CLI; Ctrl-C triggers snapshot and exit.
5) Set up `tracing-subscriber` for readable logs.
6) Test with `examples/01-hello-timer` (fires at wall-clock times), replay after restart.

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
