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
    kernel: Kernel<S>,        // deterministic core
    store: Arc<S>,            // backing store
    adapters: AdapterRegistry, // async effect executors
}

impl<S: Store + 'static> WorldHost<S> {
    /// Open a world from a manifest path
    pub fn open(store: Arc<S>, manifest_path: &Path, config: HostConfig) -> Result<Self, HostError>;

    /// Enqueue an external event (domain event or receipt)
    pub fn enqueue(&mut self, evt: ExternalEvent) -> Result<(), HostError>;

    /// Run kernel until idle (kernel has no fuel; host may count ticks for guardrails)
    pub fn drain(&mut self) -> Result<DrainOutcome, HostError>;

    /// Inspect pending effect intents (non-destructive view of kernel queue)
    pub fn pending_effects(&self) -> Vec<EffectIntent>;

    /// Apply a receipt back into the kernel
    pub fn apply_receipt(&mut self, receipt: EffectReceipt) -> Result<(), HostError>;

    /// Convenience: dispatch intents via registered adapters, return receipts
    pub async fn dispatch_effects(&self, intents: Vec<EffectIntent>) -> Vec<Result<EffectReceipt, AdapterError>>;

    /// Query reducer state
    pub fn state(&self, reducer: &str) -> Option<&[u8]>;

    /// Create a snapshot (calls `tick_until_idle` first)
    pub fn snapshot(&mut self) -> Result<(), HostError>;

    /// Access the underlying kernel (for advanced use)
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
```

Notes:
- Fuel is not a kernel concept today; `drain` should just call `tick_until_idle` and count ticks for diagnostics/guardrails.
- Kernel keeps ownership of manifest load, journal/snapshot, deterministic stepping, effect queueing, receipt application, and state queries. Host only orchestrates adapters, process lifetime, and CLI/daemon wiring.

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
}

impl AdapterRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, adapter: Box<dyn AsyncEffectAdapter>);
    pub fn get(&self, kind: &str) -> Option<&dyn AsyncEffectAdapter>;

    /// Execute all intents, returning receipts
    pub async fn execute_batch(&self, intents: Vec<EffectIntent>) -> Vec<Result<EffectReceipt, AdapterError>>;
}
```

### BatchRunner

```rust
// modes/batch.rs
pub struct BatchRunner<S: Store + 'static> {
    host: WorldHost<S>,
}

impl<S: Store + 'static> BatchRunner<S> {
    pub fn new(host: WorldHost<S>) -> Self;

    /// Run a single batch step:
    /// 1. Inject events/receipts
    /// 2. Drain kernel to idle
    /// 3. Dispatch pending effects via adapters → collect receipts
    /// 4. Feed receipts back, drain again
    /// 5. Snapshot
    pub async fn step(&mut self, events: Vec<ExternalEvent>) -> Result<StepResult, HostError>;
}

pub struct StepResult {
    pub drain_outcome: DrainOutcome,
    pub effects_executed: usize,
    pub receipts_applied: usize,
}
```

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
4. Implement `HostConfig` (`config.rs`)
5. Implement `AsyncEffectAdapter` trait (`adapters/traits.rs`)
6. Implement `AdapterRegistry` (`adapters/registry.rs`)
7. Implement stub adapters (timer, blob, http, llm)
8. Implement `WorldHost` (`host.rs`) wrapping `Kernel`
9. Implement `BatchRunner` (`modes/batch.rs`)
10. Implement CLI commands (`cli/`)
11. Write unit tests for WorldHost (open/enqueue/drain/pending_effects/apply_receipt/snapshot)
12. Test with `examples/00-counter`

## Success Criteria

- `aos world step examples/00-counter --event demo/Increment@1 --value '{}'` runs successfully
- Batch mode completes end-to-end with stub adapters
- Unit tests pass for WorldHost core methods
