# P2: Persisted Local and Python Bindings

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (world-level validation will keep running on unrealistically narrow execution paths and authored tests will remain stuck behind bespoke Rust runners)  
**Status**: Complete  
**Depends on**: `roadmap/v0.16-factory/p1-world-harness-core.md`, `roadmap/v0.15-local/p3-local-state-root-single-world-first-and-fsstore-removal.md`

## Goal

Add the realistic persisted-local world-testing path and the first-class Python bindings surface on top of the shared harness model.

Primary outcome:

1. realistic world tests run on the local persisted runtime path,
2. worlds can be seeded/reset through shared bootstrap helpers in `aos-authoring`,
3. ordinary Python scripts can drive the harness directly through a dedicated bindings package.

## Problem Statement

The repository direction from v0.15 is already clear:

1. realistic local runtime behavior should use local state root plus local persistence,
2. smoke/eval should not keep pretending that narrow in-memory execution is sufficient for all world validation,
3. authoring/bootstrap helpers should own the seeded-local-world setup path.

What is still missing is the actual product surface for that work.

Today we still do not yet have:

1. a stable way to simulate long timer horizons without elapsed real time.

## Implementation Status

Initial P2 work landed in `aos-runtime` and `aos-authoring`:

1. `WorldHarness` now supports backend-specific `snapshot` / `reopen` hooks, which lets persisted-local worlds keep using the hosted local persistence path instead of falling back to an in-memory reopen path,
2. `aos-authoring` now exposes shared persisted-local bootstrap helpers for authored local worlds, including `bootstrap_persisted_world_harness`, `bootstrap_seeded_local_world_harness`, and `bootstrap_seeded_persisted_world_harness`,
3. seeded local bootstrap is covered by an end-to-end authoring test that builds the `00-counter` fixture, boots a persisted local world, snapshots it, and reopens it through the persisted path,
4. `crates/aos-harness-py` now exists as a dedicated Python bindings crate with a `pyproject.toml` / `maturin` package surface,
5. the Python bindings now expose both `WorkflowHarness` and `WorldHarness`, with module-centered workflow constructors and world-dir-centered world constructors,
6. Python receipt helpers now cover generic ok/error/timeout receipts plus built-in helpers for `timer.set`, `blob.put`, `blob.get`, `http.request`, and `llm.generate`,
7. `aos-authoring` now has a narrow build helper that can load a manifest directly from `air/` plus an optional workflow crate without requiring a full world root or `aos.sync.json`,
8. `crates/aos-harness-py/examples/timer_smoke.py` now proves that an ordinary Python script can point directly at a workflow crate plus minimal AIR and drive a `WorkflowHarness` end to end.

## Design Stance

### 1) Realistic world tests must use `PersistedLocal`

For world-level validation, the canonical path is:

1. resolve local state root,
2. open local persistence plus local CAS,
3. seed/bootstrap the world through shared helpers,
4. execute through the node-managed hot-world path,
5. inspect and perturb the world through stable control operations.

`PersistedLocal` should explicitly mean this existing local-node plus `HostedStore` plus `HotWorld` path.
It should not become a second persisted-local runtime implementation with different semantics.

### 2) Python is the first scripting surface

We need AI-authorable tests without compiling a new Rust runner every time.

The intended model is:

1. Rust harness core in `aos-runtime`,
2. bootstrap/import helpers in `aos-authoring`,
3. a dedicated Python wrapper crate that composes those two,
4. ordinary Python scripts as the primary authoring model.

This is explicitly library-first, not framework-first.

### 3) Keep CLI/control surfaces secondary

A thin CLI or debug control surface may still be useful.
It is not the primary product of P2.

Priority order:

1. Python bindings,
2. direct Python scripts,
3. only then small CLI/debug wrappers where they materially help.

### 4) Persisted-local tests need explicit time travel

World-level validation must be able to exercise:

1. timer-heavy workflows,
2. retries and backoff,
3. deadline-driven state changes,
4. future schedule materialization behavior,
5. month-long or cadence-based scenarios.

That means the persisted-local harness surface exposed to Python must support virtual execution time operations.
It should not require sleeping or waiting for wall-clock time to pass.

### 5) Prefer `twin` over `live` for most realistic world tests

For persisted-local world validation, the default realism ladder should be:

1. `scripted` when the test is proving narrow workflow logic or exact receipt choreography,
2. `twin` when the scenario is proving realistic dependency interaction,
3. `live` only where genuine provider interoperability is the point of the lane.

This keeps realistic world testing reproducible and cheap while still leaving room for explicit live checks.

## Scope

