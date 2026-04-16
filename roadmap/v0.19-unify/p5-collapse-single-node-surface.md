# P5: Collapse To A Single Node Surface

**Priority**: P5  
**Effort**: Large  
**Risk if deferred**: Medium (the runtime architecture is now unified enough that the remaining
complexity is mostly crate/product duplication and naming)  
**Status**: Implemented through P5.6  
**Depends on**:
- `roadmap/v0.19-unify/directive.md`
- `roadmap/v0.19-unify/p4-switchable-journal-backends.md`

## Goal

Define the fifth implementation phase for `v0.19` around one clear change:

1. make `aos-node` the only node library crate,
2. move the real runtime/backend/control implementation into that crate,
3. make `aos-cli` the primary node binary surface,
4. delete `aos-node-hosted` and `aos-node-local` as separate product crates,
5. keep only backend/config differences such as Kafka vs SQLite, object-store vs local filesystem
   metadata, and vault vs `.env` secrets.

This phase is not another runtime redesign.
It is the product/crate collapse that follows from P2-P4 already being in place.

## Why This Exists

The original split between `aos-node-local` and `aos-node-hosted` was justified when they still
represented meaningfully different runtime architectures.

That is no longer true:

1. direct HTTP ingress is the default,
2. reads are hot-world reads,
3. checkpoints and discovery are world-based,
4. the worker slice/stage/flush model is the real execution core,
5. Kafka and SQLite now both sit behind the same journal backend seam.

At this point, “local” vs “hosted” should not be product surfaces. SQLite, Kafka, blobstore,
vault, and HTTP choices are backend/configuration choices over one node runtime.

## Current State In Code

P5 has already collapsed most of the implementation surface.

### What is already true

1. The real runtime/control/backend implementation lives in `aos-node`:
   - worker execution core in
     [worker/](/Users/lukas/dev/aos/crates/aos-node/src/worker)
   - direct acceptance and wait-for-flush in
     [worker/runtime.rs](/Users/lukas/dev/aos/crates/aos-node/src/worker/runtime.rs)
   - world-based replay/open in
     [worker/worlds.rs](/Users/lukas/dev/aos/crates/aos-node/src/worker/worlds.rs)
   - switchable Kafka/SQLite journals in
     [infra/](/Users/lukas/dev/aos/crates/aos-node/src/infra)
   - library-only node startup in
     [node.rs](/Users/lukas/dev/aos/crates/aos-node/src/node.rs)

2. `aos-cli` is now the executable surface for the unified node:
   - `aos node up` starts the current `aos` executable through the hidden `node-serve`
     entrypoint
   - SQLite is the default journal backend
   - Kafka is selected explicitly with `--journal-backend kafka --blob-backend object-store`
   - `aos-cli` no longer depends on an external node binary crate

3. The old local runtime/product path has been removed:
   - `aos-node-local` is no longer a workspace member
   - `LocalRuntime` and `LocalControl` have been deleted
   - `LocalStatePaths` and `FsCas` remain only as local filesystem utilities used by the unified
     runtime and authoring paths

4. `aos-node-hosted` has been removed:
   - it is no longer a workspace member
   - its temporary library reexport and binary wrapper were deleted
   - `aos-node` remains library-only

### What is still wrong

1. Core Rust type names still contain some historical `Hosted*` identifiers.
2. Those names are now internal/mechanical debt, not product or service surfaces.
3. A later mechanical rename can collapse `HostedWorkerRuntime`/`HostedWorker` terminology once it
   is worth the churn.

## Design Stance

### 1) Keep the slice runtime model; do not merge two runtimes

Do not try to unify `LocalRuntime` and `HostedWorkerRuntime` by preserving both.

The right move was:

1. keep the worker/runtime/slice model,
2. re-home it into `aos-node`,
3. delete the old local runtime implementation.

This is much safer than trying to invent a hybrid.

### 2) Backend modes are config, not crates

After P5, there should still be different operating modes:

1. local/dev preset:
   - SQLite journal
   - local state root
   - local filesystem-backed CAS/blob services
   - `.env` secrets
2. server preset:
   - Kafka or SQLite journal
   - object-store/blobstore metadata
   - vault or blob-backed secrets
   - HTTP enabled

But these should be configuration choices over one node library, not different product crates.

### 3) `aos-cli` should be the node binary

The simplest product surface is:

1. `aos` is the primary executable,
2. `aos node ...` starts the unified node,
3. profile/runtime management stays in one binary,
4. there is no second sibling node binary to locate or supervise.

That means `aos-cli` must not shell out to a sibling node binary.

### 4) `aos-node` should remain a library crate

The goal is not “put everything in one binary crate.”

