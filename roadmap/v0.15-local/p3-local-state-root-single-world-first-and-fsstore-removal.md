# P3: Local State Root, Single-World-First UX, and `FsStore` Removal

**Priority**: P3  
**Effort**: High  
**Risk if deferred**: High (local runtime semantics remain split across legacy and node-managed paths)  
**Status**: Implemented

## Goal

Make the local product model explicit and uniform:

1. All local execution state lives under a single `.aos` directory.
2. The default local experience is single-world-first.
3. Multi-world local execution remains supported, but as an explicit shared-state-root mode.
4. `aos-smoke`, `aos-agent-eval`, and similar “real runtime” tools use the local node/runtime path rather than raw `FsStore`.
5. Remove `FsStore` entirely.
6. Refactor aggressively and ignore backward compatibility.

This work is not a polish pass. It is a model correction.

## Why This Refactor Is Needed

The current local stack still carries two competing ideas of “run locally”:

1. node-managed local worlds backed by SQLite plus filesystem CAS
2. direct single-world runtime paths backed by `FsStore`

That split is no longer useful.

It causes concrete problems:

- smoke and eval tooling can exercise a less complete runtime path than the real local node
- authoring/build flows still write into legacy `FsStore` layouts
- local storage layout is partly standardized under `.aos`, but path ownership is still scattered
- the operator mental model is muddy: sometimes the world directory is the runtime boundary, sometimes the local node DB path is

We should stop treating the old direct `FsStore` world root as a first-class runtime architecture.

## Design Stance

### 1) Local execution is defined by a state root

The canonical local runtime boundary is a state root directory.

By convention, that directory is:

- single-world default: `<world-root>/.aos`
- shared multi-world mode: operator-chosen path such as `/some/path/.aos`

Everything mutable for local execution must live under that root.

This includes:

- local SQLite database
- local filesystem CAS
- module build cache
- wasmtime cache
- runtime scratch, pid, and logs if needed

No mutable local execution state should be written outside the chosen `.aos` state root unless explicitly declared as external tooling behavior.

### 2) Single-world-first is the default product model

The common case is:

1. user has one world directory
2. user runs local commands from that directory or points at it
3. runtime state lands in that world’s `.aos`

This should be the default assumption for local UX.

Multi-world-in-one-process remains valid, but it is not the primary user mental model. It is an advanced deployment mode of the same local engine.

### 3) Multi-world local remains real, but is explicit

The current local node architecture of one process running multiple worlds in one singleton universe is still good.

What changes is presentation and path ownership:

- the operator explicitly chooses a shared `.aos` state root
- multiple worlds are then seeded into that state root
- the same engine, DB, CAS, and caches are reused

This preserves the bridge-ready local node architecture without forcing every local workflow to feel like a universe admin exercise.

### 4) `.aos` is the containment boundary for local execution artifacts

Required layout shape:

```text
.aos/
  local-node.sqlite3
  cas/
  cache/
    modules/
    wasmtime/
  run/
  logs/
```

Exact subdirectory names may evolve, but the principle must not:

- one `.aos`
- one place to inspect, clean, copy, archive, or delete local execution state

### 5) `FsStore` must be removed

`FsStore` belongs to the older direct-runtime model.

Its continued existence encourages the wrong thing:

- bypassing node-managed local persistence
- teaching smoke/eval tools to exercise a narrower runtime path
- keeping authoring and runtime storage models artificially split

The end state should be:

- lightweight tests may still use in-memory storage
- real local execution uses local-node persistence plus `HostedStore`
- immutable local CAS on disk is handled by the local node persistence layer, not by `FsStore`

This means:

- remove `FsStore`
- remove direct runtime entrypoints whose main job is “open a world root with `FsStore`”
- migrate code that only wanted filesystem CAS semantics onto the local state-root CAS abstraction

### 6) Backward compatibility is explicitly out of scope

Do not preserve compatibility with:

- old `.aos/store` layouts
- old direct `FsStore`-backed local runtime behavior
- old path conventions if they conflict with the new state-root model
- old smoke/eval harness assumptions

Migration convenience is optional. Architectural cleanup is required.

## Storage Model

### Local mutable state

Local mutable runtime and admin state remains SQLite-backed.

This includes:

