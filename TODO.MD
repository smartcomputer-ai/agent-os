# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Implementation Plan — Examples Ladder

1. **[x] Scaffold `examples/` layout + workspace plumbing** — `examples/0x-*` directories exist with README + subfolders, new `aos-examples` crate registers numbered demos, CLI listing/subcommands/all-mode wired up.

2. **[x] Implement shared example runner + FS-backed harness** — Harness code now builds manifests via `aos-store`, runs `aos-kernel` with FsStore/FsJournal in `.aos/`, exposes metadata-driven CLI subcommands, and replays after each demo.

3. **[x] Example 00 — CounterSM** — Reducer crate emits `alloc/step`, demo seeds Start/Tick events, prints state transitions, and verifies replay hash.

4. **[x] Example 01 — Hello Timer** — Rust manifest builders cover schemas/cap/policy, timer reducer emits `timer.set`, harness drains effects and synthesizes receipts, CLI shows Start → timer receipt → replay hash.

5. **[x] Example 02 — Blob Echo** — Reducer now emits `blob.put`/`blob.get` micro-effects, harness synthesizes Blob receipts via the CAS, CLI command prints put/get logs and replay verifies stored vs retrieved refs.

6. **[x] Upgrade Wasmtime from 17 → 36 (LTS) via staged rollouts** — Bumped embeddings through 24.x → 30.x → 36.0.3, adapted to new `wasmtime-wasi` APIs, refreshed Cargo.lock/toolchains, and revalidated all kernel/testkit/example suites on the final 36.x LTS build.

7. **[x] Design + implement `aos-wasm-build` crate (deterministic reducer compiler)** — New `crates/aos-wasm-build` exposes `Builder`/`BuildRequest`/`BuildArtifact`, Rust backend compiles reducers via `cargo build --target wasm32-unknown-unknown`, and APIs return WASM bytes + SHA-256 hashes for downstream consumers.

8. **[x] Hook builder into caching + workflows** — Builder now fingerprints reducer sources + target/profile, caches artifacts under each example’s `.aos/cache/modules`, and `aos-examples` consumes the API with a shared `--force-build` flag plus debug logging when `RUST_LOG=debug`. Directory layout matches the runtime cache (`.aos/cache/{modules|wasmtime}`) so every ladder rung has the same structure.

9. **[x] Warm-start reducers + persist Wasmtime modules** — Kernel accepts a `KernelConfig` with module cache directories, eagerly loads every reducer in a manifest, and `ReducerRuntime` serializes compiled Wasmtime modules to `<world>/.aos/cache/wasmtime/<engine>/<wasm-hash>/module.cmod`, falling back gracefully on cache misses.

10. **[x] Example 03 — Fetch & Notify (Plan + HTTP)** — Added `examples/03-fetch-notify/` with canonical JSON AIR assets, reducer crate, and runner; wired plan executor + HTTP adapter + reducer→plan trigger path so the CLI demo runs end-to-end with mocked receipts and deterministic replay.

11. **[x] Example 04 — Aggregator (fan-out + join)** — Plan + reducer demo now lives under `examples/04-aggregator` with canonical AIR assets, shared HTTP harness, kernel fan-out scheduling, runner, docs, and replay/integration tests proving three parallel HTTP calls join deterministically.

12. **[ ] Example 05 — Chain + Compensation (M4 multi-plan choreography)** — Build `examples/05-chain-comp` plus supporting kernel/tests to showcase reducer-driven sagas across three sequential plans and a compensating path.
    - **Scope:** reuse the existing HTTP harness to simulate `charge`, `reserve`, and `notify` effects while the reducer emits intents with a `request_id` correlation key. Reducer tracks saga state and issues a `refund_plan` intent if `reserve_plan` fails.
    - **Assets:** author canonical JSON for four plans (`charge_plan@1`, `reserve_plan@1`, `notify_plan@1`, `refund_plan@1`), per-plan schemas, capabilities, and a manifest wiring triggers via `correlate_by`. Add reducer WASM crate + runner to seed `EventA/B/C` and inspect recorded DomainEvents/results.
    - **Kernel/tests:** extend plan-trigger routing to ensure multiple ready plans are deterministically ordered, write regression tests in `crates/aos-kernel` that feed a failure receipt into `reserve_plan` and assert the reducer emits the compensation intent with no orphaned events, and update `aos-examples` CLI metadata/docs so the new demo is runnable.

13. **[ ] Governance Loop + Shadow Run (M5)** — Implement proposal→shadow→approval→apply flow in the kernel plus a demo showing a safe plan upgrade.
    - **Runtime:** introduce journaled governance events, shadow executor that replays manifest changes to predict `{effects_predicted, diffs, budget deltas}`, and policy/cap ledger hooks so applied manifests swap atomically. Provide CLI plumbing (`aos-examples` or minimal `aos-cli`) to submit proposals and approve them.
    - **Demo assets:** add `examples/06-safe-upgrade` with the original `fetch_plan@1`, a proposed `fetch_plan@2` (extra assign + HTTP call), and scripts/tests proving (a) shadow produces predictions, (b) approval updates the manifest root, and (c) rerunning the world executes the new plan with recorded receipts and replay-identical state.

> Once these three examples are stable, replicate the same structure for the remaining ladder rungs (plans, fan-out, governance, LLM) by adding numbered directories plus scenario implementations under the shared CLI.


## Open Questions
- reducer lib (less boiler plate), implement WIT?
- move and test manifest loader in aos-types
- how to deal with sha hashes in json? how to make this tractable. which defs SHOULD be as json and which ones can be defined in code? is it overkill now
- understand HttpHarness in fetch_notify