The goal is:

1. `aos-node` becomes the canonical library for the node runtime,
2. `aos-cli` becomes the canonical binary,
3. runtime internals stay reusable and testable as a library.

## Recommended End State

### `aos-node`

`aos-node` should end up owning:

1. runtime core
2. control facade/service composition
3. journal backends
4. checkpoint/inventory/blobstore backends
5. secrets/vault backends
6. optional HTTP serving layer
7. shared APIs and model contracts

I would restructure it roughly like this:

1. `src/node/runtime/`
   current `worker/`
2. `src/node/control/`
   current `control/` plus any retained service composition
3. `src/node/backends/journal/`
   current Kafka and SQLite journal backends
4. `src/node/backends/storage/`
   blob/meta/checkpoint/inventory backends
5. `src/node/backends/secrets/`
   vault and `.env` providers
6. `src/node/http/`
   optional node/control HTTP serving
7. keep `src/model/` and `src/api/` as the stable shared surface

Do not keep an `embedded/` directory as a local runtime center.
Local dev is a configuration preset over the same node runtime, not a second execution model.

### `aos-cli`

`aos-cli` should end up owning:

1. CLI parsing
2. profile management
3. node start/stop/status orchestration
4. local/dev presets and operator UX

It should depend on `aos-node`, not on a separate product crate.

### Crates to delete

After the move stabilizes, delete:

1. `aos-node-local`
2. `aos-node-hosted`

## Proposed Migration Plan

### P5.1 Move hosted runtime/control/backends into `aos-node`

Implemented as the first P5 slice.

Moved from `aos-node-hosted` into `aos-node`:

1. `worker/`
2. `control/`
3. `infra/`
4. `services/`
5. `bootstrap.rs`
6. `config.rs`
7. `env.rs`
8. `test_support.rs`

Current stance:

1. behavior is preserved first
2. module names are mostly preserved
3. `aos-node-hosted` was later removed by P5.5

### P5.2 Introduce unified node presets/config in `aos-node`

Implemented as the second P5 slice.

Define one runtime/bootstrap shape that selects:

1. journal backend
2. blob/meta backend
3. secrets backend
4. HTTP on/off
5. default universe
6. owned worlds

This replaces the product distinction between old local/server node shapes.

I would explicitly model presets such as:

1. `NodePreset::LocalDev`
2. `NodePreset::Server`

or just helper constructors that build a shared config.

Current implementation:

1. `aos-node` exposes the library-only startup seam in
   [node.rs](/Users/lukas/dev/aos/crates/aos-node/src/node.rs).
2. `NodeConfig` selects role, state root, default universe, journal backend, worker config, and
   control HTTP config.
3. `NodeJournalBackend` selects Kafka or SQLite without making `aos-node` a binary crate.
4. `serve_node`, `serve_worker`, `serve_control`, and `serve_all` are reusable library functions.
5. `aos-node` intentionally has no `src/bin` files and no `[[bin]]` targets.

### P5.3 Rebuild `aos-cli` around `aos node`

Replace the sibling-binary orchestration with direct startup of the unified node shape.

That means:

1. stop resolving an external node binary
2. stop writing health/state logic that assumes a separate executable identity
3. expose the public CLI surface as `aos node`

Current implementation status:

1. `aos-cli` no longer depends on `aos-node-hosted`.
2. `aos node up` starts the current `aos` executable with the hidden internal `node-serve`
   command.
3. `node-serve` is backed by `aos-node::node::serve_node`.
4. `aos hosted ...` and `aos local ...` are not preserved as public aliases.
5. The managed node state root is `.aos-node` under the selected `--root`.
6. The public lifecycle surface is `aos node up`, `aos node down`, `aos node status`, and
   `aos node use`.

### P5.4 Delete the old local runtime implementation

Implemented as the fourth P5 slice.

Once the unified node can run the local preset with:

1. SQLite journal
2. local state root
3. local filesystem-backed CAS/blob services
4. `.env` secrets

then remove:

1. `aos-node/src/embedded/runtime.rs`
2. `aos-node/src/embedded/control.rs`
3. `aos-node-local`

This is the key simplification.
Do not keep the old embedded execution engine around “just in case.”

Current implementation status:

1. `aos-node-local` has been removed from the workspace.
2. The old public `aos local ...` command has been removed.
3. `aos node up --journal-backend sqlite` is the SQLite/local-dev node shape.
4. The old `LocalRuntime`/`LocalControl` execution implementation has been removed from
   `aos-node`.
5. `LocalStatePaths` and `FsCas` remain as minimal local filesystem utilities because the unified
   runtime, authoring, and tests still need a local state root and CAS.
