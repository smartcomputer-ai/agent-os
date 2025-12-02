# P1: Core WorldHost + Batch Mode

**Goal:** Replace manual kernel driving with a proper host abstraction around the deterministic kernel. Get batch mode working.

## New Crate: `aos-host`

```
crates/aos-host/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── host.rs             # WorldHost - core abstraction
│   ├── config.rs           # HostConfig
│   ├── error.rs            # HostError
│   ├── adapters/
│   │   ├── mod.rs
│   │   ├── traits.rs       # AsyncEffectAdapter trait
│   │   ├── registry.rs     # AdapterRegistry
│   │   ├── timer.rs        # Stub timer adapter
│   │   ├── blob.rs         # Stub blob adapter (uses store)
│   │   ├── http.rs         # Stub HTTP adapter
│   │   └── llm.rs          # Stub LLM adapter
│   ├── modes/
│   │   ├── mod.rs
│   │   └── batch.rs        # BatchRunner
│   └── cli/
│       ├── mod.rs
│       └── commands.rs     # world init, world step
```

## Core Types

### WorldHost

```rust
// host.rs
pub struct WorldHost<S: Store + 'static> {
    kernel: Kernel<S>,         // deterministic core
    store: Arc<S>,             // backing store
    adapters: AdapterRegistry, // async effect executors
    config: HostConfig,        // retained for introspection and adapter config
}

impl<S: Store + 'static> WorldHost<S> {
    /// Open a world from a manifest path
    pub fn open(store: Arc<S>, manifest_path: &Path, config: HostConfig) -> Result<Self, HostError>;

    /// Access host configuration
    pub fn config(&self) -> &HostConfig;

    /// Enqueue an external event (domain event or receipt)
    /// Named explicitly to distinguish from kernel-internal event routing
    pub fn enqueue_external(&mut self, evt: ExternalEvent) -> Result<(), HostError>;

    /// Run kernel until idle (kernel has no fuel; host may count ticks for guardrails)
    pub fn drain(&mut self) -> Result<DrainOutcome, HostError>;

    /// Query reducer state
    /// The `key` parameter is for future keyed reducers (cells); ignored for now but
    /// included to avoid API churn when keyed routing is implemented.
    pub fn state(&self, reducer: &str, key: Option<&[u8]>) -> Option<&[u8]>;

    /// Create a snapshot (calls `tick_until_idle` first)
    pub fn snapshot(&mut self) -> Result<(), HostError>;

    /// Run one complete cycle: drain → dispatch effects → apply receipts → drain again
    /// This is the primary entry point for batch/daemon/REPL modes.
    /// Internally drains effects (taking ownership), dispatches via adapters,
    /// applies all receipts, and drains again.
    pub async fn run_cycle(&mut self) -> Result<CycleOutcome, HostError>;

    /// Access the underlying kernel (for advanced use / testing)
    pub fn kernel(&self) -> &Kernel<S>;
    pub fn kernel_mut(&mut self) -> &mut Kernel<S>;
}

pub enum ExternalEvent {
    DomainEvent { schema: String, value: Vec<u8> },
    Receipt(EffectReceipt),
}

pub struct DrainOutcome {
    pub ticks: u64,
    pub idle: bool,
}

pub struct CycleOutcome {
    pub initial_drain: DrainOutcome,
    pub effects_dispatched: usize,
    pub receipts_applied: usize,
    pub final_drain: DrainOutcome,
}
```

Notes:
- Fuel is not a kernel concept today; `drain` should just call `tick_until_idle` and count ticks for diagnostics/guardrails.
- Kernel keeps ownership of manifest load, journal/snapshot, deterministic stepping, effect queueing, receipt application, and state queries. Host only orchestrates adapters, process lifetime, and CLI/daemon wiring.
- **API naming**: `enqueue_external` mirrors the kernel's internal distinction (`submit_domain_event`/`handle_receipt` for external inputs vs `enqueue_event` for internal routing).
- **No separate `pending_effects()`**: The kernel's `drain_effects()` clears the queue and snapshots capture queued intents. A non-destructive peek would break replay semantics. Instead, `run_cycle()` takes ownership of drained intents internally.
- **Keyed state**: The `key` parameter is forward-compatible with keyed reducers (cells). `DomainEvent` already carries an optional key, and manifests declare `key_field`, but routing ignores it today. Including the parameter now avoids API churn.
- **Kernel config passthrough**: WorldHost `open` must thread `KernelConfig` (module cache dir, eager load, secret resolver/placeholder secrets) through to the kernel so host mode respects cache/secrets settings, matching the spec behavior.

