# P1: Local Universe Node and API Unification

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (local and hosted modes will keep drifting into different products)  
**Status**: Complete

## P1 Checklist

Completed:

- [x] Extract the shared node protocol out of `aos-fdb` into `aos-node`.
- [x] Split the protocol into focused modules and separate base node traits from hosted-only extension traits.
- [x] Move the in-memory backend into `aos-node`.
- [x] Split the memory and FoundationDB backends into folder-backed modules.
- [x] Extract the shared `/v1` control surface into `aos-node`.
- [x] Move the hosted persisted-world adapter/open path out of the runtime crate and into `aos-node`.
- [x] Invert the dependency direction so `aos-node` depends on the runtime crate rather than the other way around.
- [x] Rename the runtime crate/package to `aos-runtime`.
- [x] Introduce `aos-node-local` and `aos-node-hosted` as the concrete service compositions.
- [x] Promote the shared `/v1` node CLI surface into the canonical `aos` CLI package/binary.
- [x] Extract a shared hot-world core into `aos-node` and move both local and hosted world-open/ingress encoding logic onto it.
- [x] Add the first managed local node lifecycle surface to `aos` via `aos local up|status|down|use`.

Remaining:

- [x] Trim more hosted-only coordination detail out of `aos-node-hosted::worker` now that the shared hot-world core is real.
- [x] Remove or replace any remaining assumptions in docs and follow-on tooling that still describe the pre-node local daemon model.
- [x] Follow up on the first direct batch/local-world UX now that `aos-node-local` has an initial local-node-facing batch surface rather than leaving it only as a roadmap note.

## Goal

Refactor local runtime around the same node model as hosted runtime so that:

1. Local daemon mode is a real universe node, not a special single-world control path.
2. Local and hosted expose the same HTTP control-plane shape and semantics.
3. The CLI can target both local and hosted through the same client path.
4. Multi-world local execution is supported without changing per-world determinism.
5. Future bridge work can treat local and hosted as peer universe nodes.

Primary outcome:

- The canonical long-lived runtime unit becomes a node that owns one universe and can run many worlds.

Related design note:

- see `crate-layout.md` in this folder for the target crate and binary layout

## Why This Refactor Is Needed

Current local and hosted paths diverge at exactly the wrong boundary.

Today:

- Hosted has a universe/world control-plane model, durable runtime metadata, and a real worker/supervisor architecture.
- Local has a direct single-world daemon with a Unix socket control channel and an HTTP shim layered over that channel.
- The CLI therefore has two mental models:
  - local world directory and control socket
  - hosted universe/world HTTP API

This split is acceptable for early bring-up, but it is the wrong shape for:

1. One CLI that administers both local and hosted worlds.
2. A future local-hosted bridge.
3. Moving worlds between local and hosted without changing their operational model.
4. Running multiple local worlds under one process with shared runtime services.

The shared boundary already exists lower down:

- `aos-runtime` already provides the single-world execution engine for both filesystem-backed and hosted persisted worlds.

What is missing is a shared outer node model.

## Design Stance

### 1) Canonical long-lived runtime unit: universe node

For daemonized execution, the primary runtime unit is:

- one process
- one universe
- many worlds

This is the default for both:

- local node
- hosted node

Rationale:

- Universe is the natural boundary for shared CAS, secrets, worker state, and future bridge attachment.
- Multi-world execution belongs at the node level, not in ad hoc world-specific daemons.
- This preserves the existing architectural idea that horizontal scale comes from many worlds, while still allowing efficient local development.

### 2) Keep direct single-world batch mode

Batch mode remains a supported direct path, but the ownership depends on semantics:

- pure engine-level single-world stepping belongs to `aos-runtime`
- local persisted-world batch/dev operations belong to `aos-node-local`
- no long-lived daemon should be required for the batch/dev path itself
- useful for tests, debugging, fixtures, experimentation, and scripted local workflows

Important rule:

- batch mode is a convenience execution path, not the canonical administrative/runtime surface

### 3) Use one HTTP API shape for local and hosted