6. The old `LocalBlobBackend`, `LocalSqliteBackend`, `LocalStoreError`, and `embedded/sqlite.rs`
   storage path have been removed.
7. `NodeWorldHarness` remains as an in-process test harness, but it is backed by the unified SQLite
   `HostedWorkerRuntime` instead of the deleted local runtime.
8. The old `embedded_integration` test target was removed because it asserted the deleted local
   scheduler/checkpoint behavior rather than the unified runtime behavior.

### P5.6 Remove the old embedded storage namespace

Implemented as the sixth P5 slice.

The old `aos-node/src/embedded` namespace had become misleading after the runtime collapse. It
mixed three different ideas:

1. a deleted local execution engine,
2. local filesystem helpers,
3. an in-process test harness.

Those are now separated:

1. [paths.rs](/Users/lukas/dev/aos/crates/aos-node/src/paths.rs) owns the node state-root layout:
   `LocalStatePaths`
2. [infra/blobstore/fs_cas.rs](/Users/lukas/dev/aos/crates/aos-node/src/infra/blobstore/fs_cas.rs)
   owns the filesystem CAS backend: `FsCas`
3. [harness.rs](/Users/lukas/dev/aos/crates/aos-node/src/harness.rs) owns the in-process
   `NodeWorldHarness`
4. the old local SQLite storage implementation and `LocalStoreError` are deleted
5. local universe selection is gone from this layer; the default is always `UniverseId::nil()`

This intentionally does not remove the SQLite journal backend introduced in P4. That backend is
the unified local/dev journal implementation. What was removed here is the obsolete embedded
SQLite storage/runtime path.

### Error-system direction

The current target is a small layered error model, not one giant enum for every node failure:

1. `PersistError` is the local persistent data error for CAS/filesystem-style operations
2. `BackendError` is the journal/blob/checkpoint backend trait error and wraps `PersistError`,
   `StoreError`, CBOR errors, and backend invariants
3. `WorkerError` and `ControlError` remain boundary errors for runtime/control APIs
4. `NodeHarnessError = anyhow::Error` is acceptable for test/orchestration harnesses

The cleanup rule is: do not add backend-specific public error enums unless the backend needs a
stable external contract. Backend internals should map into `BackendError`; local filesystem
helpers should map into `PersistError`; HTTP/control surfaces should map once at the boundary.

### P5.5 Normalize names and service identity

Implemented as the fifth P5 slice.

After the move:

Completed:

1. health responses now report service `aos-node`
2. Kafka default client identity is now `aos-node`
3. node-managed state defaults to `.aos-node`
4. public CLI lifecycle is now `aos node ...`
5. SQLite is the default `aos node up` journal backend
6. Kafka remains explicit via `--journal-backend kafka --blob-backend object-store`
7. `AOS_NODE_*` replaces the node-owned runtime/config env names introduced by this phase
8. `aos-node-hosted` and `aos-node-local` are gone from the workspace
9. direct product references to `aos-node-hosted`, `aos-node-local`, `.aos-hosted`,
   `aos hosted`, and `aos local` were removed from active crate code and current README snippets

Intentionally deferred:

1. Internal Rust type names such as `HostedWorkerRuntime`, `HostedWorker`, `HostedCas`, and
   `HostedVault` remain for now.
2. These names are now historical implementation names; they no longer define product, service,
   process, or CLI identity.
3. Renaming them is a mechanical churn-only cleanup and should be done separately if we want the
   codebase vocabulary fully normalized.

## What Should Stay Separate

Even after the collapse, some distinctions should remain explicit:

1. Kafka debug/inspection should remain Kafka-specific
2. object-store/blobstore vs local filesystem metadata should remain backend/provider choices
3. vault-backed secrets vs `.env` secrets should remain provider choices
4. HTTP should remain optional; the node runtime should still be usable in-process

Those are backend or deployment seams, not product seams.

## Non-Goals

P5 should not:

1. redesign the slice/stage/flush model
2. redesign checkpoints again
3. reintroduce materialized reads/projections
4. make Kafka ingress first-class again
5. preserve obsolete binary surfaces for `aos-node-hosted` or `aos-node-local`

## Acceptance Criteria

Phase 5 is done when:

1. `aos-node` is the only node library crate
2. `aos-cli` is the only primary node binary surface
3. the real runtime implementation no longer lives in `aos-node-hosted`
4. the old embedded runtime implementation has been deleted
5. SQLite/Kafka/blobstore/vault differences exist only as backend/config differences
6. `aos-cli` no longer shells out to a sibling `aos-node-hosted` binary
7. `aos-node-local` and `aos-node-hosted` are removed from the workspace