#### Durable outbox / restart semantics

Goal: every intent that hits the journal either gets dispatched or already has a receipt, even if the host crashes between “intent recorded” and “receipt recorded”.

Plan:
- On startup, after kernel replay, rehydrate the dispatch queue from two sources:
  - Snapshot `queued_effects` (already persisted) → enqueue for dispatch.
  - Journal tail after the last snapshot: collect `EffectIntent` records that do not have a corresponding `EffectReceipt` record → enqueue for dispatch.
- Build a set of receipt intent_hashes from snapshot (`recent_receipts`) + tail to filter out already-completed intents so we don’t double-send.
- Timers: rebuild the timer heap from `pending_reducer_receipts` in the snapshot (these carry the params) plus any unmatched timer intents found in the tail scan.
- Implementation note: the kernel doesn’t yet expose these helpers; when landing P1, add an API to retrieve `pending_reducer_receipts`, `queued_effects`, and tail intents/receipts since the last snapshot so the host can rehydrate without poking internals.

Rationale:
- Keeps determinism: journal remains the source of truth; no second outbox store.
- Closes the crash window between intent append and snapshotting; avoids hanging worlds with “orphaned” intents.
- Avoids adapter double-dispatch by de-duping against recorded receipts.

### AsyncEffectAdapter

```rust
// adapters/traits.rs
use async_trait::async_trait;

#[async_trait]
pub trait AsyncEffectAdapter: Send + Sync {
    /// Effect kind this adapter handles (e.g., "timer.set", "http.request")
    fn kind(&self) -> &str;

    /// Execute the effect intent and return a receipt
    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("effect execution failed: {0}")]
    ExecutionFailed(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("timeout")]
    Timeout,
}
```

### AdapterRegistry

```rust
// adapters/registry.rs
pub struct AdapterRegistry {
    adapters: HashMap<String, Box<dyn AsyncEffectAdapter>>,
    config: AdapterRegistryConfig,
}

pub struct AdapterRegistryConfig {
    /// Timeout for individual effect execution; synthesizes Timeout receipt on expiry
    pub effect_timeout: Duration,
}

impl AdapterRegistry {
    pub fn new(config: AdapterRegistryConfig) -> Self;
    pub fn register(&mut self, adapter: Box<dyn AsyncEffectAdapter>);
    pub fn get(&self, kind: &str) -> Option<&dyn AsyncEffectAdapter>;

    /// Execute all intents, always returning receipts.
    ///
    /// **Error handling semantics:**
    /// - Adapter execution failures (HTTP 500, rate limits, etc.) → `ReceiptStatus::Error` receipt
    /// - Adapter timeouts → `ReceiptStatus::Timeout` receipt
    /// - Adapter unreachable (can't connect at all) → synthesize `ReceiptStatus::Error` receipt
    ///
    /// This ensures every intent gets a receipt, which is required for:
    /// - `handle_receipt` to clear pending_receipts and unblock plans/reducers
    /// - Deterministic replay (snapshots won't requeue intents without receipts)
    pub async fn execute_batch(&self, intents: Vec<EffectIntent>) -> Vec<EffectReceipt>;
}
```

**Design rationale (adapter errors vs receipts):**

| Situation | Handling | Journal Record |
|-----------|----------|----------------|
| Adapter returns success | `ReceiptStatus::Ok` | Receipt with payload |
| Adapter returns failure (HTTP 500, rate limit) | `ReceiptStatus::Error` | Receipt with error info in payload |
| Adapter times out | `ReceiptStatus::Timeout` | Synthetic timeout receipt |
| Adapter unreachable (host-level failure) | `ReceiptStatus::Error` | Synthetic error receipt |

The kernel requires every `EffectIntent` to eventually receive a `ReceiptAppended` event. If `execute_batch` returned `Err` and we dropped the intent, the world would hang forever (plans wait on `pending_receipts`). Translating all failures into receipts keeps the kernel's invariants intact across restarts.

### BatchRunner

