# P2: Local Persistence Backend and CLI Cutover

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (P1 will stop at API rhetoric without a usable local node)  
**Status**: Substantially Complete

## Dependency On Completed P1 Work

The following prerequisites are now in place:

1. `aos-node` owns the shared node protocol and control surface.
2. `aos-runtime` is the dedicated single-world runtime crate.
3. Hosted-specific persisted-world adapters no longer live inside the runtime crate.
4. Local-capable versus hosted-only node traits are split cleanly enough to implement a minimal SQLite local node.

P2 is largely landed; the remaining work is follow-on hardening and product-surface depth rather than foundational architecture.

## P2 Checklist

Completed:

- [x] Add `aos-node-local` as a new crate and binary.
- [x] Replace the early local backend with a native SQLite backend for mutable node/runtime state.
- [x] Keep local CAS filesystem-backed rather than storing it inside SQLite.
- [x] Make the local backend explicitly singleton-universe in storage and behavior.
- [x] Add a single-process local supervisor that keeps worlds hot in one process.
- [x] Serve the shared `aos-node` `/v1` HTTP surface through a concrete local `NodeControl` implementation.
- [x] Tighten the shared node seams so local command execution does not require hosted coordination.
- [x] Add the first local lifecycle surface to the canonical `aos` CLI: `aos local up|status|down|use`.
- [x] Extract shared authored-world build/upload helpers into `aos-authoring`.
- [x] Remove the canonical `/v1` CLI surface's dependency on the legacy `aos-cli` daemon/control code.
- [x] Reuse the shared authoring/build crate from hosted probes instead of importing from the old CLI.
- [x] Promote the `/v1` CLI into the canonical `aos` package/binary slot.
- [x] Remove the old `aos-cli` crate and its control-socket daemon surface from the workspace.
- [x] Add the first persisted-world batch/dev surface under `aos-node-local batch ...`.

Recently completed local-product follow-on work:

- [x] Extend the first local lifecycle surface into fuller ergonomics around authored-world bootstrap and default local targeting flows.
- [x] Extract reusable local persistence, local CAS, and state-root path resolution into `aos-sqlite`.
- [x] Make `.aos` the canonical local state root for local execution artifacts.
- [x] Remove `FsStore` and cut realistic local execution over to `aos-node + aos-sqlite`.
- [x] Improve local query/control parity for ops-style worker inspection by exposing the same worker and worker-world read surface in local single-worker form.

Remaining:

- [ ] Deepen the first local batch/dev surface for persisted local worlds under `aos-node-local` beyond the initial `batch worlds|status|step|manifest|trace-summary|send|command` commands.

Deferred for now:

- [ ] Harden the native SQLite schema and migration story beyond destructive schema resets.

## Goal

Implement the first real local node backend and cut the CLI over to it for daemonized local workflows.

Primary outcomes:

1. Local node gets a transactional persistence backend that satisfies the shared node/runtime contract.
2. `.aos` becomes the canonical local state root for local execution, with single-world defaulting to `<world-root>/.aos` and shared multi-world mode using an explicit operator-chosen state root.
3. The CLI uses HTTP for daemonized local world administration.
4. Local node stores universe/world/runtime metadata in a bridge-ready way.
5. The promoted node-facing CLI surface is standalone before the old local daemon CLI is retired.

Related design note:

- see `crate-layout.md` in this folder for the target crate and binary layout

## Backend Choice

### Recommendation: SQLite first

Use SQLite as the first local node persistence backend.

Reasons:

1. The local node now needs transactional semantics across:
   - journal head
   - snapshots and active baseline
   - command records
   - world catalog/runtime metadata
   - secrets
2. Plain filesystem storage is too weak and awkward for this shape.
3. SQLite is operationally simpler than LMDB for first implementation.
4. SQLite is portable, easy to inspect, and easy to bundle in local tooling.

### Why not keep raw FS as the main local runtime backend

The old per-world `.aos/store` layout was acceptable as a temporary bootstrap model for:

- simple single-world experiments
- early direct-runtime harnesses

It is not the target local model.

The canonical local model is a state root under `.aos/` containing:

- `local-node.sqlite3` for mutable runtime/admin state
- `cas/` for immutable content-addressed bytes
- `cache/` for module and Wasmtime caches

The old `.aos/store` layout should be treated as legacy state to remove, not a backend to preserve.

It is not a good fit for:

- multi-world node metadata
- atomic world/runtime updates
- shared universe-scoped CAS operations
- node command records

### Why not LMDB first

LMDB remains a viable later option, but it is not the best first move.

