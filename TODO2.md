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

12. **[x] Example 05 — Chain + Compensation (M4 multi-plan choreography)** — Four-plan saga with HTTP harness, reducer emits compensation intent on reserved failure, canonical AIR assets + runner + kernel regression tests prove deterministic ordering and refund path.

13. **[x] Governance Loop + Safe Upgrade Example (M5 core)** — Kernel now supports proposal/shadow/approve/apply with deterministic replay, capability/policy ledgers diffed via `compute_ledger_deltas`, and manifest hashes guarded through shadow → apply. Added `examples/06-safe-upgrade` with v1→v2 plan swap, new cap/policy, shadow prediction of HTTP intents + ledger deltas, and approval/apply flow exercised in the `aos-examples` runner and integration tests.
        - Reused harness patterns from fetch-notify and chain-comp; replay stays byte-identical after upgrade.

## Secrets — Implementation Tasks (see spec/17-secrets.md)

1. ✅ Secret types into AIR & enums (validation surface)
   - `SecretDecl` + `secrets` in manifest; `EffectKind`/`CapType` extended for `vault.put`/`vault.rotate`; no `secret.get`.
   - Wired through aos-air-types, aos-effects, kernel; serde/round-trip tests updated.

2. ✅ Schema & manifest validation
   - Unique (alias, version), binding_id required, expected_digest hash shape.
   - SecretRef resolution + per-secret ACL validation after normal cap/policy gates; tests added in store/air-types.

3. ✅ Secret resolver plumbing in kernel
   - `SecretResolver` trait with map and placeholder impls; threaded via KernelConfig into EffectManager; fail-closed when missing/digest mismatch; shadow can stub.

4. ✅ Effect parameter injection
   - Detects SecretRef (canonical/sugar variant), resolves via resolver, injects plaintext before dispatch; intent hash uses injected params.
   - Policy enforcement runs on original params; receipts left to adapters for redaction.
   - Integration tests in aos-testkit cover injection, binary secrets, digest mismatch, policy deny cases.

5. ✅ Policy gate & capability wiring
   - CapType mapping updated; per-secret ACL enforced pre-enqueue; tests added.

6. ✅ Examples & docs (secrets demo)
   - LLM summarizer uses a SecretRef for api_key; manifest declares `llm/api` secret; resolver in examples supplies a demo key; HTTP/LLM harnesses redact and enforce keys; README/example run works.

Next (optional)
   - Adapter-level secret redaction/meta propagation.
   - Design-time rotation example with `vault.put`/manifest patch flow.