```rust
// modes/batch.rs
pub struct BatchRunner<S: Store + 'static> {
    host: WorldHost<S>,
}

impl<S: Store + 'static> BatchRunner<S> {
    pub fn new(host: WorldHost<S>) -> Self;

    /// Run a single batch step:
    /// 1. Inject events via `enqueue_external`
    /// 2. Call `run_cycle()` (drain → dispatch → apply receipts → drain)
    /// 3. Snapshot
    ///
    /// Note: Daemon mode (P2) uses `run_cycle_with_timers()` which partitions
    /// timer intents specially. Batch mode uses the simpler `run_cycle()`.
    pub async fn step(&mut self, events: Vec<ExternalEvent>) -> Result<StepResult, HostError>;

    /// Access the underlying host
    pub fn host(&self) -> &WorldHost<S>;
    pub fn host_mut(&mut self) -> &mut WorldHost<S>;
}

pub struct StepResult {
    pub cycle: CycleOutcome,
    pub events_injected: usize,
}
```

Note: The core drain/dispatch/receipt loop lives in `WorldHost::run_cycle()`. `BatchRunner`, `WorldDaemon` (P2), and REPL (P4) all use this shared method rather than duplicating the logic.

## Stub Adapters

For P1, all adapters are stubs that return success receipts:

```rust
// adapters/timer.rs
pub struct StubTimerAdapter;

#[async_trait]
impl AsyncEffectAdapter for StubTimerAdapter {
    fn kind(&self) -> &str { "timer.set" }

    async fn execute(&self, intent: &EffectIntent) -> Result<EffectReceipt, AdapterError> {
        // Return immediate success receipt (timer "set" but won't fire)
        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "stub.timer".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: vec![],
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

// Similar stubs for http.request, llm.generate, blob.put, blob.get
```

## CLI Commands

```rust
// cli/commands.rs
#[derive(Subcommand)]
pub enum WorldCommands {
    /// Initialize a new world directory
    Init {
        #[arg()]
        path: PathBuf,
    },

    /// Run a single step in batch mode
    Step {
        #[arg()]
        path: PathBuf,

        /// Event schema to inject
        #[arg(long)]
        event: Option<String>,

        /// Event value as JSON
        #[arg(long)]
        value: Option<String>,
    },
}

// aos world init <path>
// aos world step <path> --event demo/Increment@1 --value '{}'
```

## Dependencies

```toml
# crates/aos-host/Cargo.toml
[package]
name = "aos-host"
version = "0.1.0"
edition = "2024"

[dependencies]
aos-kernel = { path = "../aos-kernel" }
aos-store = { path = "../aos-store" }
aos-effects = { path = "../aos-effects" }
aos-cbor = { path = "../aos-cbor" }

tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
anyhow = "1"
thiserror = "1"
tracing = "0.1"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Tasks

1. Create `crates/aos-host/` directory structure
2. Add `aos-host` to workspace `Cargo.toml`
3. Implement `HostError` (`error.rs`)
4. Implement `HostConfig` (`config.rs`) — include adapter timeouts, limits
5. Implement `AsyncEffectAdapter` trait (`adapters/traits.rs`)
6. Implement `AdapterRegistry` with `AdapterRegistryConfig` (`adapters/registry.rs`)
   - Implement timeout handling that synthesizes `ReceiptStatus::Timeout` receipts
   - Ensure `execute_batch` always returns receipts (never drops intents)
7. Implement stub adapters (timer, blob, http, llm)
8. Implement `WorldHost` (`host.rs`) wrapping `Kernel`
   - Store `config: HostConfig` field
   - Implement `enqueue_external()` (not `enqueue()`)
   - Implement `state(reducer, key)` with optional key parameter
   - Implement `run_cycle()` as the shared drain/dispatch/receipt loop
9. Implement `BatchRunner` (`modes/batch.rs`) using `run_cycle()`
10. Implement CLI commands (`cli/`)
11. Write unit tests for WorldHost (open/enqueue_external/drain/run_cycle/snapshot/state)
12. Test with `examples/00-counter`

## Success Criteria

- `aos world step examples/00-counter --event demo/Increment@1 --value '{}'` runs successfully
- Batch mode completes end-to-end with stub adapters
- Unit tests pass for WorldHost core methods