### [x] 1) Implement the `PersistedLocal` backend

Add a `WorldHarness` backend that is a thin wrapper over the existing realistic local execution path:

1. local state root,
2. local mutable persistence,
3. local CAS,
4. node-managed hot-world execution.

Required outcome:

1. realistic world tests use the same persisted-local semantics as the real local runtime,
2. no long-lived daemon is required just to drive those tests,
3. no parallel persisted-local runtime implementation is introduced.

Completed:

1. `PersistedLocal` is now a real `WorldHarness` backend reached through shared authoring bootstrap helpers,
2. persisted-local harnesses use backend hooks to snapshot and reopen via the hosted local persistence path,
3. `aos-runtime` remains generic and does not introduce a second persisted runtime implementation.

### [x] 2) Add shared bootstrap helpers for seeded local worlds

Shared helpers should own:

1. local state-root preparation/reset,
2. compiled module persistence,
3. authored-world import/bootstrap into local persistence,
4. clean test-world creation and teardown.

Ownership split:

1. `aos-authoring`
   - state-root preparation,
   - authored-world bootstrap/import helpers.
2. harness layer
   - world execution and inspection.

Completed:

1. `aos-authoring` now has shared helpers for backend-parameterized local harness bootstrap,
2. high-level seeded helpers combine local reset/open, authored bundle build, import/bootstrap, and harness construction,
3. these helpers return the shared `WorldHarness` surface rather than a separate authoring-only runner abstraction.

### [x] 3) Create a dedicated Python bindings crate

Create a dedicated wrapper crate and Python package that expose the harness library directly to Python.

Recommended shape:

1. wrapper crate: `crates/aos-harness-py`,
2. Python package: `aos_harness`,
3. thin boundary over `aos-runtime` and `aos-authoring`,
4. avoid binding raw kernel internals directly.

Completed:

1. `crates/aos-harness-py` is in the workspace as a mixed `pyo3` + `maturin` crate,
2. the Python module is named `aos_harness`,
3. the wrapper is thin over shared authoring/bootstrap and runtime harness surfaces rather than exposing raw kernel types.

### [x] 4) Expose the harness surface to Python

The Python surface should support ordinary scripts without a bespoke Rust runner.

Required operations:

1. `send_event`
2. `send_command`
3. `run_to_idle`
4. `pull_effects`
5. `apply_receipt`
6. `state_get`
7. `trace_summary`
8. `snapshot_create`
9. `reopen`
10. `time_get`
11. `time_set`
12. `time_advance`
13. `time_jump_next_due`
14. `artifact_export`

Expected product shape:

1. Python-first API,
2. JSON-like dict/list/bytes boundary where practical,
3. aligned with the shared harness model rather than inventing a separate framework abstraction.

Completed:

1. the bindings expose both `WorkflowHarness` and `WorldHarness` rather than routing all tests through the world bootstrap path,
2. `WorkflowHarness` now supports direct `from_workflow_dir(...)` / `from_air_dir(...)` constructors for authored module tests,
3. `WorldHarness` now exposes `from_world_dir(...)` and world-specific constructors over the shared bootstrap path,
4. the bindings expose `send_event`, `send_command`, `run_to_idle`, `run_until_runtime_quiescent`, `pull_effects`, `apply_receipt`, `state_get`, `state_bytes`, `trace_summary`, `snapshot_create`, `reopen`, `time_get`, `time_set`, `time_advance`, `time_jump_next_due`, and `artifact_export`,
5. JSON-like values cross the boundary through Python `json` encoding and bytes-oriented methods remain available where JSON is not appropriate.

### [x] 5) Add Python receipt helpers

Provide ergonomic helpers for common built-in receipt types so authored tests do not need to hand-encode every payload.

Priority helpers:

1. `http.request`
2. `llm.generate`
3. `blob.put`
4. `blob.get`
5. `timer.set`

These are helpers over the generic harness API, not a rigid DSL.

Completed:

1. the Python `WorldHarness` surface now exposes `receipt_ok`, `receipt_error`, and `receipt_timeout`,
2. built-in helpers cover `receipt_timer_set_ok`, `receipt_blob_put_ok`, `receipt_blob_get_ok`, `receipt_http_request_ok`, and `receipt_llm_generate_ok`,
3. `apply_receipt_object` accepts the helper-produced receipt dictionaries directly, so tests can create, hold, reorder, and inject receipts without hand-encoding CBOR.

### [x] 6) Prove AI-authorable Python scripts

Explicitly support tests written as ordinary Python scripts that:

1. call the harness library,
2. loop and branch,
3. inspect state and effects,
4. inject receipts,
5. write custom assertions,
6. emit artifacts.

