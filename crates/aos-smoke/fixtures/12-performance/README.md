# 12-performance

Performance smoke fixture for event ingress throughput against a simple counter workflow.

It runs two variants:
- non-keyed routing (empty cell / single workflow instance)
- keyed routing (round-robin across configurable cells)

Run:
- `cargo run -p aos-smoke -- performance`
- `cargo run -p aos-smoke -- performance --messages 1000 --cells 32`
- `cargo run -p aos-smoke -- performance --messages 10000 --cells 10 --in-memory`

## Profiling with Samply

Install `samply` (if not already installed):
- `cargo install samply`

Run profiler (filesystem-backed mode):
- `cargo build -p aos-smoke`
- `samply record -- target/debug/aos-smoke performance --messages 10000 --cells 10`

Run profiler (in-memory CAS+journal mode):
- `cargo build -p aos-smoke`
- `samply record -- target/debug/aos-smoke performance --messages 10000 --cells 10 --in-memory`

Notes:
- `samply` opens the profile in a browser (Firefox Profiler UI).
- Use the in-memory mode to inspect kernel/wasm/index overhead without fsync-heavy journal costs.

## Main Bottlenecks Identified So Far

These are based on post-warmup scoped timers plus sampling profiles from a debug run.

1. Journal durability (`fsync`) dominates per-event ingress time.
   - Hot path: `Kernel::append_record -> FsJournal::append -> File::sync_all`.
   - In sampled runs, `sync_all` was the single largest self-time bucket (~60%).
   - Meaning: throughput is currently bounded by forced durability on each append, not by workflow logic.

2. Workflow invocation has high per-event Wasmtime setup overhead.
   - Hot path: `WorkflowRegistry::invoke -> WorkflowRuntime::run_compiled`.
   - Per invocation, runtime creates a new `Store`, creates a `Linker`, instantiates module, resolves exports, then calls `step`.
   - In sampled runs, setup/instantiation + call stack was a major bucket (~10%+ total).

3. Keyed workflows still pay for snapshot/base reads, but no longer rewrite the persistent cell index on every event.
   - The kernel now keeps post-snapshot keyed head state in an in-memory delta and only materializes new state blobs/index roots at snapshot time.
   - That removes the worst former write amplification path for hot keyed cells.
   - Remaining keyed costs are now mostly journal durability plus head-view bookkeeping/reads.

4. Per-event `run_to_idle` loop amplifies fixed overhead.
   - Scoped timers show `encode` is negligible, while `submit` and `drain` dominate post-warmup event cost.
   - Meaning: fixed runtime/journal/index costs are paid once per event rather than amortized.

## Proposed Solutions

1. Add an opt-in non-durable performance mode for benchmarking.
   - Keep default behavior unchanged.
   - Perf options: `sync_every_n`, `sync_on_shutdown`, or `never` (bench/dev only).
   - Purpose: isolate kernel/runtime compute from storage durability limits.

2. Reduce journal append overhead even in durable mode.
   - Keep journal file handle open instead of opening on every append.
   - Consider `sync_data` where acceptable instead of `sync_all`.
   - Batch sync policy (every N records or interval) for configurable profiles.

3. Reduce Wasmtime per-event setup cost.
   - Reuse a `Linker` at runtime level.
   - Cache exports and avoid repeated lookup where possible.
   - Add instance/store pooling (or reusable invocation context) per module.

4. Optimize keyed cell-index/store access further.
   - Introduce an in-process node cache for local CAS-backed node reads.
   - The kernel already keeps hot keyed state in memory and flushes it on snapshot/checkpoint.
   - Next step is reducing remaining base-index/CAS lookup overhead on head reads and snapshot commit.

5. Add batching mode to the smoke benchmark for comparison.
   - Current mode: send event + drain to idle per event.
   - Additional mode: submit many events then drain once.
   - This quantifies fixed per-event overhead versus actual reducer throughput.
