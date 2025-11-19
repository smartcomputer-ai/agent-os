# Example reducer harness (v0)

## Motivation

The higher-level examples (`examples/03-fetch-notify`, `examples/04-aggregator`, etc.) all repeat the same host-side ceremony to drive a reducer/plan demo:

* Bootstrap logic resets the journal directory, compiles the reducer crate to WASM, writes it to the `FsStore`, updates the manifest's module hash, and instantiates a `Kernel` with matching cache directories before the actual scenario logic starts. This pattern shows up nearly verbatim in `fetch_notify.rs` lines 41-107 and `aggregator.rs` lines 61-166.
* Each demo duplicates an ad-hoc `submit_start` helper that CBOR-encodes an event envelope, calls `kernel.submit_domain_event`, and immediately `tick_until_idle` (e.g., `fetch_notify.rs` lines 134-143, `aggregator.rs` lines 193-202).
* Replay validation always re-loads assets, patches the module hash, creates a fresh kernel, runs it to idle, and compares final reducer-state bytes plus a hash printout (see `fetch_notify.rs` lines 109-131 and `aggregator.rs` lines 167-189).
* Tiny helpers such as `patch_module_hash` are redefined per file even though they do the same `loaded.modules.get_mut(reducer_name)?.wasm_hash = wasm_hash.clone()` mutation (lines 145-151 and 205-212 respectively).

As we add Example 05+ the duplication will keep growing, while the interesting bits (HTTP harness logic, state assertions) stay small. A thin harness would keep the demo code focused on scenario-specific intent/result handling without hiding the deterministic machinery.

## Goals for a shared harness

1. **Centralize reducer manifest prep** so each example does not have to juggle `FsStore`, `HashRef`, and `LoadedManifest` plumbing.
2. **Standardize event submission** for the "seed domain event, tick kernel" workflow while still letting the demo print its own logs.
3. **Make replay verification a first-class helper** so every example gets a consistent replay check without inlining the same code.
4. **Stay incremental**: keep the surface area tiny (no procedural macros, no async) and allow examples to drop down to raw kernel APIs when needed.

## Proposed API surface (v0)

```rust
pub struct ExampleReducerHarness {
    example_root: PathBuf,
    reducer_name: Name,
    event_schema: Name,
    module_crate: &'static str,
    store: Arc<FsStore>,
    wasm_bytes: Vec<u8>,
    wasm_hash: HashRef,
    kernel_config: KernelConfig,
}
```

### Construction

```rust
impl ExampleReducerHarness {
    pub fn prepare(cfg: HarnessConfig) -> Result<Self>;
}

pub struct HarnessConfig<'a> {
    pub example_root: &'a Path,
    pub reducer_name: &'a str,
    pub event_schema: &'a str,
    pub module_crate: &'a str, // e.g. "examples/04-aggregator/reducer"
}
```

`prepare` would:

1. Call `util::reset_journal`, `util::compile_reducer`, and `util::kernel_config`.
2. Open / cache a shared `Arc<FsStore>` for the example root.
3. Load AIR assets via `manifest_loader::load_from_assets`, fail fast if missing, and patch the reducer's wasm hash exactly once.
4. Cache the compiled wasm bytes + hash for future runs (replay can reuse it without recompiling).

### Running an example session

```rust
impl ExampleReducerHarness {
    pub fn start(&self) -> Result<ExampleRun>;
}

pub struct ExampleRun {
    harness: Arc<ExampleReducerHarness>,
    kernel: Kernel<FsStore>,
}
```

`start` would build a `Kernel` from the (already patched) `LoadedManifest`, FsJournal, and KernelConfig stored on the harness. The caller owns `ExampleRun` and can still reach `kernel` directly when needed (`pub fn kernel(&mut self) -> &mut Kernel<FsStore>`).

Common helpers live on `ExampleRun`:

* `submit_event<T: Serialize>(&mut self, event: &T) -> Result<()>` — serialize with CBOR, call `submit_domain_event` with the configured `event_schema`, tick until idle, and let callers print their own human-friendly log.
* `read_state<T: DeserializeOwned>(&self) -> Result<T>` — CBOR-decode the reducer bytes for quick assertions.
* `finish(self) -> Result<ReplayHandle>` — returns ownership of the final state bytes plus a lightweight replay descriptor.

### Replay verification

```rust
pub struct ReplayHandle {
    harness: Arc<ExampleReducerHarness>,
    final_state_bytes: Vec<u8>,
}

impl ReplayHandle {
    pub fn verify_replay(mut self) -> Result<Vec<u8>>;
}
```

`verify_replay` would:

1. Spin up a brand-new kernel (reusing the cached wasm hash + manifest) and `tick_until_idle`.
2. Fetch the reducer state and compare it byte-for-byte against `final_state_bytes`.
3. Print the canonical `Hash::of_bytes` summary that every example currently logs.

The method returns the final bytes so the caller can continue inspecting them if needed.

### Putting it together (example sketch)

```rust
fn run(example_root: &Path) -> Result<()> {
    let harness = ExampleReducerHarness::prepare(HarnessConfig {
        example_root,
        reducer_name: "demo/Aggregator@1",
        event_schema: "demo/AggregatorEvent@1",
        module_crate: "examples/04-aggregator/reducer",
    })?;

    let mut run = harness.start()?;
    run.submit_event(&AggregatorEventEnvelope::Start { ... })?;

    let mut http = HttpHarness::new();
    // scenario-specific code goes here

    let state: AggregatorStateView = run.read_state()?;
    assert_eq!(state.last_responses.len(), 3);

    run.finish()?.verify_replay()?;
    Ok(())
}
```

### Extensibility hooks

* Keep `ExampleRun::kernel_mut(&mut self) -> &mut Kernel<FsStore>` so demos can interact with `HttpHarness` or custom effect drains without wrapping every API.
* Expose the compiled wasm hash on the harness so future tests can inspect it.
* Allow `HarnessConfig` to optionally supply a pre-built `LoadedManifest` builder for legacy examples (00-02) that still use in-Rust manifests.

## Benefits

* **Less duplication:** aggregator/fetch would each drop ~40 lines of setup/replay code while preserving their scenario-specific control flow.
* **Consistency:** Every new example automatically prints the same replay hash and uses the same `submit_event` semantics, making docs/tests easier to follow.
* **Deterministic reuse:** The harness caches the wasm hash + manifest patching so replay is guaranteed to use the exact same module bytes without code duplication.

## Next steps

1. Prototype the harness inside `crates/aos-examples/src/examples/` (maybe `runner.rs`) so we can refactor `fetch_notify.rs` and `aggregator.rs` incrementally.
2. After two demos adopt it, decide whether the API needs to move into a reusable crate (e.g., `aos-testkit`).
3. Add unit tests for the harness itself to ensure manifest patching and replay verification behave like today's handwritten code.
