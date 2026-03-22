# P17: Hosted Role Split and Bootstrap Refactor

**Priority**: P17  
**Effort**: High  
**Risk if deferred**: High (control, worker, and materializer stay coupled to one oversized
runtime object, making deployment splits and correctness boundaries harder to reason about)  
**Status**: Complete

## Progress Snapshot

The item is no longer just proposed. The hosted codebase has now been refactored to the intended
role split in production topology, and the remaining worker runtime is explicitly worker-scoped
rather than a cross-role hosted abstraction.

Completed so far:

- `bootstrap/` exists and now assembles explicit `ControlDeps` and `MaterializerDeps`
- `bootstrap/` now assembles explicit `ControlDeps` and `MaterializerDeps`, and broker-mode worker
  bootstrap returns a worker-scoped `HostedWorkerRuntime` directly
- `services/` exists and now contains real hosted service seams rather than just naming ideas
- service modules under `src/services/` no longer import or wrap `HostedRuntime` directly
- the worker-owned runtime has been renamed to `HostedWorkerRuntime`
- `HostedRuntime` is no longer the public cross-role name exposed by the crate
- `HostedWorkerRuntime` now lives under the `worker` module boundary rather than being re-exported
  from the crate root
- `HostedWorkerRuntimeInner` is no longer one flat bag of fields; it is split into:
  - `HostedWorkerInfra`
  - `HostedWorkerState`
- runtime-based compatibility constructors have been removed from the production types:
  - `ControlFacade::from_worker_runtime(...)` is gone
  - `HostedMaterializer::from_worker_runtime(...)` is gone
- runtime-backed test convenience now lives under explicit [test_support] rather than the main
  production API surface
- the worker app no longer carries a fake `WorkerDeps { runtime }` wrapper; it takes the
  worker-scoped runtime directly inside the worker boundary
- control no longer uses `HostedRuntime` directly in broker mode for:
  - create world
  - submit event
  - submit receipt
  - submit command / get command
  - CAS access
  - secrets
  - trace / trace-summary
  - fork seed creation and fork submission
- materializer no longer depends on `HostedRuntime` in the main broker/bootstrap path
- broker-mode control now uses standalone journal/meta/CAS/vault/replay services
- `http_control` coverage now includes a broker-mode regression that exercises the standalone
  control path
- the hosted binary now has an explicit `materializer` mode
- `control` no longer implicitly starts materializer
- `all` now composes the same control, worker, and materializer role apps rather than special
  control-side materializer wiring
- broker-mode `main.rs` no longer constructs `HostedRuntime` for control
- broker-mode `main.rs` no longer passes `HostedRuntime` through the app layer for control or all
- app entrypoints now take role-specific inputs:
  - `serve_control(ControlDeps, ...)`
  - `serve_worker(HostedWorkerRuntime, ...)`
  - `serve_materializer(MaterializerDeps, ...)`
  - `serve_all(ControlDeps, HostedWorkerRuntime, MaterializerDeps, ...)`
- hosted tests and the profiling binary now use `HostedWorkerRuntime` explicitly instead of the
  old ambiguous `HostedRuntime` name

Still incomplete:

- `HostedWorkerRuntime` still exposes a fairly broad worker/test support surface for broker and
  embedded harnesses
- some replay/query helper methods remain on `HostedWorkerRuntime` because hosted tests and
  `hosted-prof` still use them directly
- this is no longer a cross-role architecture problem; it is optional worker-internal API hygiene

So the current state is:

- control: split from worker runtime in production topology
- materializer: split from worker runtime in production topology
- worker: centered on `HostedWorkerRuntime`, which is now worker-module-only
- binary and app topology: aligned with the intended role split

## Goal

Refactor `aos-node-hosted` so the hosted system is explicitly modeled as three separate role
applications:

- control
- worker
- materializer

The desired end state is:

- each role has its own narrow dependency set
- only worker owns active `WorldHost` execution state
- control does not depend on worker runtime state
- materializer does not depend on worker runtime state
- bootstrap is shared, but runtime capability injection is role-specific
- `aos-node-hosted all` remains available for dev, but becomes simple orchestration over the same
  split apps

## Why This Exists

The current `HostedRuntime` has become the hosted god object.

Today it mixes:

- Kafka transport and local journal caches
- blobstore / CAS access
- vault access
- world registry metadata
- active worker world hosts
- create / submit APIs
- recovery / reopen / trace helpers
- utility path/config access

This is convenient in a single-process dev binary, but it hides the real hosted boundaries.

In practical hosted deployment:

- worker will run in its own pod(s)
- control will run separately
- materializer will often run separately from worker, and may or may not be colocated with control

The code should reflect those boundaries directly.

## Current Problems

### 1) `HostedRuntime` is the wrong abstraction

Control and materializer currently depend on a worker-oriented runtime wrapper because that wrapper
happens to expose the operations they need.

This is backwards.

What roles should depend on:

- control: submission, CAS, vault, projection reads, optional trace/debug replay helpers
- worker: ingress/journal/checkpoint transport, CAS, vault, active world execution
- materializer: journal read, CAS provider, projection persistence