This is required so AI agents can author useful tests without also having to generate new binaries.

Completed:

1. `crates/aos-harness-py/examples/timer_smoke.py` is a plain Python script using the published `aos_harness.WorkflowHarness` API rather than a bespoke Rust runner,
2. the script points directly at the `01-hello-timer` `workflow/` crate and sibling `air/` defs, drives scripted timer receipts, and asserts on reopened state and exported artifacts,
3. the new `aos-authoring` workflow build helper is covered by a Rust test that loads the same fixture without requiring a full world root,
4. the script was installed and executed through `maturin develop` plus a normal Python interpreter run, which proves the package shape works outside `cargo test`.

### [x] 7) Prove long-horizon timer simulation

Add explicit coverage for:

1. retry/backoff workflows with multiple timer hops,
2. deadline-driven workflows,
3. long-range timer setups that advance days or months instantly,
4. persisted-local reopen behavior with future due timers.

This is schedule-readiness work, not full recurring schedule semantics.

Completed:

1. `aos-authoring` now has a persisted-local timer harness test that stages `01-hello-timer`, schedules a timer 30 days into the future, snapshots, reopens, jumps directly to the next due time, and asserts the workflow finishes cleanly,
2. the persisted-local timer test proves reopen behavior with a future due timer and proves month-scale logical-time jumps without waiting real time,
3. the work surfaced and fixed a real daemon-reopen bug where pending `timer.set` work could be double-owned by both the scheduler and the queued effect list after reopen,
4. this is considered sufficient for the P2 timer-simulation proof bar; broader retry/backoff and deadline-specific coverage can follow as additional hardening rather than blocking closure of this item.

### [x] 8) Migrate realistic smoke/eval lanes

Move the lanes that are meant to represent realistic world behavior onto the `PersistedLocal` path.

Near-term targets:

1. world-realistic smoke flows,
2. local-product conformance scenarios,
3. broader world eval preparation work.

Completed:

1. `aos-smoke`'s shared `ExampleHost` now defaults to the `PersistedLocal` harness backend instead of the ephemeral backend, so the representative smoke lanes boot through the persisted-local harness path by default,
2. `01-hello-timer` was migrated off manual kernel timer receipt synthesis and now uses harness-level logical-time timer control (`run_cycle_with_timers` plus `time_jump_next_due`) on the persisted-local path,
3. `08-retry-backoff` now runs successfully on the persisted-local smoke path while preserving scripted receipt control for its HTTP and timer responses,
4. the migration also surfaced and fixed a missing built-in system-module resolver entry for `sys/CapEnforceHttpOut@1`, which was required for realistic HTTP-bearing smoke worlds to boot reliably.

## Non-Goals

P2 does **not** attempt:

1. the generic eval core itself,
2. a framework-y metadata model or fixture DSL,
3. judge/rubric execution logic,
4. the first sidecar factory runner,
5. full hosted or multi-universe orchestration,
6. forcing all tests onto Python execution,
7. forcing all tests onto persisted-local execution,
8. full `schedule.upsert` / cron / timezone / DST / misfire-policy semantics.

## Deliverables

1. `PersistedLocal` backend for `WorldHarness` over the existing local-node / `HostedStore` / `HotWorld` path.
2. Shared seeded-local-world bootstrap helpers in `aos-authoring`.
3. Dedicated Python bindings package.
4. Python receipt helpers for common built-ins.
5. First working Python-driven harness scripts.
6. Long-horizon timer simulation coverage.
7. Migration of realistic world-testing lanes onto the persisted-local path.

## Acceptance Criteria

1. A world author can run a realistic local test from Python without a custom Rust runner binary.
2. Python scripts can drive world execution, effect inspection, receipt injection, snapshot/reopen, time travel, and artifact export.
3. Month-long timer/deadline scenarios can be exercised without waiting real time.
4. The seeded-local-world bootstrap path is shared rather than reimplemented by smoke/eval tools.
5. The Python API is thin over the shared Rust harness model rather than being a second framework.
6. Persisted-local tests exercise the same runtime/storage path intended for real local execution.
7. `PersistedLocal` is implemented as the harness surface over the existing realistic local path, not as a second backend with divergent semantics.

## Recommended Implementation Order

1. add shared seeded-local bootstrap helpers,
2. implement `PersistedLocal` as a harness wrapper over the existing hot-world path,
3. create the Python bindings crate,
4. expose the core harness operations to Python,
5. add Python receipt helpers,
6. prove long-horizon timer simulation,
7. migrate realistic world-testing lanes.
