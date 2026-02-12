# P1: Protocol Roots, Baselines, and GC Contract

**Priority**: P1
**Effort**: Medium
**Risk if deferred**: High (infra and universes may bake in non-GC-safe storage semantics)
**Status**: Proposed

## Goal

Lock the AOS protocol semantics needed for future local + distributed GC before
building hosting infra. This sprint is schema/protocol groundwork, not deletion.

## Why now

Infra will introduce shared CAS, shared journals, shared snapshots, and world
movement across workers. If root semantics and blob-edge semantics are implicit,
we cannot safely compact journals or collect CAS in either local FS or
future distributed stores.

## Decision Summary

1) Baseline snapshots become semantic restore roots, not only performance hints.
2) CAS references must be explicit in typed nodes or blob-edge nodes.
3) Opaque blobs are leaves unless accompanied by explicit refs.
4) Baseline safety is fenced by receipt horizon semantics.
5) `blob.put` schemas are updated in place (`@1`) with no version bump.

## Protocol/Schema Changes

### 1. Blob edge node

Add built-in schema:

- `sys/BlobEdge@1`
- Fields: `{ blob_ref: hash, refs: list<hash> }`

Purpose: records references that cannot be discovered from opaque blob bytes.

### 2. Blob put schema update (in place)

Update the existing `@1` schema pair in place:

- `sys/BlobPutParams@1 = { bytes: bytes, blob_ref?: hash, refs?: list<hash> }`
- `sys/BlobPutReceipt@1 = { blob_ref: hash, edge_ref: hash, size: nat }`

Semantics:

- Kernel computes `computed_ref = sha256(bytes)` on ingest.
- If `blob_ref` is provided and does not match `computed_ref`, reject.
- If `blob_ref` is omitted, kernel normalizes requested params by setting
  `blob_ref = computed_ref` before journaling/dispatch so downstream events are
  stable.
- Adapter stores blob bytes as today.
- Kernel writes `sys/BlobEdge@1` node (empty `refs` if omitted).
- Receipt returns `edge_ref` for persistence in state/events.

### 3. Baseline snapshot journal entry

Define `BaselineSnapshot` record (new entry type or explicit snapshot flag):

- `snapshot_ref: hash`
- `height: nat`
- `logical_time_ns: nat`
- `receipt_horizon_height?: nat`

Restore semantics:

- `state = load(snapshot_ref)`
- replay journal entries with `height >= baseline.height`

### 4. Snapshot root completeness

Clarify snapshot contract (spec + implementation docs): snapshot must carry all
roots needed for deterministic restore and future GC traversal, including:

- `manifest_hash`
- reducer state roots
- keyed reducer `cell_index_root`
- workspace roots (direct or via reducer state)
- additional `pinned_roots[]`

## Invariants

- GC traversal never parses arbitrary blob bytes.
- Any new feature that introduces CAS refs must use typed hash fields or
  `blob.put refs`.
- Baseline is valid only when receipt horizon precondition holds.
- Replay-or-die remains strict: baseline + tail must be byte-identical to full
  replay snapshot output.

## Deliverables

- Built-in schema updates in `spec/defs` + `spec/schemas`.
- AIR/spec text updates in `spec/03-air.md` and `spec/02-architecture.md`.
- Kernel decoding/validation support for updated `blob.put@1` payload shapes.
- Tests proving omitted `refs` is treated as leaf behavior (`refs = []`).

## DoD

- `blob.put@1` roundtrips with `edge_ref` and deterministic receipt encoding.
- Baseline entry can be written and loaded by kernel/runtime.
- Snapshot contract explicitly documents required roots for future GC.
- Tests cover: typed refs, blob-edge refs, and omitted-refs leaf blobs.

## Non-Goals

- No mark/sweep deletion yet.
- No journal segment deletion yet.
- No distributed scheduler/lease work yet.
