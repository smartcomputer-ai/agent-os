# P1.5: Host Test Harness & Infrastructure Consolidation

**Status:** Phase 1, 2 & 4 COMPLETE (Phase 3 pending)

**Goal:** Consolidate fragmented test infrastructure around `WorldHost`/`TestHost` so P1–P5 share a single runtime path, then complete the minimal test harness for `examples/00-counter` and `examples/01-hello-timer`.

---

## Completed Work

### Phase 1: TestHost + Fixtures (DONE)
- [x] Created `crates/aos-host/src/fixtures/mod.rs` with all fixture helpers
- [x] Expanded TestHost API: `from_loaded_manifest`, `drain_effects`, `state<T>`, `run_cycle_with_adapters`, `kernel()` escape hatches
- [x] Added `test-fixtures` feature flag to aos-host
- [x] Created 8 integration tests in `crates/aos-host/tests/testhost_integration.rs`:
  - Counter-style manifest building
  - Timer effect flow (emit → drain → inject receipt)
  - `run_cycle_batch` with stub adapters
  - Replay smoke checks
- [x] Fixed stub timer adapter to return proper `TimerSetReceipt` format

### Phase 2: Testkit Re-exports (DONE)
- [x] Added `aos-host` dependency to `aos-testkit` with `test-fixtures` feature
- [x] Converted `aos-testkit/src/lib.rs` to re-export from `aos-host::fixtures`
- [x] All 42 testkit tests pass with the new re-export layer
- [x] `TestWorld` kept for low-level kernel testing (direct synchronous access)
- [x] `TestHost` now re-exported from testkit for users wanting high-level abstraction

### Phase 4: Consolidation (DONE - done before Phase 3)
- [x] Moved `TestWorld` to `aos-host/src/fixtures/mod.rs`
- [x] Moved `MockLlmHarness` to `aos-host/src/adapters/mock.rs`
- [x] Removed `aos-testkit/src/llm_harness.rs`
- [x] Converted `aos-testkit` to pure re-export shim (~27 lines)
- [x] Moved all integration tests from `aos-testkit/tests/` to `aos-host/tests/`:
  - `helpers.rs`, `world_integration.rs`, `journal_integration.rs`
  - `snapshot_integration.rs`, `governance_integration.rs`, `policy_integration.rs`
  - `secret_integration.rs`, `effect_params_normalization.rs`
- [x] Deleted `aos-testkit/tests/` directory
- [x] Minimized `aos-testkit` to single dependency (aos-host with test-fixtures)
- [x] All 54 aos-host tests pass (original 12 + 42 migrated)

### Running Tests
```bash
# aos-host tests (with fixtures) - canonical location
cargo test -p aos-host --features test-fixtures

# aos-testkit - now a pure re-export shim (no tests)
cargo test -p aos-testkit
```

---

## Background: Current State

Three competing test harnesses exist with overlapping functionality:

| Harness | Location | Wraps | Lines |
|---------|----------|-------|-------|
| `TestHost` | `aos-host/src/testhost.rs` | `WorldHost` | ~45 |
| `TestWorld` | `aos-testkit/src/lib.rs` | `Kernel` directly | ~500 |
| `ExampleReducerHarness` | `aos-examples/src/support/reducer_harness.rs` | `Kernel` + `FsStore` | ~200 |

**Core issue**: `aos-testkit` and `aos-examples` bypass `WorldHost`, creating a layering violation and duplicating host-level concerns (effect dispatch, receipt handling, adapter registry).

---

## Target Architecture

```
aos-host (production runtime + canonical test interface)
├── WorldHost          # Production runtime (existing)
├── TestHost           # Canonical test harness (expand)
├── adapters/
│   ├── stub.rs        # Stub adapters (existing)
│   └── mock.rs        # NEW: MockHttpAdapter, MockLlmAdapter
└── fixtures/          # NEW: move from aos-testkit
    └── mod.rs         # build_loaded_manifest, stub_reducer_module, etc.

aos-testkit → thin re-export layer (eventually deprecated/removed)

aos-examples
├── support/
│   ├── manifest_loader.rs  # AIR JSON loading (stays)
│   ├── util.rs             # compile_reducer (stays)
│   └── example_host.rs     # NEW: uses TestHost, replaces ExampleReducerHarness
└── examples/               # Example runners
```