What they should not depend on:

- control on active worker world ownership
- materializer on active worker world ownership
- materializer on worker-supervisor lifecycle

### 2) The current bootstrap still does not fully express real hosted topology

Historically:

- `run_control()` starts HTTP and a colocated materializer
- `run_all()` starts control, worker, and materializer with two `HostedRuntime` objects

This is fine for dev convenience, but it is not the right architectural center.

The architecture should be:

- one shared bootstrap layer for constructing concrete clients
- three role apps built from role-specific capabilities

### 3) Materializer was mediated through worker-oriented runtime plumbing

This has now been fixed in the main broker/bootstrap path. The remaining issue is test and helper
fallbacks that still use `HostedRuntime`.

Historically, the materializer asked a `HostedRuntime` to:

- refresh journal state from Kafka
- return partition entries
- resolve domain-scoped stores

This works, but it is the wrong boundary. The materializer should depend directly on a journal
reader and store provider.

## Design Stance

### 1) Worker is the only role that owns active world execution

Only worker should own:

- active `WorldHost`s
- timer management
- checkpoint watermark state
- speculative apply / rollback state
- ingress consumer-group ownership

Control and materializer may replay worlds ephemerally for query/debug/projection work, but they
must not be modeled as sharing worker runtime state.

### 2) Shared bootstrap is fine; shared god objects are not

We do want one place to:

- parse hosted config
- open Kafka clients
- open blobstore/CAS clients
- open vault client
- open projection store
- construct role-specific dependencies

We do **not** want one runtime object passed everywhere after bootstrap.

### 3) Capabilities should be injected narrowly

Each role should get only the interfaces it needs.

Examples:

- materializer gets journal read, CAS/store provider, projection persistence
- control gets submission API, CAS, vault, projection reads, optional trace reader
- worker gets ingress source, journal writer/reader, checkpoint store, CAS, vault

### 4) Materializer progress remains local to the projection sink

Even after the split, materializer should keep its own persisted source offsets in SQLite.

That offset means:

- "the projection DB has durably incorporated journal up to here"

It should not be replaced with Kafka consumer offsets, because consumer offsets describe transport
progress, not projection durability.

## Target Structure

Suggested `aos-node-hosted` structure:

```text
crates/aos-node-hosted/
  src/
    app/
      control.rs
      worker.rs
      materializer.rs
      all.rs

    bootstrap/
      config.rs
      clients.rs
      deps.rs

    services/
      submissions.rs
      journal.rs
      checkpoints.rs
      cas.rs
      vault.rs
      projections.rs
      replay.rs

    control/
      http.rs
      facade.rs
      ...

    worker/
      app.rs
      supervisor.rs
      execution.rs
      lifecycle.rs
      checkpoint.rs
      timers.rs
      commands.rs
      types.rs

    materializer/
      app.rs
      service.rs
      runtime.rs
      sqlite.rs
      projection.rs

    infra/
      kafka/
      blobstore/
      vault/
      state/
```

This is not just file movement. The important part is that `services/` become the role-facing
boundaries and `infra/` remains concrete backend plumbing.

## Capability Split

### Worker

Required worker-facing capabilities:

- `SubmissionSource`
  - drain owned ingress submissions
  - commit batch progress
- `JournalWriter`
  - append journal frames
- `JournalReader`
  - recover frames for replay
- `CheckpointStore`
  - read/write latest partition checkpoints
- `CasProvider`
  - domain-scoped blob and manifest access
- `VaultService`
  - secret resolution

Worker should own:

- `HostedWorker`
- supervisor loop
- partition ownership
- active world maps
- checkpoint policy and timers

### Control

Required control-facing capabilities:

- `SubmissionApi`
  - create world
  - fork world
  - submit event / receipt / command
- `CasService`
  - blob upload and metadata
- `VaultService`
  - secret bindings / versions
- `ProjectionStore`
  - materialized reads
- `TraceService`
  - replay-based trace/debug helpers

Control should not own:

- active worlds
- partition consumer groups
- checkpoint loops

### Materializer

Required materializer-facing capabilities:

- `JournalReader`
  - read partition log frames
- `ProjectionStore`
  - persist materialized state
- `CasProvider`
  - domain-scoped store resolution for manifest/snapshot/state payload reads

Materializer should not own:

- submission APIs
- vault mutation APIs
- worker active-world state

## Concrete Bootstrap Model

Introduce a shared bootstrap layer that opens concrete hosted clients once and assembles narrow
dependency sets.

### `bootstrap/config.rs`

Responsibilities:

- parse env/CLI/defaults
- define hosted role-independent config
- define role-specific config subsets

### `bootstrap/clients.rs`

Responsibilities:

- open Kafka client set(s)
- open blobstore/CAS client set
- open vault client
- open projection SQLite store
- construct store/path helpers

This layer should know how to build concrete infra clients, but not how to run worker/control/
materializer logic.

### `bootstrap/deps.rs`

Responsibilities:

- assemble `ControlDeps`
- assemble `MaterializerDeps`
- assemble or open the worker-scoped runtime for worker app bootstrap

