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

12. **[x] Example 05 — Chain + Compensation (M4 multi-plan choreography)** — Added `examples/05-chain-comp` with four plans, reducer, CLI runner, and kernel regression tests for the refund choreography.
    - **Scope:** reuse the existing HTTP harness to simulate `charge`, `reserve`, and `notify` effects while the reducer emits intents with a `request_id` correlation key. Reducer tracks saga state and issues a `refund_plan` intent if `reserve_plan` fails.
    - **Assets:** author canonical JSON for four plans (`charge_plan@1`, `reserve_plan@1`, `notify_plan@1`, `refund_plan@1`), per-plan schemas, capabilities, and a manifest wiring triggers via `correlate_by`. Add reducer WASM crate + runner to seed `EventA/B/C` and inspect recorded DomainEvents/results.
    - **Kernel/tests:** extend plan-trigger routing to ensure multiple ready plans are deterministically ordered, write regression tests in `crates/aos-kernel` that feed a failure receipt into `reserve_plan` and assert the reducer emits the compensation intent with no orphaned events, and update `aos-examples` CLI metadata/docs so the new demo is runnable.

13. **[ ] Governance Loop + Safe Upgrade Example (M5 core)** — Build the entire governance path *inside the runtime + example harness before any CLI work*.
    - **Kernel/runtime:** add proposal, shadow, approval, and apply event types to the journal; wire state transitions so proposals enqueue manifests/caps/policies, `shadow` replays the candidate manifest deterministically to emit `{effects_predicted, diffs, budget_deltas}`, and `apply` atomically swaps the manifest root only after an approval record exists. Harden capability/policy ledgers so pending manifests cannot leak until apply time, and ensure replay reproduces the same predictions.
        - Note: `crates/aos-kernel/src/shadow/` already includes a minimal `ShadowExecutor` + config/summary structs; extend this skeleton instead of rewriting it so the new governance logs plug cleanly into the existing harness.
    - **Shadow executor:** implement a deterministic runner that boots a mirror world using the proposed manifest, drains all ready plans/effects with mocked receipts, and returns structured predictions (effect counts, capability deltas, PlanResult previews) without mutating primary state. Guard it behind manifest hashes so re-running produces identical outputs and the resulting receipts/diagnostics are journaled for audit.
    - **Example assets/tests:** add `examples/06-safe-upgrade` with canonical AIR for `fetch_plan@1` and `fetch_plan@2`. Author reducer + plan pairs plus fixtures that (a) propose v2, (b) shadow to capture predicted HTTP delta, (c) approve/apply, and (d) rerun the world to show receipts now include the second HTTP call while replay stays byte-identical. Cover the flow with integration tests in `aos-examples`/`aos-kernel` proving no orphaned events, manifest roots change only via apply, and governance logs remain deterministic even under failure injections.
        - Reuse patterns from at least two existing demos (e.g., `examples/03-fetch-notify`, `examples/05-chain-comp`) plus their corresponding `aos-examples` runners/tests so the safe-upgrade scenario feels consistent with the ladder.

14. **[ ] Governance CLI plumbing** — Once the example + runtime path exist, extend the user-facing tooling to drive it.
    - Provide `aos-examples` (or minimal `aos-cli`) subcommands for `world propose`, `world shadow`, `world approve`, and `world apply`, including human-readable dumps of predictions/budget deltas, plus journaling inspectors so the safe-upgrade scenario is runnable without bespoke scripts.

> Once these three examples are stable, replicate the same structure for the remaining ladder rungs (plans, fan-out, governance, LLM) by adding numbered directories plus scenario implementations under the shared CLI.


## Open Questions
- reducer lib (less boiler plate), implement WIT?
- move and test manifest loader in aos-types
- how to deal with sha hashes in json? how to make this tractable. which defs SHOULD be as json and which ones can be defined in code? is it overkill now
- understand HttpHarness in fetch_notify