---

## Migration Phases

### Phase 1: Expand TestHost + Add Fixtures to aos-host

**Goal**: Make `TestHost` feature-complete for all test scenarios.

#### 1.1 Add fixtures module to aos-host

Create `crates/aos-host/src/fixtures/mod.rs` by extracting from `aos-testkit/src/lib.rs`:
- `build_loaded_manifest()`
- `stub_reducer_module()`, `stub_event_emitting_reducer()`
- Schema/plan/trigger helpers (`schema()`, `start_trigger()`, `text_expr()`, etc.)
- Capability helpers (`cap_http_grant()`, `timer_defcap()`, etc.)

Feature-gate with `#[cfg(feature = "test-fixtures")]`.

#### 1.2 Expand TestHost API

Add to `crates/aos-host/src/testhost.rs`:

```rust
impl<S: Store + 'static> TestHost<S> {
    // NEW: Construct from programmatic manifest (like testkit tests do)
    pub fn from_loaded_manifest(store: Arc<S>, loaded: LoadedManifest) -> Result<Self, HostError>

    // NEW: Effect inspection
    pub fn drain_effects(&mut self) -> Vec<EffectIntent>

    // NEW: Typed state access
    pub fn state<T: DeserializeOwned>(&self, reducer: &str) -> Result<T, HostError>

    // NEW: Timer support
    pub async fn run_cycle_with_timers(&mut self) -> Result<CycleOutcome, HostError>

    // NEW: Replay verification
    pub fn verify_replay(&self) -> Result<(), HostError>

    // NEW: Kernel escape hatch (for tests needing direct access)
    pub fn kernel(&self) -> &Kernel<S>
    pub fn kernel_mut(&mut self) -> &mut Kernel<S>

    // NEW: Adapter customization
    pub fn with_adapter(&mut self, adapter: Box<dyn AsyncEffectAdapter>)
}
```

#### 1.3 Add mock adapter infrastructure

Create `crates/aos-host/src/adapters/mock.rs`:
- `MockHttpAdapter` - configurable canned responses
- `MockLlmAdapter` - consolidate from `aos-testkit/src/llm_harness.rs`
- `RecordingAdapter` / `ReplayAdapter` - for deterministic replay from fixtures

---

### Phase 2: Migrate testkit Tests to TestHost

**Goal**: All `aos-testkit` integration tests run through `TestHost`.

#### 2.1 Update imports in test files

```rust
// Before
use aos_testkit::{TestWorld, fixtures};

// After
use aos_host::testhost::TestHost;
use aos_host::fixtures;
```

#### 2.2 Adapt test patterns

```rust
// Before (direct kernel access)
let mut world = TestWorld::with_store(store, loaded).unwrap();
world.submit_event_value(START_SCHEMA, &input);
world.tick_n(2).unwrap();
world.kernel.handle_receipt(receipt).unwrap();

// After (through TestHost)
let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();
host.send_event_value(START_SCHEMA, &input).unwrap();
host.run_cycle_batch().await.unwrap();
host.inject_receipt(receipt).unwrap();
host.run_cycle_batch().await.unwrap();
```

#### 2.3 Move test files

Move from `crates/aos-testkit/tests/` to `crates/aos-host/tests/`:
- `world_integration.rs`
- `journal_integration.rs`
- `governance_integration.rs`
- `snapshot_integration.rs`
- `policy_integration.rs`
- `secret_integration.rs`
- `effect_params_normalization.rs`

#### 2.4 Convert aos-testkit to re-exports

```rust
// aos-testkit/src/lib.rs
pub use aos_host::testhost::TestHost;
pub use aos_host::fixtures;

#[deprecated(note = "Use aos_host::testhost::TestHost instead")]
pub type TestWorld = TestHost<aos_store::MemStore>;
```

---

### Phase 3: Migrate Examples to TestHost

**Goal**: Examples use `TestHost` for programmatic execution.

#### 3.1 Create ExampleHost wrapper

Create `crates/aos-examples/src/support/example_host.rs`:

```rust
pub struct ExampleHost {
    host: TestHost<FsStore>,
    reducer_name: String,
    event_schema: String,
}

impl ExampleHost {
    pub fn prepare(cfg: HarnessConfig<'_>) -> Result<Self> {
        util::reset_journal(cfg.example_root)?;
        let wasm_bytes = util::compile_reducer(cfg.module_crate)?;
        let store = Arc::new(FsStore::open(cfg.example_root)?);
        // ... load manifest, patch hash ...
        let host = TestHost::from_loaded_manifest(store, loaded)?;
        Ok(Self { host, reducer_name, event_schema })
    }

    pub async fn submit_event<T: Serialize>(&mut self, event: &T) -> Result<()>
    pub fn read_state<T: DeserializeOwned>(&self) -> Result<T>
    pub fn verify_replay(&self) -> Result<()>
}
```

#### 3.2 Migrate example runners

Update each example (00-08) to use `ExampleHost` instead of `ExampleReducerHarness`.

#### 3.3 Consolidate mock adapters

- Move `HttpHarness` logic to `aos-host/src/adapters/mock.rs` as `MockHttpAdapter`
- Delete `crates/aos-examples/src/support/http_harness.rs`
- Delete `crates/aos-examples/src/support/reducer_harness.rs`

---

### Phase 4: Cleanup

**Goal**: Remove deprecated code.

1. Remove `TestWorld` type alias from aos-testkit after deprecation period
2. Consider deleting aos-testkit crate entirely (merge into aos-host)
3. Update all documentation references

---

## Files to Modify

### Create
- `crates/aos-host/src/fixtures/mod.rs`
- `crates/aos-host/src/adapters/mock.rs`
- `crates/aos-examples/src/support/example_host.rs`

### Modify
- `crates/aos-host/src/testhost.rs` - expand API
- `crates/aos-host/src/lib.rs` - add fixtures module
- `crates/aos-host/Cargo.toml` - add test feature flags
- `crates/aos-testkit/src/lib.rs` - convert to re-exports
- `crates/aos-examples/src/examples/*.rs` - use ExampleHost

### Move
- `crates/aos-testkit/tests/*.rs` → `crates/aos-host/tests/`
- `crates/aos-testkit/src/llm_harness.rs` → `crates/aos-host/src/adapters/mock.rs`

### Delete (Phase 4)
- `crates/aos-examples/src/support/reducer_harness.rs`
- `crates/aos-examples/src/support/http_harness.rs`
- Eventually: `crates/aos-testkit/` (merge into aos-host)

---

## Immediate Tasks (Original P1.5 Scope)

Complete these first to unblock P2/P3:

1. **Add `TestHost::from_loaded_manifest()` constructor** - enables programmatic manifest building
2. **Add `TestHost::drain_effects()`** - enables effect inspection in tests
3. **Add `TestHost::run_cycle_with_timers()`** - enables timer tests
4. **Add fixtures module** with `build_loaded_manifest`, `stub_reducer_module`
5. **Create integration test for `examples/00-counter`** using TestHost + `run_cycle_batch`
6. **Create integration test for `examples/01-hello-timer`** using TestHost + `run_cycle_with_timers`
7. **Add replay smoke check** - open → cycle → snapshot → reopen → assert state equality

---

## Success Criteria

### P1.5 (Immediate)
- Integration tests can drive the host through `run_cycle` (both modes) without bespoke harnesses
- Counter + timer examples pass with stub adapters and replay equality holds

### Full Consolidation
- `cargo test -p aos-host` covers all scenarios currently in aos-testkit
- Examples (00-08) run via TestHost-based harness
- `aos world step` CLI and programmatic TestHost share the same WorldHost path
- Replay-or-die check passes for all examples
- No direct Kernel usage in test code (except via `kernel()` escape hatch when needed)

---

## Open Decision

**aos-testkit disposition after migration:**

- **Option A (Merge)**: Delete aos-testkit, move everything to aos-host. Simpler dependency graph.
- **Option B (Rename)**: Keep as aos-testhost for semantic clarity. Easier for downstream users.

Decide after Phase 2 when we see how migration shakes out.
