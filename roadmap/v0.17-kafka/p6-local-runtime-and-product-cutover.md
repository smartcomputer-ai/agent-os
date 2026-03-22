# P6: Local Runtime and Product Cutover

**Priority**: P6  
**Effort**: High  
**Risk if deferred**: High (local and hosted will diverge again exactly when the hosted model is being rewritten)  
**Status**: Complete for v0.17 (local log-first runtime, seed/fork, and workspace product
surface are all wired through the shared embedded/control seam)

## Goal

Keep the local product surface strong while converging local runtime semantics onto the new
log-first hosted direction.

The local product should remain easy to use.

The local architecture should not remain permanently different.

## Completed In Code

Implemented on the experimental branch:

1. Phase 2 has started: the new shared log/checkpoint contracts exist in `aos-node`.
2. An embedded backend proves those contracts via `MemoryLogRuntime`.
3. `aos-node-hosted` has been replaced with a route-first runtime/control surface on top of the
   new seam rather than the old FDB-shaped worker model.
4. Shared semantic cases already exist for route-epoch rejection, authoritative frame replay, and
   checkpoint recovery.
5. Phase 4 has started on the hosted side: `aos-node-hosted` now has real Kafka-backed runtime
   planes and a real durable blobstore-backed checkpoint/blob plane.
6. Hosted restart recovery now rebuilds from blobstore checkpoints plus Kafka route/journal replay.
7. Phase 3 local cutover now exists for the core local path: `aos-node-local` owns an embedded
   `LocalLogRuntime` backed by local SQLite plus filesystem CAS rather than the old `aos-sqlite`
   node-store seam.
8. Local world creation, command submission, domain ingress, and receipt ingress now enter through
   the log-first submission path and normalize into authoritative `WorldLogFrame`s.
9. Local bootstrap/create-world now emits the initial canonical snapshot history immediately rather
   than bootstrapping opaque local state off to the side.
10. Local control reads and batch flows now run against the new local log runtime for the core
    world/runtime/manifest/state/journal/trace surfaces plus workspace resolution.
11. Local end-to-end and batch coverage now exercise the new embedded log-first runtime path.
12. `CreateWorldSource::Seed` now works on the shared log-first create path and the local
    embedded/runtime control surface.
13. Local world forking now works on the same runtime/control seam by selecting a source
    baseline snapshot, rewriting it under the configured fork policy, and reopening the forked
    world from that baseline.
14. Local workspace root/tree/read/mutate/diff APIs are now wired through the shared embedded
    runtime/control surface rather than stopping at `workspace resolve`.

## Primary Stance

Local should be:

- the same logical model as hosted
- embedded in one process on one machine by default

This means:

- same normalization rules from ingress/admission objects to canonical world history
- same canonical world-record vocabulary
- same routing concepts
- same snapshot/checkpoint concepts
- same effect/fabric semantics

but not necessarily:

- literal Kafka
- literal S3

for the default local experience.

## What We Keep

The current product stance remains correct:

- `aos` remains the main CLI
- `aos-node-local` remains the local service/binary surface
- direct local batch/dev workflows remain first-class

The reset is architectural, not a rejection of the local UX work.

## What Changes

The internals of local should converge on the new seam.

That implies:

- the old `aos-sqlite`-shaped node-store seam was transitional and should not remain the local
  architecture center of gravity
- local should stop depending on the old hosted persistence concepts surviving forever
- local should be rewritten around log/checkpoint/fabric semantics once those contracts exist

## Current Remaining Scope

Completed elsewhere:

- local secrets are no longer part of P6 scope; they were completed as env/`.env` local-only
  resolution in P9
- the old FDB-shaped hosted path is gone

There is no remaining phase-critical P6 scope after the local workspace cutover.

## Current Reset Direction

The next local refactor should not preserve the current halfway model.

It should reset the local runtime so that it is structurally as close to the Kafka/log design as
possible while still taking advantage of single-process embedding.

Required stance:

- local should preserve the same authoritative boundaries as hosted
- local should emulate Kafka semantics, not Kafka storage mechanics
- local should not keep fake topic-shaped durable tables just to resemble hosted infrastructure
- local should not keep a durable ingress queue in the default one-process owner model unless a
  specific product surface proves it necessary

### Default local owner model

For the default local mode, the owner is hot and in-process.

That means the normal path should be:

1. submit into the local node/runtime
2. inject into the hot `WorldHost`
3. let the runtime/kernel normalize and execute
4. capture the resulting canonical journal tail
5. persist the authoritative world-log frame

Not:

1. write a durable local inbox/topic row
2. wake a second local worker loop
3. later translate the row into the journal

The latter may be useful for optional hosted-sim or debugging modes, but it should not define the
default local architecture.

### Durable local planes

The durable local core should be reduced to the minimum planes needed to match hosted semantics:

- runtime metadata
- world directory / route metadata
- authoritative world-log frames
- checkpoint / active-baseline metadata
- blob / CAS storage

Everything else should be treated as optional operational state or product projection.

In particular:

- command status/result tracking is not authoritative history
- local effect/fabric work queues are not authoritative history
- raw submission envelopes are not authoritative history

### Hot-state reads

Normal local reads should come from hot runtime state.