Example shape:

```text
ControlDeps {
  submissions,
  cas,
  vault,
  projections,
  trace,
}

MaterializerDeps {
  journal_reader,
  projections,
  cas_provider,
}
```

## App Bootstrap

### Worker

Bootstrap flow:

1. open shared config
2. open concrete infra clients
3. construct worker-scoped `HostedWorkerRuntime`
4. construct `WorkerApp`
5. start supervisor / partition loop

### Control

Bootstrap flow:

1. open shared config
2. open concrete infra clients
3. assemble `ControlDeps`
4. construct `ControlFacade`
5. serve HTTP

Do not implicitly start the materializer from `run_control()` in the final architecture.

### Materializer

Bootstrap flow:

1. open shared config
2. open concrete infra clients
3. assemble `MaterializerDeps`
4. construct `MaterializerApp`
5. poll journal and persist projections

### `all`

`all` should remain a dev convenience only.

It should:

- bootstrap the same role apps
- run them in one process
- not introduce special runtime semantics

## Migration Plan

### Phase 1: Extract bootstrap and pure service seams

Status: complete

First extract the seams that are already mostly obvious:

- `VaultService`
- `ProjectionStore`
- `CasService`
- `CheckpointStore`

This should be mostly mechanical and low risk.

Completed:

- `bootstrap/deps.rs`
- `services/cas.rs`
- `services/vault.rs`
- `services/projections.rs`
- `services/journal.rs`
- `services/meta.rs`
- `services/submissions.rs`
- `services/replay.rs`

Remaining in this phase:

- none required for `p17`

### Phase 2: Move control off `HostedRuntime`

Status: complete

Refactor control to depend on:

- `SubmissionApi`
- `CasService`
- `VaultService`
- `ProjectionStore`
- `TraceService`

What is done:

- `ControlFacade` now depends on `ControlDeps`
- broker-mode control no longer relies on worker runtime state for submit/CAS/secrets/replay
- trace and fork now use replay service, not direct runtime methods

What remains:

- no required `p17` work remains in this phase

### Phase 3: Move materializer off `HostedRuntime`

Status: complete

Completed:

- materializer now uses `MaterializerDeps`
- broker/bootstrap path builds standalone journal + CAS provider dependencies
- main production path no longer routes materializer through `HostedRuntime`

Remaining:

- no required `p17` work remains in this phase

### Phase 4: Shrink worker runtime into worker-only app state

Status: complete

What remains of the old hosted runtime concept after phases 2 and 3 is now worker-owned:

- active worlds
- execution
- recovery
- checkpointing
- timers

Completed:

- `HostedRuntime` was replaced by worker-scoped `HostedWorkerRuntime`
- `HostedWorkerRuntime` now lives under the `worker` module boundary
- `HostedWorkerRuntimeInner` is split into `HostedWorkerInfra` and `HostedWorkerState`
- infra-owned helpers like domain-scoped CAS/blob-meta access now live directly on
  `HostedWorkerInfra`
- duplicate inner delegator helpers in `worker/runtime.rs` were removed so the infra/state split is
  reflected in callsites, not just field layout

### Phase 5: Simplify binaries

Status: complete

After the split:

- `run_worker()` bootstraps only worker
- `run_control()` bootstraps only control
- `run_materializer()` should exist explicitly
- `run_all()` composes the same three apps for local/dev usage

Completed:

- explicit app-layer entrypoints now exist for control, worker, materializer, and all
- the binary exposes `worker`, `control`, `materializer`, and `all`
- `run_control()` no longer starts materializer implicitly
- `all` now orchestrates the same three role apps

Remaining:

- none required for `p17`

## What This Explicitly Does Not Require

This item does **not** require:

- changing the journal-authoritative model
- making materializer use Kafka consumer-group offsets
- making control read worker memory
- making worker depend on projection SQLite
- removing `all`

## Remaining Work

`p17` itself is complete.

Optional follow-on cleanup, if we want to keep tightening the worker internals:

1. move more replay/query helpers off `HostedWorkerRuntime` into explicit test/profiler harnesses
2. further shrink the worker runtime's public surface for embedded and broker tests
3. split `services/meta.rs` into a narrower `checkpoints.rs` plus command-record metadata only if
   that naming separation becomes worthwhile

## Success Criteria

This item is complete when:

- control, worker, and materializer each have explicit role-specific app entrypoints
- control does not depend on `HostedRuntime` in production paths
- materializer does not depend on `HostedRuntime` in production paths
- only worker owns active world execution state
- bootstrap builds narrow dependency sets rather than passing one giant runtime object everywhere
- `all` is only orchestration over the same split apps

Final condition:

- the old cross-role `HostedRuntime` concept is gone; only a worker-scoped runtime remains

## Audit Note

After the final runtime pass, the remaining overlap between `src/worker/runtime.rs` and
`src/services/` is intentional:

- `services/` owns production control/materializer capability seams
- `HostedWorkerRuntime` owns worker behavior plus worker/test/profiler support helpers

The file no longer contains duplicate infra delegator methods for domain-scoped CAS/blob metadata.
