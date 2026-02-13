# P1: Protocol Roots, Baselines, and Immediate Runtime Work

**Priority**: P1  
**Effort**: Medium/High  
**Risk if deferred**: High (future infra and storage work will harden wrong semantics)  
**Status**: Implemented (core), February 13, 2026

## Completion Snapshot

- Completed:
  - `BlobEdge` schema + `blob.put@1` schema/receipt updates
  - `blob.put@1` runtime normalization + deterministic `edge_ref`
  - baseline snapshot record semantics (`logical_time_ns`, `receipt_horizon_height`)
  - active-baseline enforcement on world init (unseeded worlds)
  - baseline-aware restore behavior
  - receipt-horizon baseline promotion precondition checks
  - snapshot root-completeness metadata + fail-closed validation on write/load
  - reducer-state typed-hash traversal helper (schema-known traversal only)
  - correctness tests listed below (except seeded/fork-specific create semantics)
  - spec alignment updates in `spec/02-architecture.md`, `spec/03-air.md`, `spec/07-gc.md`
- Remaining/N-A in current codebase:
  - seeded/fork world-create baseline pointer semantics are not implemented as a dedicated creation surface in this repository yet (no separate seed/fork world creation API path exists to wire).

## Goal

Ship the minimum complete slice needed now for GC-safe semantics and baseline restore correctness.  
This milestone includes protocol/schema updates plus the immediate kernel/runtime behavior required to exercise them.

## Scope (Now)

This P1 replaces broader snapshot-track decomposition. It is the only in-scope roadmap item for `v0.11-snapshots`.

### 1) Protocol and schema contract

1. Add built-in schema:
   - `sys/BlobEdge@1 = { blob_ref: hash, refs: list<hash> }`
2. Update `blob.put` schemas in place (`@1`, no version bump):
   - `sys/BlobPutParams@1 = { bytes: bytes, blob_ref?: hash, refs?: list<hash> }`
   - `sys/BlobPutReceipt@1 = { blob_ref: hash, edge_ref: hash, size: nat }`
3. Define baseline snapshot record semantics:
   - `snapshot_ref: hash`
   - `height: nat`
   - `logical_time_ns: nat`
   - `receipt_horizon_height?: nat`
4. Make snapshot root completeness explicit:
   - `manifest_hash`
   - reducer state roots
   - keyed reducer `cell_index_root`
   - workspace roots (directly or via reducer state)
   - additional `pinned_roots[]`
5. Define world-creation baseline requirement:
   - every world must have an `active_baseline`
   - unseeded create writes an initial empty/default baseline snapshot
   - seeded/forked create points `active_baseline` at the seed/fork snapshot

### 2) Immediate kernel/runtime behavior

1. Implement `blob.put@1` handling:
   - compute `computed_ref = sha256(bytes)`
   - reject if provided `blob_ref != computed_ref`
   - normalize missing `blob_ref` to `computed_ref` before journaling/dispatch
   - treat omitted `refs` as `[]`
   - persist `sys/BlobEdge@1`
   - return deterministic receipt containing `edge_ref`
2. Implement baseline-aware restore path:
   - load baseline snapshot
   - replay journal tail where `height >= baseline.height`
3. Add receipt-horizon safety checks on baseline promotion/acceptance paths.
4. Validate snapshot root completeness on write/load paths that create restore roots.
5. Implement reducer-state reachability semantics:
   - reducer state roots are required snapshot roots
   - schema-known reducer state is traversable for typed `hash` refs
   - hashes hidden in opaque bytes/text are not auto-discovered and require explicit refs
6. Implement world creation/init behavior that always sets `active_baseline`.

### 3) Spec/documentation alignment

Update and align:

- `spec/03-air.md`
- `spec/02-architecture.md`
- `spec/07-gc.md`
- built-ins under `spec/defs` and related schema artifacts under `spec/schemas`

### 4) Tests (correctness only)

1. `blob.put` ref mismatch rejects deterministically.
2. Omitted `refs` behaves as leaf (`refs=[]`) and returns stable `edge_ref`.
3. Baseline + tail replay is byte-identical to full replay (`replay-or-die`).
4. Unsafe baseline promotion fails on receipt-horizon precondition.
5. Snapshot root completeness checks fail closed when required roots are missing.
6. Typed refs in reducer state are reachable in traversal tests; opaque embedded hashes are not treated as refs.
7. World creation tests verify an `active_baseline` exists immediately (empty/default for unseeded, seed/fork snapshot for seeded worlds).

## Invariants

- GC traversal never parses arbitrary blob bytes.
- CAS references must be explicit in typed nodes or blob-edge nodes.
- Opaque blobs are leaves unless explicit refs are provided.
- Reducer state is traversed only when schema-known; opaque embedded hashes are out of contract unless explicitly attached.
- Baseline validity is fenced by receipt horizon semantics.
- Every world has an active baseline; restore never depends on a separate no-baseline mode.
- Replay-or-die is strict: baseline+tail output must match full replay exactly.

## DoD

1. Schema and built-in definitions for `BlobEdge` and updated `blob.put@1` are merged.
2. Kernel/runtime supports normalized `blob.put@1` and emits `edge_ref` receipts.
3. Baseline restore path works and is replay-identical.
4. Receipt-horizon baseline safety checks are enforced.
5. Snapshot root completeness is documented and enforced by code paths that create/accept restore roots.
6. World create/init code paths always persist `active_baseline`.
7. Deterministic tests cover the cases listed above.

## Explicitly Out of Scope

- Mark/sweep deletion execution.
- Time-based retention planner/run surfaces.
- Journal hot/cold compaction and segment architecture.
- Automatic ref extraction/materialized adjacency index on every node write (`hash -> refs[]`).
- Storage trait refactor and alternate local KV backends.
- Distributed scheduler/lease/universe execution work.
