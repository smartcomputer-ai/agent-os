# P4: Embedded Universe Mode (Shared Runtime, Local Authority, Future Export/Import)

**Priority**: P4  
**Effort**: High  
**Risk if deferred**: Medium (local worlds remain supported, but they stay on a more legacy host/storage shape longer)  
**Status**: Proposed

## Goal

Ship a first-class embedded universe mode that preserves strong local-world support without maintaining a separate runtime architecture from hosted mode.

Design stance for P4:

- Embedded universes are isolated, local-first, and authoritative on the local machine.
- The worker/runtime loop should be the same architectural shape as hosted mode wherever practical.
- This milestone does not attempt live embedded/hosted communication or two-way sync.
- Movement between embedded and hosted modes remains explicit export/import work and depends on later reachability/export machinery.

Core outcomes:

1. One local worker process can host multiple worlds in one embedded universe.
2. Embedded persistence uses a first-party local backend instead of the current world-root fs-only host shape.
3. Snapshots, baselines, inbox, journal, effect queues, and timer queues follow the same protocol semantics as hosted mode where applicable.
4. Local-first use remains a product-level feature, not just a test harness or legacy compatibility path.

## Dependencies

- Requires `v0.20-infra/p2-hosted-persistence-plane.md` merged far enough to freeze the runtime/storage protocol boundary.
- Requires `v0.20-infra/p3-universe-runtime-plane.md` merged far enough to establish the shared worker lifecycle and multi-world run loop.
- Export/import movement between embedded and hosted modes depends on later GC/reachability-based package/export work and is not required for the core embedded runtime.

## Non-Goals (P4)

- Live communication between embedded and hosted universes.
- Shared CAS federation across embedded and hosted universes.
- Bidirectional sync or offline reattach/merge semantics.
- Active-active embedded clustering.
- A generic pluggable storage abstraction across arbitrary local KV engines.

## Scope (Now)

### 1) First-party embedded persistence backend

Implement one concrete embedded persistence backend for local authority:

- ordered local KV for metadata, journal tail, inbox, leases, effect queues, and timer queues
- local filesystem object storage for blobs, snapshots, and segments
- same logical persistence protocol as the hosted runtime boundary where practical

Notes:

- RocksDB is the default candidate today, but the milestone should choose one concrete engine rather than invent a generic backend matrix.
- Embedded mode should not require FoundationDB at runtime.
- Feature-gating the hosted/FDB client dependency is acceptable if it keeps local-only builds lighter.

### 2) Shared worker/runtime shape

Run embedded worlds through the same general runtime shape as hosted mode:

1. open world by `(universe_id, world_id)`
2. restore from active baseline + tail
3. drain inbox to journal
4. tick kernel
5. publish/settle local effect and timer work
6. snapshot/compact by policy

Differences from hosted mode:

- embedded universes have a single local authority by default
- timers and adapter workers may run in-process
- lease machinery may remain present for code-path reuse, but no distributed failover is required

### 3) Local multi-world hosting

Replace the current implicit "one world root = one runtime authority" shape with an embedded-universe host that can manage multiple worlds in one process.

Required properties:

- many worlds per embedded universe
- shared local CAS/object storage within that universe
- local operational APIs comparable to hosted mode where reasonable
- filesystem AIR assets remain bootstrap/import material, not the authoritative runtime store format

### 4) CLI and operator experience

Extend the CLI/runtime entry points so local operation remains first-class without introducing a second independent host stack.

Desired outcomes:

- one CLI surface can operate embedded and hosted modes
- local-first users do not need hosted services present
- build/deploy packaging can still separate local-only and hosted-capable binaries if operationally useful

### 5) Relationship to hosted mode

In the first embedded milestone:

- embedded universes do not communicate with hosted universes
- `fabric.send` applies within the authoritative persistence plane being used, not across an embedded/hosted boundary
- moving a world between embedded and hosted remains future export/import work

## Export/Import Follow-On

Explicit movement between embedded and hosted modes should be added later only after reachability/export tooling exists.

Required future prerequisites:

- full root reachability walk over baseline roots plus journal/baseline-associated objects
- explicit package/export format for world data and metadata
- import validation and rehydration rules for the destination persistence plane

This follow-on is intentionally separate from the core embedded runtime so P4 does not block on full GC/export machinery.

## Testing and Validation

### Deterministic integration tests

1. Embedded restore from baseline + tail is replay-identical to full replay.
2. One local worker can host multiple active worlds concurrently without cross-world state bleed.
3. Local timers and effects survive restart and restore correctly.
4. Embedded persistence corruption paths fail closed.

### Packaging/ops tests

1. Local-only build/run path works without FoundationDB present.
2. Hosted-capable build still runs embedded universes correctly.
3. CLI/operator commands behave consistently across embedded and hosted modes where semantics overlap.

## Deliverables / DoD

1. First-party embedded persistence backend is implemented and wired to the shared runtime boundary.
2. Embedded universe mode can host multiple worlds in one local worker process.
3. Runtime startup is keyed by universe/world identity rather than filesystem world root assumptions.
4. Local snapshots/baselines/queues/restore semantics are deterministic and tested.
5. Local-first operation remains available without requiring hosted infrastructure.

## Explicitly Out of Scope

- Live hosted/embedded bridging.
- World export/import implementation.
- Shared CAS across embedded and hosted universes.
- Automatic migration or failback between local and hosted modes.