- world catalog metadata
- journal/inbox state
- snapshots and active baselines
- command records
- runtime reopen metadata
- local secrets if enabled

### Local immutable CAS

Local CAS remains filesystem-backed.

This is the correct local stance.

We do **not** want to move CAS bytes into SQLite.

Reasons:

- large immutable blobs fit filesystem CAS better
- content-addressed files are easy to inspect and manage
- this matches the current local-node direction already
- it keeps local persistence simpler and closer to hosted CAS concepts

What changes is ownership:

- local filesystem CAS belongs under the local state root
- it is opened through local-node persistence, not through `FsStore`

## Runtime Architecture

### Canonical local execution path

The canonical local execution path should be:

1. resolve local state root
2. open local persistence (`SqliteNodeStore` plus filesystem CAS under that root)
3. adapt it through `HostedStore`
4. run worlds through the same node-managed hot-world path used by local/hosted persistence-backed execution

This is the path that should define “real local execution”.

### What remains acceptable for tests

It remains acceptable to use:

- `MemStore`
- in-memory world persistence
- in-memory journals

But only for truly lightweight tests, unit tests, and focused determinism harnesses.

Anything intended to represent a realistic local runtime should use persisted local-node state.

## Tooling and Product Consequences

### `aos-smoke`

`aos-smoke` should stop opening authored worlds directly via `FsStore`.

Instead it should:

1. create or resolve a local state root
2. seed the authored world into local persistence
3. run via the local-node/hosted-store path

This ensures smoke tests cover:

- real world bootstrap
- persisted journal/inbox/snapshot behavior
- reopen/replay paths
- local-node storage semantics

Dependency stance:

- engine-only smoke/tests may use `aos-runtime`
- persisted local smoke should use `aos-node + aos-sqlite`
- `aos-smoke` should not depend on `aos-node-local` in the end state unless it is explicitly testing the local service product surface

### `aos-agent-eval`

`aos-agent-eval` should follow the same rule.

It is specifically the wrong place to use a narrower runtime model because its job is to exercise behavior closer to production-like execution.

Dependency stance:

- `aos-agent-eval` should use `aos-node + aos-sqlite` for realistic local execution
- it should not depend on `aos-node-local` in the end state
- `aos-runtime` alone is not sufficient for the persisted local semantics we want eval to cover

### Authoring flows

Authoring should no longer treat `FsStore` as the normal persistent backing store.

Required direction:

- compile/build outputs that belong to local execution go into the chosen `.aos` state root
- world bootstrap/import should write into local-node CAS and metadata through the local persistence path
- authoring helpers should target “local state root + local node persistence”, not “legacy direct world store”
- authoring should own the shared persisted-local bootstrap helpers so smoke/eval-style tools do not reimplement local world setup themselves

This does **not** mean authoring must always talk to a long-lived daemon.

It means authoring should use the same storage model, even in direct batch or helper flows.

That ownership split should be:

- `aos-authoring`: resolve local state roots, prepare/reset local `.aos`, and bootstrap persisted local worlds from authored manifests
- `aos-smoke` / `aos-agent-eval`: compile fixture-specific modules, patch manifests, drive effects, and assert behavior

## Required Refactors

### 1) Introduce a shared local state-root path resolver

Add a single resolver type that owns local path derivation.

It should answer paths for:

- SQLite DB
- CAS root
- module cache
- wasmtime cache
- runtime/log/scratch directories

No subsystem should independently invent `.aos/...` paths.

### 2) Make state-root resolution the primary local input

Prefer passing:

- world root for single-world default mode
- explicit state root for shared multi-world mode

Do not make raw SQLite DB path the primary product-level abstraction.

The DB path becomes an internal detail derived from the state root.

### 3) Cut `FsStore` out of runtime-facing code

Remove `FsStore` usages from:

- smoke harnesses
- eval harnesses
- real local runtime entrypoints
- any authoring path that is actually preparing local execution state

### 4) Move filesystem CAS concerns behind the local-node layer

Any code that needs local persistent blobs should go through:

- local node persistence traits
- local state-root CAS helpers owned by local persistence

Not through a generic `FsStore`.

### 5) Extract reusable local persistence into `aos-sqlite`

The reusable local state implementation should not remain buried inside `aos-node-local`.

Create a dedicated crate named `aos-sqlite` to own:

- `LocalStatePaths` or equivalent state-root resolver
- SQLite-backed local mutable persistence
- local filesystem CAS rooted under `.aos/cas`
- local storage/bootstrap helpers needed by persisted local execution
- storage-coupled local secret persistence/resolution helpers if they remain local-specific

`aos-node-local` should then become a thinner composition crate that owns:

- HTTP server wiring
- control facade
- supervisor lifecycle
- batch/dev commands
- daemon/service concerns

Reason:

- `aos-smoke`, `aos-agent-eval`, and similar tooling should depend on the reusable local backend, not on the full local service crate with HTTP and CLI concerns
- local persistence is a real reusable seam
- `aos-node-local` is primarily a product/service composition layer

Short-term expedient:

- it is acceptable to let smoke/eval depend on `aos-node-local` briefly during migration if that speeds up removal of `FsStore`

End-state requirement:

- reusable local persisted execution depends on `aos-sqlite`, not directly on `aos-node-local`

### 6) Simplify direct local execution around the same storage model

If a direct batch path remains, it should still use:

- local state root
- local SQLite persistence
- local filesystem CAS

The difference from daemonized operation should be process lifetime and control surface, not storage model.

## Crate Direction

High-level direction:

1. keep the node-managed hot-world path as the canonical persisted execution seam
2. keep lightweight in-memory seams for tests
3. extract reusable local persistence/state code into `aos-sqlite`
4. keep `aos-node-local` as the local service/binary composition layer
5. remove `FsStore`
6. push local persistent filesystem CAS ownership into local persistence rather than a standalone store crate
7. update authoring to target the new local state-root model

This refactor has now also folded the old `aos-store` crate into `aos-kernel`.

Current expectation:

- `aos-sqlite` becomes the reusable local persisted-state crate used by local tooling
- `MemStore` may still survive for lightweight tests
- manifest/catalog loading should be reevaluated separately from filesystem-backed CAS
- kernel-owned store primitives should not imply “production local persistence backend”

Explicit dependency model:

- `aos-runtime`: engine-only and lightweight test seam
- `aos-node`: shared persisted-world execution seam built around `WorldStore`
- `aos-sqlite`: local persisted backend implementing the local `WorldStore` side
- `aos-node-local`: local service/daemon/batch composition layer on top of `aos-node + aos-sqlite`

Tooling rule:

- use `aos-runtime` when you intentionally want engine-only execution
- use `aos-node + aos-sqlite` when you want realistic persisted local execution
- use `aos-node-local` only when you are testing or implementing the local service product surface itself

## Non-Goals

This proposal does not require:

- moving local CAS bytes into SQLite
- removing multi-world support from local node
- preserving old runtime path behavior
- preserving old on-disk layouts

## Acceptance Signals

This refactor is successful when:

1. Running a local world always means choosing a `.aos` state root.
2. The default single-world local flow stores all mutable and cached execution artifacts under `<world-root>/.aos`.
3. Shared multi-world local flow stores all execution artifacts under one operator-chosen `.aos`.
4. `aos-smoke` and `aos-agent-eval` run through persisted local-node runtime paths by default.
5. `FsStore` is gone.
6. No product-critical local execution path depends on legacy direct world-root storage semantics.
7. The codebase has one local storage model, not two competing ones.

Current state:

- `LocalStatePaths` is the shared local path resolver.
- reusable local persistence lives in `aos-sqlite`.
- `aos-node-local` is the service/composition layer on top of `aos-sqlite`.
- `aos-smoke` and `aos-agent-eval` use shared persisted-local bootstrap through authoring helpers.
- `FsStore` has been removed.
- legacy `.aos/store` style state is purged on local open/reset rather than preserved.

## Recommended Execution Order

1. Introduce `LocalStatePaths` or equivalent shared resolver.
2. Extract reusable local persistence and CAS into `aos-sqlite`.
3. Convert local-node code to consume `aos-sqlite` everywhere.
4. Convert smoke/eval harnesses to persisted local execution via `aos-sqlite`.
5. Convert authoring/bootstrap helpers to the same local state-root model and make them the shared bootstrap path for smoke/eval.
6. Delete `FsStore` and all direct runtime paths that depend on it.
7. Remove compatibility shims instead of preserving them.
