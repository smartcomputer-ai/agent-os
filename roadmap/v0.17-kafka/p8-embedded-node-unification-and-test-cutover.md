# P8: Embedded Node Unification and Test Cutover

**Priority**: P8  
**Effort**: High  
**Risk if deferred**: High (tests, authoring, smoke, eval, and local product flows will keep diverging around different persistence seams)  
**Status**: Complete (shared embedded runtime/storage moved into `aos-node`; authoring/smoke/eval/python persisted-world flows now use the shared embedded harness; remaining local product-surface cutovers are deferred to separate follow-up work)

## Goal

Unify embedded local execution so that product code, tests, and developer tooling all rely on the
same node-level infrastructure rather than a mix of:

- `aos-runtime` harness-only persisted-local seams
- fs journal test paths
- sqlite journal transitional paths
- embedded local runtime planes in a separate product path

The desired end state is one embedded node path for:

- local product behavior
- authoring/bootstrap flows
- smoke runs
- agent eval runs
- python harness integration
- node-level semantic tests

## Completed In Code

Implemented on the experimental branch:

1. Shared embedded local runtime/control/worker code moved from `aos-node-local` into
   `aos-node::embedded`.
2. Local storage primitives (`FsCas`, `LocalStatePaths`, local SQLite planes, local store error)
   also moved into `aos-node` so the shared embedded seam does not depend back on
   `aos-store-local`.
3. `aos-node-local` now acts as a thin wrapper:
   - HTTP serving
   - batch entrypoints
   - local product assembly/re-exports
4. `aos-store-local` is now a compatibility re-export crate over the `aos-node` local storage
   surface rather than an architecture-defining implementation root.

Deferred follow-up:

1. Remaining local product surfaces such as secrets, forking, and workspace mutation will be
   handled in separate follow-up work rather than as part of this P item.

## Problem Statement

The current codebase still splits local execution across at least two shapes:

1. generic host/harness execution that can be wired to a journal backend
2. embedded node execution that persists runtime planes and recovers through log-first node flows

This creates avoidable drift:

- tests may validate kernel behavior against one persistence model and product behavior against
  another
- `PersistedLocal` can become a second architecture instead of a temporary seam
- local product semantics and developer-tool semantics can diverge again

That is exactly the kind of split `v0.17-kafka` is supposed to eliminate.

## Primary Stance

There should be one shared embedded node infrastructure for persisted local semantics.

That infrastructure should be used by:

- `aos-node-local`
- local authoring/bootstrap flows
- smoke
- eval
- python harnesses
- semantic integration tests that care about persisted local behavior

`aos-runtime` should remain the generic host/kernel layer.

It should not remain the place where a second persisted-local product mode lives forever.

## Repository Direction

The long-term shape should be:

- `aos-kernel`
  - deterministic kernel, snapshots, journal, replay
- `aos-runtime`
  - generic world host/execution machinery
- `aos-node`
  - shared node contracts plus shared embedded-node/local-runtime infrastructure
  - shared local storage primitives needed by that embedded runtime
- `aos-node-local`
  - thin local product wrapper: HTTP server, batch entrypoints, CLI composition
- `aos-node-hosted`
  - hosted composition on the same shared node seam
- `aos-store-local`
  - compatibility re-export crate during transition, not the architecture center of gravity

Important stance:

- do not make broad product/tooling code depend on `aos-node-local` as the architecture seam
- do not add another crate just to paper over the current split
- instead, move shared embedded-node logic to the existing shared node layer

## What Moves To Shared Embedded Node Infra

The shared embedded path should own:

- local runtime open/recovery
- durable world directory / route metadata handling
- checkpoint-head handling
- retained local authoritative history handling
- embedded control APIs over the shared node contracts
- direct non-HTTP APIs that tests and tools can call in-process

`aos-node-local` should mostly keep:

- binary entrypoints
- config parsing
- HTTP serving
- local product assembly

## What Stops Being A First-Class Product Seam

The following should be demoted, removed, or kept only as narrow test helpers:

- fs journal as a product-facing persisted mode
- raw sqlite journal as a product-facing persisted-local architecture
- `PersistedLocal` as a long-term journal-backed product mode inside `aos-runtime`

If a narrow low-level journal helper remains useful for kernel/unit tests, that is fine.

But it should not define how persisted local semantics are exercised across the repository.

## Test Strategy Reset

The testing pyramid should become clearer.

### Kernel tests

Kernel/unit tests may still use minimal in-process helpers and direct journal fixtures when the
goal is isolated kernel behavior.

### Node semantic tests

Tests for persisted local semantics should go through the shared embedded node path.

This includes:

- checkpoint and restart behavior
- route/epoch handling on the embedded path
- local world create/command/event/receipt flows
- workspace and other local product surfaces once migrated

### Hosted semantic parity

Where possible, embedded and hosted backends should continue sharing semantics-level cases for:

- replay-or-die recovery
- checkpoint and retained-history behavior
- route-epoch fencing
- canonical normalization into world history

## Tooling Consequences

The same embedded path should eventually power:

- authoring bootstrap
- smoke examples
- eval worlds
- python harness persisted-local mode

This reduces false confidence from test-only persistence seams and makes local tooling exercise the
real runtime path earlier.

## Migration Plan

### Phase 1

Land `P7` kernel journal invariant and compaction semantics first so the embedded path and hot
world lifecycle stop depending on multiple journal modes.

### Phase 2

Move shared embedded local runtime/control/recovery code out of `aos-node-local` and into the
shared `aos-node` layer.

Status: done.

### Phase 3

Cut authoring/smoke/eval/python harness persisted-local flows over to that embedded shared path.

Status: done.

### Phase 4

Retire or sharply demote:

- fs journal product usage
- raw sqlite journal product usage
- long-term `PersistedLocal` harness mode in `aos-runtime`

Status: mostly done for `aos-runtime` and shared tooling. Python still accepts `persisted_local`
as a compatibility alias, but the implementation now routes to the embedded harness.

### Phase 5

Originally scoped remaining local product-surface migration:

- secrets
- world forking
- workspace root/tree and mutation APIs

Status: deferred to separate follow-up work; this P item is otherwise complete.

## Design Constraints

This phase should preserve:

- direct in-process embedded execution for developer workflows
- the ability to test without standing up full hosted infrastructure
- a clear separation between generic kernel/host code and node/runtime orchestration

This phase should avoid:

- another architecture-significant crate whose only job is to hide the split
- making `aos-node-local` the dependency root for general tooling
- keeping multiple durable persisted-local architectures alive

## Out of Scope

1. Requiring all kernel tests to use the node layer.
2. Forcing local developer workflows to run Kafka/S3 by default.
3. Solving all future projection/read-model concerns in this cutover.

## DoD

1. The roadmap states that persisted local semantics should be exercised through one shared
   embedded node path.
2. The roadmap states that `aos-runtime` should not remain a second long-term persisted-local
   product seam.
3. The roadmap states that `aos-node-local` becomes a thin product wrapper over shared embedded
   node infrastructure.
4. The roadmap calls out authoring, smoke, eval, and python harness flows as migration targets.
5. The roadmap calls out fs/sqlite journal product seams as transitional rather than the final
   architecture.