SQLite is primarily for durability and restart, not the steady-state query engine.

That implies:

- manifest/state/trace/runtime reads should prefer the in-memory `WorldHost`
- restart should rebuild hot worlds from checkpoint plus journal replay
- durable tables should be small and clearly plane-shaped rather than a single mixed world-state
  row

### Effects, receipts, and fabric

Local should follow the same semantic pattern as hosted:

1. a submitted event or command drives workflow execution
2. the kernel records canonical intent records
3. local effect/fabric dispatch is driven from hot runtime state or journal-derived runtime state
4. receipts re-enter through the same owner/runtime path
5. the kernel records canonical receipt records

The default local mode does not need a separate durable "topic" table for effect intents, receipts,
or fabric transport in order to preserve these semantics.

If local needs helper indexes for wakeups, retries, or crash recovery, those should be explicitly
non-authoritative operational indexes, not a second journal.

### Schema direction

The local SQLite layout should be renamed and reduced toward the same conceptual planes used by the
Kafka design.

Expected direction:

- a small runtime-meta table
- a small world-directory / route table
- an authoritative journal-frame table
- a checkpoint-head table
- optional clearly non-authoritative projection tables

Not:

- a mixed `worlds` row that behaves like a hidden source of truth
- a durable ingress queue in the default local hot-owner path
- topic-emulation tables whose only purpose is to mimic Kafka storage layout

## Recommended Local Runtime Shape

### Default local mode

One process owns all local partitions for one local universe.

Suggested local implementation:

- local append-only log in SQLite or local files
- local checkpoints on disk
- local CAS/artifacts on disk
- the same logical CAS contract as hosted, including direct or packed blob layouts behind the
  logical hash API
- optional shared on-disk blob cache for multiple local workers/processes
- local secret resolution through env/files/local provider
- local fabric through local processes, containers, or configured sandbox integrations
- the same layered keyed-workflow head model as hosted: snapshot-anchored `CellIndex` roots plus
  in-memory clean cell cache plus dirty delta layer with spill-to-CAS behavior

### Optional hosted-sim mode

Later, local may support a hosted-simulation mode using:

- local Kafka-compatible broker
- local S3-compatible object store

But that should be optional and explicitly for reproduction/testing, not the default developer UX.

## Product Surface Consequences

### 1) Local and hosted should share semantics, not necessarily infrastructure

That is the important compatibility target.

More specifically:

- local and hosted should agree on which facts are authoritative world history and which are only
  submission, transport, or projection detail
- local should not preserve old seams by reifying request-shaped control or fabric messages into
  local journal history
- local and hosted should normalize equivalent ingress causes into the same canonical replay
  records even if the surrounding infrastructure is different

### 2) Import/export and bridge work become cleaner

Because local and hosted use the same record/checkpoint model, moving between them becomes much
less special.

### 3) Smoke/eval/batch should exercise the new semantics

The goal should be realistic local execution without forcing hosted infrastructure on every
developer.

### 4) Hosted control/read surfaces still need a minimum contract

The current runtime and product surface already assume operator reads such as:

- manifest and definition lookup
- workflow/cell state reads
- journal-head and active-baseline visibility
- governance/trace/quiescence summaries
- current route visibility

The new architecture may serve those reads from authoritative planes or derived materializations,
but it should define them explicitly and keep them clearly non-authoritative unless the read is
directly over an authoritative plane such as the route directory or journal metadata.

## Cutover Plan

### Phase 1

Keep current local and current hosted runtime working while the new shared contracts are defined.

### Phase 2

Implement the new shared log/checkpoint/fabric contracts in the shared node/runtime layer.

### Phase 3

Rebuild local on top of those contracts using an embedded backend.

### Phase 4

Implement the Kafka/S3 hosted backend on the same contracts.

### Phase 5

Retire or demote the old FDB-shaped hosted path once the new path is credible.

Status: done. The old FDB-shaped hosted path has been removed.

## Semantic Conformance

The embedded local backend and the Kafka/S3 hosted backend should be required to pass the same
semantics-level cases for:

- replay-or-die snapshot restore
- strict quiescence and apply blocking
- checkpoint publication and recovery
- stale submission/receipt rejection across route-epoch changes
- equivalent normalization from ingress causes to canonical manifest/world-record history

## Repository Direction

Expected consequences:

- `aos-node` remains the shared seam, but the hosted-facing traits must change materially
- `aos-node-local` remains the local composition/product layer
- local SQLite/files runtime code should live directly in `aos-node-local`; `aos-sqlite` is not
  the future node-runtime seam
- `aos-fdb` / `aos-node-hosted` become transitional rather than the future center of gravity

## Out of Scope

1. Immediate removal of current local or current hosted binaries.
2. Requiring local developers to run Kafka and S3 by default.
3. Full bridge/import/export design in this document.

## DoD

1. The roadmap states that local remains a first-class product surface.
2. The roadmap states that local and hosted should converge semantically under the new seam.
3. The roadmap preserves a lightweight embedded local default.
4. The roadmap declares current local and current hosted implementations transitional with respect
   to the new architecture.
5. The roadmap calls out a minimum hosted read/control surface and shared semantic conformance
   expectations across embedded and Kafka/S3 backends.
