# v0.15 Local: Crate Layout and Binary Roles

This note captures the intended crate layout for the local/node refactor so that P1 and P2 have a concrete target.

The main design goal is to reduce graph complexity while keeping the real architectural seams explicit.

## Design Principles

1. Keep one user-facing CLI: `aos`.
2. Separate single-world execution from multi-world node supervision.
3. Keep local and hosted as two compositions of the same node runtime, not two separate products.
4. Do not introduce another top-level persistence-contract crate for v0.15.
5. Keep FoundationDB as an implementation detail of the hosted node composition, not part of the primary user mental model.
6. Keep authoring/build/upload helpers out of the legacy daemon CLI so the node-facing CLI can stand on its own.

## Recommended Target Shape

### `aos`

Owns:

- the primary user-facing CLI
- local authoring flows
- local and hosted node administration over HTTP
- upload/bootstrap/push flows from authored worlds into nodes

Must not own:

- long-lived daemon runtime implementation
- hosted-only deployment wiring
- local-only daemon protocol

Design stance:

- there should be one primary CLI, not separate local and hosted CLIs
- `aos` may start or manage a local node for convenience, but the long-lived node runtime is still a separate service binary
- the resource-oriented `/v1` CLI surface is now the right foundation for this crate
- the remaining work is convergence and rename/cutover, not preserving the old control-socket CLI architecture
- direct batch/step execution should return later outside the thin node client; for real local persisted-world semantics it should live under `aos-node-local`

Current transition status:

- [x] the promoted `/v1` node CLI now owns the canonical `aos-cli` package and `aos` binary slot
- [x] the path-level folder rename is complete
- [x] the first local lifecycle surface now exists in `aos` via `aos local up|status|down|use`
- [x] the important dependency inversion is done: the promoted CLI no longer depends on the old daemon CLI for authoring/build helpers
- [ ] the remaining cleanup is follow-on command-surface and local-product work

### `aos-authoring`

Owns:

- shared local authoring/build/upload helpers
- sync-map loading and AIR import resolution
- workflow/module compilation helpers for authored worlds
- manifest bundle and patch-document construction

Design stance:

- this is a support crate, not a user-facing product
- it exists to let the node-facing CLI and probes reuse authoring logic without depending on the legacy local daemon CLI
- this crate is a temporary but useful consolidation point; if the final `aos` CLI absorbs it cleanly later, that is acceptable

### `aos-runtime`

The runtime crate is now `crates/aos-runtime`.

Owns:

- single-world execution engine
- replay/restore/open helpers for one world
- persistence-backed single-world store adapter
- direct batch-mode helpers
- local/test/smoke-friendly single-world runtime APIs

Later follow-on:

- pure engine-level stepping utilities may still live here, but local persisted-world batch/dev commands should not

Must not own:

- worker leases
- multi-world supervision
- node HTTP/control surfaces
- universe/world catalog administration

Dependency stance:

- `aos-node` depends on this crate for single-world execution
- pure engine-level batch helpers depend on this crate directly

### `aos-node`

Owns:

- shared multi-world node runtime
- shared hot-world runner logic
- single-process local supervision logic
- shared command and query service model
- shared HTTP resource/control facade
- mutable runtime/admin persistence traits needed by the node runtime
- hosted-only distributed coordination extension traits

Must not own:

- FoundationDB-specific keyspace or transaction code
- SQLite-specific schema code
- direct authored-world filesystem UX

Design stance:

- the node crate is the shared outer runtime for daemonized execution
- local node and hosted node are both compositions of this crate
- mutable runtime/admin persistence contracts live here rather than in a separate top-level “persist” crate
- the base node traits must be implementable by the local single-process node without leases or worker coordination
- hosted-only distributed coordination traits still belong in `aos-node`, but under an explicit hosted extension layer rather than in the base local-capable trait set

### `aos-node-hosted`

Owns:

- hosted node binary and wiring
- FoundationDB-backed persistence implementation
- hosted distributed supervisor implementation
- hosted deployment config
- hosted bootstrap/secret/runtime wiring

Design stance:

- this is a service binary/package, not a second user-facing CLI
- “hosted” is the public role; FoundationDB remains an implementation detail of this composition
- this crate should implement hosted extension traits defined by `aos-node`; it should not own the trait definitions themselves unless they become FoundationDB-specific

### `aos-node-local`

Owns:

- local node binary and wiring
- local runtime home/bootstrap
- default local universe bring-up
- single-process hot-world runtime management
- later batch/dev entrypoints over real local persisted world state

Depends on:

- `aos-sqlite` for reusable local persistence, local CAS, and local state-root path resolution

Current status:

- [x] initial crate and binary now exist
- [x] the local backend now uses native SQLite for mutable node/runtime state
- [x] local CAS is filesystem-backed under the local runtime home
- [x] the first local supervisor is single-process and single-worker in semantics
- [x] local currently answers key shared API reads from hot world state rather than requiring durable projections
- [x] the shared hot-world core now lives in `aos-node`, and both local and hosted open/ingress handling depend on it
- [x] local storage/runtime semantics are explicitly singleton-universe even though the shared API still uses universe-shaped resources
- [x] the crate now has a first persisted-world batch/dev surface via `aos-node-local batch ...`

