# P1: Colocated Control and Hot-World Reads

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (the unification work keeps designing around projections and the old
hosted control/worker split instead of proving the target runtime shape early)  
**Status**: In Progress  
**Depends on**:
- `roadmap/v0.19-unify/directive.md`
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`
- `roadmap/v0.18-execution/architecture/local-architecture.md`

## Goal

Define the first implementation phase for `v0.19`:

1. make hosted control read directly from a colocated `HostedWorkerRuntime`,
2. serve hosted reads from hot active-world kernel state instead of projections/materializations,
3. port the local direct-read approach into hosted rather than inventing a second read model,
4. rename hosted `submission_offset` response fields to a backend-neutral `accept_token`,
5. prove the unified control/read UX before deleting projection and materialization code.

This phase is intentionally about the control/read surface only.
It is not yet the projection deletion phase, ingress refactor phase, or backend-unification phase.

## Why This Exists

The directive is correct about the current center being wrong.

Today hosted still behaves as if the public control surface should be built around:

1. worker publishes projection records,
2. materializer rebuilds read tables,
3. control serves those read tables as the default read path.

That is the wrong first-class model for the runtime we now actually have.

Both local and hosted workers already keep worlds hot in memory.
Hosted also already has the core machinery for:

1. world registration and activation,
2. active-world kernel access,
3. world runtime state inspection,
4. in-process submission through the runtime.

So if the target architecture is a unified node with switchable backends, the first proof should be
the simplest possible one:

- colocated control reads the worker directly,
- reads come from the hot world,
- projections stop being the default control path.

That lets later cleanup become deletion work instead of architecture work.

## Current Problem

Hosted control still has the old read-side center:

1. world listing and world summaries come from `HostedProjectionStore`,
2. manifest and defs read through projection-derived head metadata,
3. `state_get` and `state_list` use materialized cell rows,
4. `journal_head` and `journal_entries` come from the materializer store,
5. `workspace_resolve` comes from workspace projection rows.

Meanwhile the hosted worker already has the real runtime state in memory and already exposes part of
the read surface directly:

1. `list_worlds`,
2. `get_world`,
3. `runtime_info`,
4. `trace_summary`,
5. ad hoc state helpers.

That split creates two bad outcomes:

1. the first implementation phase keeps designing around a read path that is meant to go away,
2. control semantics stay framed as `latest_durable` materialized reads even though the real node
   model is moving toward hot in-process reads.

## Design Stance

### 1) Control and worker are colocated from this phase onward

This phase assumes the future process model directly:

- hosted control and hosted worker are always colocated,
- one process may run separate Tokio tasks for HTTP/control and worker supervision,
- they share the same `HostedWorkerRuntime`.

Standalone remote hosted control is not a design target for this phase.

### 2) Hot worker state is the control read surface

Hosted control should read from active worlds owned by `HostedWorkerRuntime`, not from
`HostedProjectionStore`.

The worker is the only place that already has:

1. active kernels,
2. active baselines,
3. journal tail state,
4. in-flight mailbox and timer state,
5. the exact runtime view the unified node should expose.

### 3) Reads are explicitly speculative in P1

This phase does **not** introduce a new durable read fence.

If a world has pending staged slices, the hot kernel may be ahead of the last durable flush.
That is acceptable for this phase.

The intended simplicity is:

1. control reads the active world directly,
2. if a flush later fails, the worker rollback/reopen path restores correctness,
3. a future durable read-after-write mode, if needed, should be built as a waiter on the
   accept/flush path rather than as a second read model.

### 4) Reuse the local direct-read model

The hosted implementation should copy the local control/runtime read shape wherever possible.

That means hosted should gain worker methods equivalent to local direct reads for:

1. world summary/runtime,
2. manifest,
3. defs list / def get,
4. state get / state list,
5. workspace resolve,
6. journal head / entries / raw entries.

The local path is already the simplest and clearest version of the desired behavior.

### 5) Phase 1 changes the control path, not the projection runtime yet

Projection publication, projection continuity, and materializer code may remain in the tree during
P1.

But they should no longer sit on the hosted public control read path.

The P1 win is:

- projections become optional leftover machinery,
- control no longer depends on them for correctness or UX.

That sets up phase 2 as a deletion pass instead of another refactor.

### 6) Rename `submission_offset` now

Hosted acceptance responses should stop naming the returned handle as `submission_offset`.

For P1 the field should become:

- `accept_token`

It may remain a `u64` in the first implementation.
The important change is semantic:

- this is the backend-specific token returned by the acceptance path,
- not a promise that the system is fundamentally modeled around Kafka ingress offsets.

### 7) Keep the HTTP surface, change the backend

The generic control HTTP router can remain.

This phase is not about inventing new endpoints.
It is about changing what hosted control calls underneath:

- runtime-backed hot reads instead of projection/materializer reads.

## Read Semantics In P1

### Supported meaning

Hosted direct reads in P1 mean:

- current hot active-world state,
- potentially speculative relative to durable flush,
- in-process and low-latency.

### Unsupported meaning

P1 does **not** promise:

- durable read fencing,
- materialized read-replica semantics,
- remote control reading from workers it does not share a runtime with.

### Consistency parameter stance

For endpoints that currently accept a consistency query, the direct-read hosted path should be
honest about semantics.

Recommended stance:

1. omitted consistency means hot latest,
2. `latest` means hot latest,
3. `latest_durable` is not part of this phase and should not be presented as supported behavior.

Breaking this old naming is preferable to preserving a false contract.

## Scope

### [x] 1) Add worker-runtime read methods that mirror local direct reads

Add direct hosted worker methods for:

1. `manifest`,
2. `defs_list`,
3. `def_get`,
4. `state_get`,
5. `state_list`,
6. `workspace_resolve`,
7. `journal_head`,
8. `journal_entries`,
9. `journal_entries_raw`.

These methods should:

0. Check if world is already ready and hot
2. otherwise, activate it if needed
3. read directly from the active kernel state.

### [x] 2) Rewire hosted control to depend on the runtime for reads

Change hosted control bootstrap/facade so the colocated path uses `HostedWorkerRuntime` as the
authoritative read backend.

Main effect:

1. `ControlFacade` world/state/journal/workspace/manifest/defs reads stop calling
   `HostedProjectionStore`,
2. the colocated control surface becomes structurally closer to `aos-node-local`.

### [x] 3) Port the local read implementations rather than designing a hosted-specific variant

The hosted read logic should follow local behavior closely:

1. manifest and defs read from `Kernel`,
2. state reads use the kernel state/index directly,
3. workspace resolve reads from `sys/Workspace@1` state,
4. journal reads dump the hot kernel journal with current bounds.

The desired outcome is parallelism between local and hosted code paths, not a new hosted-only read
layer.

### [x] 4) Rename hosted acceptance fields to `accept_token`

Update:

1. `SubmissionAccepted`,
2. `CreateWorldAccepted`,
3. control responses,
4. worker responses,
5. tests and helper code,
6. any doc text that still describes the acceptance handle as a submission offset.

For P1 the token may still be a plain `u64`.

### [x] 5) Update hosted tests to validate hot-read behavior

Add or update tests so hosted control coverage proves:

1. read endpoints work without projection/materializer involvement,
2. state/journal/workspace/manifest/defs responses come from the worker runtime,
3. the colocated runtime shape is the supported path,
4. projection lag or absence does not break hosted control reads.

### [x] 6) Leave projection/materializer deletion for the next pass

P1 should not mix in broad deletion work.

The cleanup boundary is:

1. stop depending on projections for control reads now,
2. remove projection/materializer code in the next focused pass.

## Non-Goals

P1 does **not** attempt:

1. deleting projection publication or materializer code,
2. removing Kafka ingress,
3. introducing backend-neutral staging/flush abstraction,
4. moving discovery to checkpoint/blobstore inventory,
5. switching checkpoints from partition-based to world-based,
6. introducing switchable journal backends,
7. preserving standalone remote control semantics,
8. adding a durable read fence or wait-for-flush API.

## Deliverables

1. A hosted worker read surface that mirrors the local direct-read model.
2. A colocated hosted control facade that reads directly from `HostedWorkerRuntime`.
3. Hosted read semantics documented as hot/speculative rather than materialized/durable.
4. Hosted acceptance response fields renamed to `accept_token`.
5. Updated tests proving control reads no longer depend on projections/materialization.

## Acceptance Criteria

1. Hosted control world/list/runtime/manifest/defs/state/journal/workspace reads succeed without
   `HostedProjectionStore` on the control path.
2. Hosted control and worker run as a single colocated process shape with shared runtime access.
3. A registered world that is not yet active can still be activated and read directly by control.
4. Hosted reads reflect active hot-world state even if the world has staged but unflushed slices.
5. `submission_offset` no longer appears in the hosted acceptance surface for this phase.
6. Projection/materializer code can be deleted in a later pass without redesigning the hosted
   control surface again.

## Recommended Implementation Order

1. add local-style read helpers on `HostedWorkerRuntime`,
2. port manifest/defs/state/workspace/journal logic from local into hosted worker methods,
3. rewire `ControlFacade` and hosted bootstrap to call the runtime directly for reads,
4. rename acceptance fields to `accept_token`,
5. update hosted tests and docs to the new speculative hot-read contract,
6. leave projection deletion for the next focused cleanup phase.
