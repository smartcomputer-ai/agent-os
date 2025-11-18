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

11. **Example 04 — Aggregator (fan-out + join)**  
    - **Assets/layout:** create `examples/04-aggregator/{air,plans,reducer,runner,defs}` mirroring earlier demos; keep every schema/module/cap/policy/plan as canonical JSON artifacts, with hashes referenced from the manifest. Update `examples/README.md` and `aos-examples` CLI metadata to list/run the new slug (`aggregator`).  
    - **Reducer state machine:** new reducer crate emits `AggregateRequested@1` intents (keyed by `request_id`), tracks pending fan-out handles, and consumes a single `AggregateComplete` event raised by the plan to store merged payload previews + status vector. Built via `aos-wasm-build` just like Example 03; reuse reducer glue patterns to stay deterministic.  
    - **Plan definition (JSON):** author `demo/aggregator_plan@1` with 3 `emit_effect` steps (different URLs or body params) that bind handles `fetch_a/b/c`, followed by either (a) individual `await_receipt` nodes per handle or (b) shared fan-in structure that tolerates receipts arriving out of order. After receipts resolve, `assign` steps collect `status`/`body_preview`, and `raise_event` sends `AggregateComplete` back to the reducer. Ensure `required_caps = [cap_http_aggregate]`, `allowed_effects = ["http.request"]`, and guard edges only enforce data dependencies (no fake sequential wait).  
    - **Scheduler + kernel work:** update plan execution so `next_ready_step` can surface multiple ready nodes in lexical order per tick, enqueue every ready `emit_effect` before pausing, and resume `await_receipt` steps when any matching receipt arrives (deterministic tie-breaking). Add regression/unit tests in `crates/aos-kernel` that instantiate a tiny plan with parallel emits and assert receipts processed in arbitrary order still converge to the same recorded step values/state.  
    - **Shared HTTP harness:** lift the current `HttpHarness` from `fetch_notify.rs` into a reusable helper (e.g., `crates/aos-examples/src/examples/http_harness.rs`) so both demos can synthesize receipts. Aggregator runner should re-use it to emit three mock responses (with distinct payloads) and feed receipts back out of order to prove the fan-in semantics.  
    - **Runner + replay:** implement `examples/04-aggregator/runner` (Rust) to compile the reducer, load manifest assets, submit the initial Start/Aggregate event, drain HTTP effects via the shared harness (customizing bodies per request), print the aggregated result, and perform the same deterministic replay hash check as Example 03.  
    - **Testing/docs:** add CLI integration tests (or snapshot logs) ensuring `aos-examples fetch-notify` still works after harness refactor and `aos-examples aggregator` exercises the new plan. Document the scenario + how to run it in `examples/04-aggregator/README.md`. Acceptance: plan DAG executes three HTTP calls in parallel, receipts can arrive out of order, reducer state/replay hashes remain byte-identical, and JSON defs validate via existing loader.

> Once these three examples are stable, replicate the same structure for the remaining ladder rungs (plans, fan-out, governance, LLM) by adding numbered directories plus scenario implementations under the shared CLI.


## Open Questions
- reducer lib (less boiler plate), implement WIT?
- move and test manifest loader in aos-types
- how to deal with sha hashes in json? how to make this tractable. which defs SHOULD be as json and which ones can be defined in code? is it overkill now
- understand HttpHarness in fetch_notify
