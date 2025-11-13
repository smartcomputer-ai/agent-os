# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Implementation Plan — Examples Ladder

1. **[x] Scaffold `examples/` layout + workspace plumbing** — `examples/0x-*` directories exist with README + subfolders, new `aos-examples` crate registers numbered demos, CLI listing/subcommands/all-mode wired up.

2. **[x] Implement shared example runner + FS-backed harness** — Harness code now builds manifests via `aos-store`, runs `aos-kernel` with FsStore/FsJournal in `.aos/`, exposes metadata-driven CLI subcommands, and replays after each demo.

3. **[x] Example 00 — CounterSM** — Reducer crate emits `alloc/step`, demo seeds Start/Tick events, prints state transitions, and verifies replay hash.

4. **[x] Example 01 — Hello Timer** — Rust manifest builders cover schemas/cap/policy, timer reducer emits `timer.set`, harness drains effects and synthesizes receipts, CLI shows Start → timer receipt → replay hash.

5. **[x] Example 02 — Blob Echo** — Reducer now emits `blob.put`/`blob.get` micro-effects, harness synthesizes Blob receipts via the CAS, CLI command prints put/get logs and replay verifies stored vs retrieved refs.

6. **Upgrade Wasmtime from 17 → 36 (LTS) via staged rollouts**  
   - **Stage A:** bump to Wasmtime 24.x LTS (first release with component model + WASI 0.2 defaults), update Cargo.lock, clear `.aos/cache`, rerun all WASI integration tests.  
   - **Stage B:** refactor embedding code for the `wasmtime-wasi` API changes introduced around 30.x (split `WasiView`, stream trait renames), land on Wasmtime 30.x and verify host adapters.  
   - **Stage C:** upgrade to Wasmtime 36.x (current LTS). Update toolchains to Rust ≥1.87, validate async/component defaults and Cranelift changes, then freeze on 36.x for long-term support before tackling `aos-wasm-build`.

7. **Design + implement `aos-wasm-build` crate (deterministic reducer compiler)**  
   - Create `crates/aos-wasm-build/` exposing a library API (`Builder`, `BuildRequest`, `BuildArtifact`) that accepts a reducer workspace path + build options (toolchain, profile, features) and returns `{wasm_bytes, wasm_hash, build_log}`.  
   - Layout: `src/lib.rs` re-exports; `src/config.rs` for toolchain pinning (Rust toolchain file, target triple), `src/artifact.rs` for hash + metadata, `src/backends/mod.rs` with `rust.rs` implementing the first backend via cargo as a subprocess (deterministic env, `wasm32-unknown-unknown`, release).  
   - API sketch: `let artifact = Builder::new().backend(Backend::Rust).request(BuildRequest::from_path(path)).compile()?;` returning a struct with `wasm_hash()` and `write_to(dir)` helpers. Later backends (TS, Python) can implement the same `ModuleCompiler` trait inside this crate.

8. **Hook builder into caching + workflows**  
   - Introduce a cache store (e.g., `.aos/cache/modules/<hash>.wasm`) keyed by `(source_digest, toolchain_version)` inside `aos-wasm-build`; `Builder` should check cache before spawning the backend, and materialize hits without re-running cargo.  
   - Expose cache metadata so `aos-examples`/future CLI can reuse compiled reducers, and add a CLI command (later) that reports cache status / invalidates entries.

> Once these three examples are stable, replicate the same structure for the remaining ladder rungs (plans, fan-out, governance, LLM) by adding numbered directories plus scenario implementations under the shared CLI.

- [x] Cache compiled reducer modules/instances in `aos-wasm` so each reducer tick doesn’t recompile the WASM module (current per-event latency is dominated by fresh instantiation). — `ReducerRuntime::run` now hashes the WASM bytes and reuses compiled `wasmtime::Module`s from an in-memory cache, avoiding redundant recompilation within a process.