Reasons:

- SQLite has broader tooling and lower implementation friction for mixed relational/queue/state metadata.
- The local node is not chasing extreme write throughput first.
- The first problem to solve is semantic unification, not backend micro-optimization.

## Persistence Shape

The local backend should satisfy the same runtime/admin contract as hosted mode.

Crate placement stance for P2:

- do not add a new top-level persistence-contract crate for this work
- put the shared mutable runtime/admin persistence traits in the shared node/runtime layer
- keep the reusable local SQLite implementation in `aos-sqlite`
- keep the FoundationDB implementation inside the hosted node composition crate

This means local persistence must cover:

1. universe metadata
2. world metadata and handles
3. journal storage
4. snapshots and active baseline
5. lightweight command records
6. hot-world runtime/catalog metadata
7. universe-scoped CAS
8. secret bindings and versions if local secret storage is enabled

Not required for minimal local v0.15:

- distributed leases
- worker heartbeats
- worker-world ownership indexes
- durable effect/timer claim queues
- durable projections as a persistence requirement

### CAS recommendation

Use a hybrid approach:

- mutable runtime/admin metadata in SQLite
- immutable CAS blobs on the filesystem under the local node runtime home

Current local stance:

- keep CAS fully filesystem-backed for simplicity
- let SQLite store references to immutable CAS objects where needed

Correctness rule:

- CAS remains content-addressed
- local CAS is node-scoped because the local node itself is single-universe
- storage location is an implementation detail

## Local Runtime Home

Local runtime state is owned by a state root.

### Required distinction

Two different things still exist:

1. authored world root
   - AIR
   - workflow source
   - sync config
2. local state root
   - default single-world mode: `<world-root>/.aos`
   - shared multi-world mode: operator-chosen path
   - local node persistence DB
   - local node CAS
   - module and runtime caches
   - worker/runtime metadata

### Design stance

Do not treat arbitrary world-root storage layout as the runtime model.

Instead:

- local execution always resolves a state root
- in the common case that state root is the world’s `.aos`
- in shared mode multiple worlds are seeded into one explicit state root

### `.aos` stance

Keep `.aos` as the local state-root boundary for local execution artifacts.

That means:

- single-world local execution writes under `<world-root>/.aos`
- shared multi-world local execution writes under one explicit `.aos` root
- legacy `.aos/store` layouts are old state to purge, not a model to preserve

## CLI Cutover

### End-state CLI modes

The CLI should have two fundamentally different execution paths:

1. direct batch/local world path
2. node HTTP client path

### Direct batch/local world path

Used for:

- `aos-node-local` batch/step mode
- local experimentation
- tests
- fixtures
- direct single-world debugging

Ownership stance:

- pure engine-level stepping lives in `aos-runtime`
- batch/dev operations over real local persisted world state should live in `aos-node-local`

Current status:

- `aos-node-local` now has a first persisted-world batch/dev surface
- the initial commands are coarse but real: `batch worlds`, `status`, `step`, `manifest`, `trace-summary`, `send`, and `command`

### Node HTTP client path

Used for:

- daemonized local administration
- hosted administration
- any command that is really operating against a long-lived node

This path talks to:

- local node over HTTP
- hosted node over HTTP

### Required CLI behavior changes

1. Remove dependence on the local Unix socket control protocol.
   Status: completed at the canonical CLI/runtime boundary.
2. Make local daemonized commands go through HTTP.
   Status: completed for the first local lifecycle surface.
3. Add convenient local targeting without changing the wire model.
   Status: completed in first usable form; `aos local up|status|down|use`, explicit local profile kind, smoother local default targeting, and local-root inference for `aos world create` now exist.
4. Preserve automatic world-directory discovery for authoring commands.
   Status: still in place.

Examples of the intended split:

- `aos run --batch -w ./my-world`
  - direct local world execution
- `aos local up`
  - starts local node
- `aos local status`
  - inspects the managed local runtime and checks `/v1/health`
- `aos local down`
  - stops the managed local node process
- `aos local use`
  - selects the reserved local CLI profile
- `aos world create --local-root ./my-world --target local`
  - uploads bundle into local node universe
- `aos world state get ... --target local`
  - uses shared HTTP client against local node

## World Authoring to Local Node Flow

The local node should not require users to abandon authored world directories.

Required authoring flow:

1. Author AIR and workflow locally in a directory.
2. Build bundle or manifest as usual.
3. Upload/bootstrap into local node.
4. Select or reference resulting local world handle/id.
5. Administer that world through the shared node API.

