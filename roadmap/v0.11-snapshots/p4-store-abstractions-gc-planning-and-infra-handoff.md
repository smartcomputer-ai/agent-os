# P4: Store Abstractions, GC Planning, and Infra Handoff

**Priority**: P4
**Effort**: Medium/High
**Risk if deferred**: High (infra may couple to FS-specific assumptions)
**Status**: Proposed

## Goal

Finalize the kernel/storage interfaces and GC planning surfaces required for
infra/universe implementation, while keeping current local behavior intact.

## Decision Summary

1) Introduce storage traits now; do not block on backend migration.
2) Keep FS backend as default, add optional local KV backend later.
3) Build GC planning/root enumeration now; defer sweep deletion.
4) Deliver a clear handoff contract for distributed infra.

## Storage Abstraction Boundary

Define interfaces (names illustrative):

- `CasStore` (`put/get/has`, hash verification)
- `JournalStore` (append, scan, segment index, head)
- `SnapshotStore` + `SnapshotIndexStore`
- `QueueStore` (for future inbox/outbox/timers)
- `LeaseStore` (future worker ownership)

Requirements:

- deterministic read ordering
- canonical CBOR boundaries preserved
- no ambient mutable refs in CAS APIs

## Local Backends

### 1. FS backend (required now)

- remains reference implementation
- supports baseline restore, retention, and segmented compaction

### 2. Optional local KV backend (candidate: RocksDB)

- not a prerequisite for infra
- useful for laptop scale and performance testing
- must satisfy same replay and ordering invariants as FS

## GC Planning Surfaces (No Sweep Yet)

Add root enumeration and dry-run APIs:

- `aos gc plan`
- root categories:
  - retained baselines
  - journal tail/segments in retained window
  - active manifest root
  - explicit operator pins

Planner output:

- reachable object counts/sizes (estimated where needed)
- candidate unreachable objects
- reason trails for retained roots

## Infra/Universe Handoff Contract

Before infra execution starts, this sprint should provide:

- stable root and baseline semantics
- segmented journal and compaction semantics
- retention and compaction planner outputs suitable for orchestration
- storage traits that map to distributed implementations
  (shared CAS, shared journal index, shared queue/lease systems)

## DoD

- Storage traits exist and are used by kernel/host paths touched in P1-P3.
- FS backend passes replay-or-die under baseline + compaction + retention.
- `gc plan` can enumerate roots and produce deterministic output.
- Infra team can implement distributed backends without semantic ambiguity.

## Non-Goals

- Distributed GC execution.
- Cross-world union-root mark/sweep in shared CAS.
- Full universe orchestrator.