The HTTP API should be unified at the resource/semantic level, not merely made “similar”.

Required stance:

- local node and hosted node expose the same versioned `/v1` resource model
- local node keeps universe/world resources in the URL shape
- CLI may hide local default-universe selection ergonomically, but the wire shape remains shared

This means local should not keep a second private daemon API once the new node model exists.

### 4) Remove the Unix socket control channel from the long-lived local runtime path

The local control socket was a useful bring-up tool, but it should not remain the authoritative local control interface.

End-state:

- local daemon/node is administered over HTTP only
- local CLI becomes an HTTP client for daemonized local operation
- any remaining direct in-process control calls are internal implementation details, not external protocol

### 5) Reuse the worker/supervisor model, not just the HTTP surface

Do not implement local daemon orchestration as a second unrelated runtime product.

Instead, split the current hosted worker into:

- a shared hot-world runner and shared command/query service model
- a hosted distributed coordination layer
- a local single-process supervision layer

This is the critical architectural choice.

Rejected approaches:

1. Two independent HTTP implementations with only CLI sharing.
2. A shared “HTTP trait” but duplicated runtime behavior behind it.
3. Preserving process-per-world as the main local daemon architecture.
4. Forcing local to implement hosted leases and worker coordination just to share the current supervisor verbatim.

Accepted approach:

- shared node runtime semantics at the world-running layer
- shared node HTTP/control semantics
- different supervision/coordination layers for local and hosted

## Scope

### In scope

1. Define local daemonized runtime as a single-universe node.
2. Support multiple worlds in a single local node process.
3. Unify local and hosted HTTP resource model.
4. Remove the local Unix socket as the primary local daemon control plane.
5. Rework CLI paths so daemonized local and hosted both use HTTP clients.
6. Preserve direct batch mode outside the thin `aos` node client.
7. Make the local node shape explicitly bridge-ready.

### Out of scope

1. Implement the local-hosted bridge.
2. Multi-universe local nodes in the first pass.
3. Auth, tenancy, or public-network hardening for local node.
4. Removing direct single-world batch execution.
5. Full world mobility UX beyond what is needed for local node bring-up.

## Required Runtime Shape

### 1) Single local process corresponds to one universe

The initial local node should own exactly one universe.

Reasons:

- keeps local semantics simple
- matches the future bridge boundary
- avoids premature local universe multiplexing complexity
- still allows many local worlds under one process

This is a default and intentional simplification, not a forever protocol limit.

Later expansion to multi-universe local nodes is allowed, but it is not required for v0.15.

### 2) Multiple worlds may run in one local process

This is explicitly desirable and should be supported.

Important invariant:

- many worlds in one process does not mean one shared execution context

Instead:

- each world still restores independently
- each world still replays independently
- each world still runs deterministically and single-threadedly at the world execution layer
- the local node keeps many worlds hot and multiplexes them inside one process

Important local stance:

- local v0.15 is single-process and effectively single-worker
- lease acquisition, worker heartbeats, and multi-worker ownership are not part of the local base model
- future multi-threading is allowed, but it should not force hosted-style coordination into the initial local design

### 3) Same world lifecycle concepts as hosted

Local node should adopt the same concepts already used by hosted mode:

- universe
- world
- world handle
- world runtime state
- world admin lifecycle
- command submission/polling
- hot-world execution inside a node

Hosted-only concepts that local does not need to replicate in v0.15:

- lease-driven ownership
- worker fleet coordination
- distributed ready queue semantics
- durable projections as a storage requirement rather than a read implementation choice

This should be true even if the initial local operator UX makes some of these implicit.

## Crate and Ownership Direction

The crate and binary target shape for this refactor is documented in `crate-layout.md`.

The important direction for P1 is:

- keep one primary user-facing CLI: `aos`
- keep a dedicated single-world runtime crate: `aos-runtime`
- add a shared `aos-node` crate for multi-world supervision and shared HTTP/control semantics
- use separate local and hosted node service binaries as compositions of `aos-node`
- define a base node trait layer that local can implement without leases
- define hosted-only distributed coordination traits as an extension layer