This preserves “simple local development of individual worlds” while removing the divergent daemon model.

## CLI Consolidation Stance

The right convergence direction is:

- promote the newer resource-oriented `/v1` CLI surface
- keep authored-world build/upload helpers reusable from a shared support crate
- retire the old control-socket and `/api` CLI path instead of trying to evolve it further

Current reality:

- the promoted `/v1` surface now owns the canonical CLI package/binary identity
- the old local/batch daemon UX can now be removed without leaving the new CLI coupled to it
- that cutover is complete at the crate/package level

## Required Runtime Semantics Locally

The local backend must support the same observable runtime semantics expected by the shared worker/supervisor.

### 1) Durable command model

Local node should use:

- command submission
- command status polling
- the same command resources as hosted

Important clarification:

- local does not need a hosted-style durable command queue
- it may execute commands inline or through a simple in-process dispatcher
- it should still persist lightweight command records so the HTTP model remains shared

### 2) Shared query semantics without mandatory durable projections

Local node should expose the same read surfaces used by the shared API:

- head projection
- cell projections
- workspace projections

Important clarification:

- local does not need to persist those projections as derived tables in v0.15
- local may answer these reads directly from hot world state
- durable projections remain a hosted capability and a possible later local optimization

### 3) No hosted-style lease model in local

Local v0.15 should not replicate hosted lease and worker coordination semantics.

Reasons:

- local is single-process and effectively single-worker
- all local worlds stay hot under one runtime
- simulating hosted distributed coordination would add complexity without adding useful semantics

What local should persist instead:

- world runtime/catalog state needed to reopen and continue hot worlds after restart
- command status
- replay roots and CAS references

## Implementation Plan

### Phase 1: local persistence crate

Status: Completed

1. Add a reusable local persistence implementation.
2. Implement the base node trait set needed by local single-process supervision.
3. Reuse the in-memory implementation as behavioral reference where useful.

### Phase 2: local node binary

Status: Completed in first usable form

1. Add `aos-node-local`.
2. Wire shared node runtime and shared HTTP facade to local persistence.
3. Bootstrap a default universe on first startup.

### Phase 3: local bundle/bootstrap flow

Status: Completed in first usable form

1. Add CLI support to upload local authored worlds into the local node.
2. Create local worlds from uploaded manifest/baseline roots using the same create semantics as hosted.
3. Preserve world handles and explicit IDs.

Note:

- the canonical CLI has the authoring/upload machinery
- the first local lifecycle surface now exists
- when the active profile is local, `aos world create` now defaults to the current authored world directory when it finds an `air/` tree, instead of always requiring `--local-root`
- local-target convenience flows now exist in first usable form, though further polish is still possible

### Phase 4: CLI cutover

Status: Mostly completed

1. Switch daemonized local commands to the shared HTTP client path.
2. Remove local control socket dependency.
3. Keep direct batch path available somewhere in the product, but not necessarily inside the thin `aos` node client.

Note:

- the shared HTTP client path is now canonical
- explicit local lifecycle commands now exist and local-target ergonomics now work in first usable form
- direct batch execution for real local persisted worlds should live under `aos-node-local`

### Phase 5: cleanup

Status: Substantially completed

1. Remove dead local daemon/control code.
2. Remove old local-only HTTP shim.
3. Remove local protocol drift from CLI assumptions.

Note:

- the old daemon/control crate is gone
- the remaining cleanup is mostly batch-surface depth rather than deep architectural refactor

## Bridge Readiness Requirements

This persistence and CLI work must avoid boxing the project out of the later bridge.

Required properties:

1. Local node stores explicit universe/world identities.
2. Local node CAS is universe-scoped and protocol-driven.
3. World create/fork/bootstrap persist enough information to export and reattach worlds later.
4. CLI does not special-case filesystem paths as if they were the universe protocol.
5. Local shares command and read API semantics with hosted even when the implementation strategy differs.

Future bridge implication:

- attaching a local world to a hosted universe should become a node-to-node operation, not a filesystem synchronization trick

## Acceptance Signals

This milestone is complete when:

1. A local node can run with a SQLite-backed persistence implementation.
2. That node can host multiple worlds in one universe.
3. The CLI administers daemonized local worlds over HTTP, not a Unix socket protocol.
4. Direct single-world batch execution remains available and useful.
5. Authored world directories still provide a simple local development experience.
6. Local shares command semantics with hosted without copying hosted lease/worker coordination.
7. The local node stores enough structured runtime state that future node-to-node bridge work does not need to reinvent local semantics.