Design stance:

- this is the daemonized local node runtime
- the primary user entrypoint remains `aos local up` or equivalent CLI flows, not direct daemon UX
- all local worlds stay hot in one process in the initial design
- local does not implement distributed worker leases or multi-worker coordination just to mirror hosted
- local should not carry hosted-style universe catalogs internally; universe identity is a singleton runtime boundary, not a full local admin plane
- the current local product surface now includes managed lifecycle commands in `aos`; remaining work is richer local authoring/bootstrap ergonomics on top
- the current local product surface now also includes first usable local-target ergonomics in `aos`
- the first direct batch/dev commands for local persisted worlds now live here rather than in `aos` or a new crate

### `aos-sqlite`

Owns:

- reusable local SQLite-backed mutable persistence
- local filesystem CAS helpers
- local state-root path resolution
- local secret persistence/resolution that is storage-coupled

Design stance:

- this is not a second local product surface
- this exists so local persisted execution can be reused without depending on the `aos-node-local` service crate
- `aos-node-local` composes it for daemon/service behavior, while smoke/eval/authoring can depend on it directly where appropriate

## Store Stance

Kernel-level store primitives and node persistence are still different abstractions.

The old `aos-store` crate has now been folded into `aos-kernel`.

The remaining immutable content-addressed storage surface is the kernel store API used broadly across runtime/CLI/adapters.

Node persistence is mutable runtime/admin state, including:

- journal head
- inbox cursor
- command records
- runtime/catalog state
- secrets
- universe/world metadata

Hosted node persistence additionally includes:

- leases
- worker heartbeat / worker-world coordination
- durable effect/timer queues
- durable projections

Because those concerns are different, v0.15 should not try to collapse them into one abstraction.

What changed is crate placement, not the conceptual split:

- immutable CAS/store traits now live in `aos-kernel`
- local persisted runtime/admin state lives in `aos-sqlite`
- we still do not want a separate top-level `aos-persist` crate

## Why Not Remove the Single-World Runtime Crate

The single-world runtime seam is real because it is reused by:

- direct batch runs
- tests and smoke fixtures
- local direct execution/debugging
- node-managed hosted/local execution

Removing that seam would push too much into the node crate and make batch/local-direct execution second-class again.

The right move is to keep the seam and make it clearer, not erase it.

## Binary Model

The intended binaries are:

- `aos`: user-facing CLI
- `aos-node-local`: local daemon/service binary
- `aos-node-hosted`: hosted daemon/service binary

This is not “multiple CLIs”.

It is one CLI plus two service binaries.

## Dependency Direction

Recommended high-level dependency direction:

1. `aos-runtime` depends on kernel/store/effect/runtime primitives.
2. `aos-node` depends on `aos-runtime`.
3. `aos-node-local` depends on `aos-node` and includes SQLite persistence plus single-process supervision implementation.
4. `aos-node-hosted` depends on `aos-node` and includes FoundationDB persistence plus hosted distributed coordination implementation.
5. `aos` talks to local and hosted nodes over the same HTTP model.
6. A later local batch/dev surface can live in `aos-node-local` without fattening the main `aos` binary.

## Transitional Mapping from Current Crates

Near-term mapping from the current workspace:

- `crates/aos-runtime` -> keep as the single-world runtime seam
- current `aos-node-hosted` world-running logic -> move shared parts into `aos-node`
- current `aos-node-hosted` lease/worker/maintenance coordination -> remains hosted extension logic and becomes part of `aos-node-hosted`
- current `aos-node-hosted` remaining hosted wiring -> becomes `aos-node-hosted`
- current local daemon path -> converges toward `aos-node-local`
- reusable local SQLite persistence/CAS/state-root layer -> lives in `aos-sqlite`
- shared authored-world build/upload helpers -> live in `aos-authoring`
- promoted `/v1` command/resource model -> is now the foundation for the user-facing `aos` CLI
- current `aos-fdb` protocol traits -> move toward backend-neutral node/runtime naming, but do not force a separate top-level contract crate in v0.15

## Trait Layering

Recommended trait layering inside `aos-node`:

- base local-capable traits:
  - `WorldStore`
  - `NodeCatalog`
  - `CommandStore` or `CommandService`
  - `WorldAdminStore`
  - `UniverseStore`
  - optional `SecretStore`
- hosted-only extension traits:
  - distributed coordination
  - effect queue
  - timer queue
  - optional durable projection store

Important stance:

- hosted extension traits should live in `aos-node`, not `aos-node-hosted`
- the reason is that they are part of the hosted node protocol surface, not FoundationDB internals
- `aos-node-hosted` should provide implementations, not define the contracts

## Non-Goals for This Refactor

1. Perfect final naming across all crates before the architecture is working.
2. Splitting every seam into its own crate.
3. Introducing a separate user-facing hosted CLI.
4. Making local mode synonymous with SQLite forever.