Important stance:

- do not solve this by introducing a second user-facing CLI
- do not solve this by moving multi-world node supervision into the single-world runtime crate
- do not add another top-level persistence-contract crate just to name the seam

## API Unification Requirements

The unified node API should be based on the hosted `/v1` model, not the current local `/api` shim.

Local node requirements:

1. Use the same versioned routes as hosted.
2. Preserve universe/world resource nesting.
3. Expose command submission and polling rather than special local-only verbs.
4. Expose the same CAS and workspace root operations.
5. Expose the same core runtime/journal/trace surfaces.

Important clarification:

- “same HTTP API” means same resource model and same semantics for shared operations
- it does not require local to expose hosted-only worker/lease/failover endpoints
- the API should be a shared core plus hosted-only extensions

Allowed ergonomic simplifications in CLI only:

- implicit selection of the default local universe
- convenience flags that infer the local world from current directory

Not allowed:

- a permanently different local wire protocol

## Bridge Readiness Constraint

This refactor should intentionally prepare for a future bridge between local and hosted universes.

That means:

1. Local and hosted must look like peer universe nodes over HTTP.
2. World identity must remain explicit as `(universe_id, world_id)`.
3. CAS interaction must be universe-scoped and protocol-driven, not filesystem-driven.
4. World movement must continue to be expressible in terms of:
   - manifest hash
   - active baseline snapshot
   - journal tail
   - CAS closure

Implication:

- `.aos` must not be the bridge boundary

The future bridge should talk to nodes, not to local directory layouts.

## Migration Plan

### Phase 1: extract shared node contracts

Status: Completed

1. Move the hosted control facade and HTTP contracts into a shared `aos-node` layer.
2. Split current runtime traits into:
   - local-capable base node traits
   - hosted-only distributed coordination extension traits
3. Keep the single-world execution engine in `aos-runtime` rather than folding it into node orchestration.

Note:

- the shared hot-world core now lives in `aos-node`, but the hosted worker still has more hosted-only coordination logic to shrink around it
- that remaining extraction is now a focused follow-on task rather than a prerequisite for the crate split

### Phase 2: introduce local node

Status: Completed in first usable form

1. Create `aos-node-local`.
2. Boot a default local universe on startup if none exists.
3. Run shared hot-world runtime logic against SQLite persistence with single-process supervision.
4. Expose the shared `/v1` HTTP API.

Note:

- the current SQLite backend is still a pragmatic first slice built around persisted memory snapshots, not the final persistence shape

### Phase 3: cut CLI over

Status: Largely completed, with local-product follow-on work remaining

1. Point daemonized local CLI commands at the local node HTTP API.
2. Remove the Unix socket control path from primary use.
3. Keep direct batch-mode commands out of the thin `aos` client, with local persisted-world batch landing in `aos-node-local`.

Note:

- the canonical CLI is now the shared `/v1` node client
- local-target convenience flows still need follow-on work
- direct batch-mode UX should be revisited separately, with local persisted-world batch landing under `aos-node-local` rather than inside `aos`

### Phase 4: retire divergent local daemon code

Status: Mostly completed

1. Remove the old local control socket protocol.
2. Remove the local HTTP shim that only proxies to that protocol.
3. Keep only:
   - direct batch world execution
   - shared-node HTTP client path

Note:

- the legacy local daemon crate/package is gone
- the remaining cleanup is mainly stale assumptions
- the first `aos-node-local` batch/dev surface now exists
- the remaining work is to deepen that surface and finish removing stale assumptions

## Acceptance Signals

This milestone is complete when:

1. A local node process runs one universe and can host multiple worlds.
2. The same CLI client path can target hosted and daemonized local nodes.
3. The long-lived local control socket is no longer required.
4. Direct local batch execution still works for single-world experimentation.
5. Local and hosted share one core node API even though hosted keeps additional distributed-coordination extensions.
6. Local and hosted node APIs are close enough that a future bridge can treat them as peer nodes without filesystem-specific logic.
