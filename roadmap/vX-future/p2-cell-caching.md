# p2: Cell Caching (Hot/Cold) and Snapshot-Anchored Persistence

This document explores a future design for reducer *cell* state caching and persistence that prioritizes:

- high throughput with **millions of events**
- low write amplification (especially for **hot cells**)
- bounded memory via aggressive eviction/offload
- deterministic semantics and “replay-or-die” correctness

It deliberately assumes an experimental environment: we can change snapshot formats, internal storage layouts, and kernel internals without backward-compat constraints.

## Context / Why This Exists

The current keyed-cell implementation behaves like a *write-through cache*:
- on every reducer step that changes state, the kernel writes the new state bytes to CAS and updates a persistent `CellIndex` root.
- cache eviction is cheap because the canonical state is already in CAS.

That is conceptually simple, but it has two problems at scale:
- **write amplification**: hot cells updated frequently produce a new CAS blob per update.
- **CAS bloat**: without GC, immutable blobs accumulate roughly proportional to number of updates (not number of distinct keys).

This p2 design aims to move to a *snapshot-anchored* persistence model:
- between snapshots, the kernel may keep the authoritative “latest state” in memory (or reconstructable from the journal),
- CAS writes for cell state become primarily:
  - snapshot commits, and
  - eviction spill for cold/large cells (runtime optimization only)

## Goals

1) **Hot cells stay hot**: avoid writing hot cells to CAS on every event.
2) **Evict aggressively**: after seconds of inactivity and/or size/memory pressure, drop resident bytes.
3) **Snapshot commits are fast**: few dirty resident cells remain at snapshot time because eviction has already spilled cold state.
4) **Deterministic world semantics**: cache/eviction timing must not change the logical world state.
5) **Restart is rare, replay is acceptable**: on restart, load the last snapshot and replay all events since that snapshot.

## Non-Goals

- Skipping journal replay after restart by using “post-snapshot spill blobs”.
  - Spill blobs are treated as cache artifacts; they may exist but are not authoritative.
- Perfectly minimal CAS usage without GC.
  - CAS GC is expected to exist; this design just makes it easier by reducing needless writes.
- A per-(reducer,key) event index for partial replay.
  - We assume sequential replay from snapshot is the recovery mechanism.

## Key Assumptions (Explicit)

1) **Recovery model**: restart loads the newest snapshot, then replays *every* journal record after the snapshot height, in order.
2) **Authoritative persistence**: the snapshot is the only checkpoint the kernel trusts for state; anything written after snapshot time is ignored on restart.
3) **Caching is an optimization**: eviction/spill policies may be non-deterministic and depend on wall-clock and memory pressure, as long as they cannot affect logical state.

## Design Overview

We separate *logical state* from *cache state*:

- **Logical state** (deterministic): snapshot + journal replay defines the state at any journal height.
- **Cache state** (optimization): resident bytes, spill blobs, and eviction timing.

Concretely, we introduce an LSM-like two-level cell state view per reducer:

1) **Base layer**: `base_root` — a persisted cell index root captured in the most recent snapshot.
   - Maps `key_hash -> CellMeta{ state_hash, ... }`.
   - This layer is stable between snapshots.
2) **Delta layer**: `delta` — an in-memory overlay (memtable) for all mutations since the last snapshot.
   - Maps `key_hash -> DeltaEntry` (latest state or delete tombstone).
   - This layer is discarded on restart and reconstructed via journal replay.

Eviction works against *resident bytes* (hot cache), not against the authoritative mapping:
- if a cell’s bytes are evicted, we must still be able to load them on demand without replay.
- therefore eviction may **spill** the latest bytes to CAS and keep only a `state_hash` pointer in the delta layer.

### Why the Base/Delta Split Matters

It yields three properties simultaneously:

- **No write-on-update for hot cells**: updates touch only the in-memory delta, not CAS.
- **Cheap cache-miss loads**: if a cell’s latest bytes were spilled, we can reload by `state_hash` without replay.
- **Deterministic semantics**: snapshots only ever refer to `base_root` produced by deterministic snapshot commit logic.

## Spec: Cell State View

All state reads and reducer invocations MUST observe the same logical view:

Given `(reducer, key)`:
1) If `delta` contains an entry for `key_hash`:
   - If it is a tombstone: state is `None`.
   - Else state is the delta’s latest state (resident bytes if present, otherwise load from CAS by `state_hash`).
2) Else consult the snapshot base index:
   - Look up `CellMeta` in `base_root` via `CellIndex`.
   - If found, load bytes from CAS by `CellMeta.state_hash`.
   - Else state is `None`.

This means the kernel always has a total order: `delta overrides base`.

## Spec: Delta Entry

Delta entries represent “the latest value since the last snapshot”.

Recommended shape:

```text
DeltaEntry {
  key_bytes: Vec<u8>            // for diagnostics and snapshot commit; may be elided if derivable
  key_hash: [u8; 32]
  kind: Upsert | Delete
  state_hash: [u8; 32]          // hash(bytes) of latest state when kind=Upsert
  resident: Option<Vec<u8>>     // present if currently cached in memory
  dirty: bool                   // true if changed since last snapshot (always true for delta entries)
  spilled: bool                 // true if resident bytes have been written to CAS
  last_access: CacheTime        // cache-only bookkeeping (may be nondeterministic)
  approx_size: usize            // cache-only; used for memory pressure heuristics
}
```

Notes:
- `state_hash` MUST be computed as the content hash of the bytes.
- `dirty` is redundant if the delta exists at all; included for clarity.
- `spilled` indicates whether CAS has a blob for the current `state_hash`.

## Spec: Persistence Modes

The kernel SHOULD support a per-world or per-reducer mode. Suggested enum:

1) `WriteThrough` (current behavior)
   - On every update: `put_blob(state)`, update persisted index root immediately.
   - Pros: simplest, great cache-miss performance, minimal replay dependency.
   - Cons: worst write amplification, CAS bloat.

2) `SnapshotCommit` (this p2 proposal; default for large scale)
   - On update: mutate delta only; do not write to CAS.
   - On eviction: spill to CAS if needed; keep only `state_hash` pointer.
   - On snapshot: deterministically commit delta into a new base index root and persist it.

3) `SnapshotOnlyNoSpill` (mostly for experimentation)
   - Never spill on eviction; eviction implies you must keep bytes resident or rebuild by replay-on-access.
   - This is generally not acceptable without a per-key event index because cache misses become “scan replay suffix”.

This document focuses on `SnapshotCommit`.

## Spec: Reducer Step Semantics Under SnapshotCommit

When a reducer step produces `output.state`:

- If `output.state = Some(bytes)`:
  - Compute `state_hash = Hash(bytes)` (hashing only).
  - Write **no CAS blobs** here.
  - Update delta entry:
    - `kind = Upsert`
    - `resident = Some(bytes)`
    - `state_hash = ...`
    - `spilled = false` (because current bytes are not known to be in CAS yet)
- If `output.state = None`:
  - Update delta entry:
    - `kind = Delete` (tombstone)
    - drop resident bytes
    - `spilled` irrelevant

Important: the reducer’s logical state evolution MUST depend only on the input state bytes (from view rules above) and event bytes; caches do not change these.

## Spec: Eviction / Offload

Eviction is allowed to be nondeterministic and policy-driven (time since access, size, global memory pressure).

However, eviction MUST preserve this invariant:

> After eviction, the kernel must still be able to supply the correct prior state bytes for the next reducer step for that key *without scanning the journal*.

Therefore:
- If evicting a delta upsert entry with `resident=Some(bytes)` and `spilled=false`:
  - perform `put_blob(bytes)` to CAS,
  - set `spilled=true`,
  - set `resident=None`.
- If evicting a base-only entry (no delta entry) and the base index points to CAS:
  - simply drop resident bytes (if any); no write required.

### “Evict after a few seconds” (wall-clock policy)

Using wall-clock time is fine because it’s cache-only.
But any wall-clock timestamps MUST NOT appear in:
- journal records
- snapshot payloads
- any deterministic hashes that are compared across replay

## Spec: Snapshot Commit

Snapshot commit is the only place where we “publish” authoritative cell refs for recovery.

A snapshot commit MUST:
1) ensure that any delta upserts that will be referenced by the snapshot are materialized in CAS (blob exists)
2) produce a deterministic new `base_root` that reflects base + delta
3) write the snapshot (including the new `base_root`) as usual
4) clear the delta (or mark entries clean and drop their cache-only metadata)

### Step 1: Materialize all delta upserts

For each delta entry where `kind=Upsert`:
- if `resident=Some(bytes)` and `spilled=false`: `put_blob(bytes)` and set `spilled=true`
- if `resident=None`: it must already be `spilled=true` (otherwise we lost the bytes)

### Step 2: Build new base_root deterministically

We need deterministic behavior even if delta is a hash map internally.

Required rule:
- Apply delta entries in a deterministic order, e.g. lexicographic by `(key_hash bytes, key_bytes)` (or just `key_hash` if collision handling is defined).

Algorithm:
- let `root = old_base_root` (or empty if none)
- for each delta entry in sorted order:
  - if `Delete`: `root = index.delete(root, key_hash)`
  - if `Upsert`: build `CellMeta` and `root = index.upsert(root, meta)`
- `new_base_root = root`

This yields work proportional to the number of changed keys since last snapshot (not number of events).

### Step 3: Snapshot write

Snapshot payload MUST include `new_base_root` for all reducers (keyed and non-keyed).
Non-keyed reducers can be treated as a single “sentinel key” cell (implementation detail).

### Step 4: Delta reset

After snapshot commit:
- `base_root := new_base_root`
- `delta := empty`
- any spill blobs that were created but not referenced by the snapshot are now unreachable and can be GC’d (see GC section).

## Spec: Restart / Replay

On restart:
1) Load latest snapshot → establishes `base_root` for each reducer.
2) Discard any leftover runtime delta/spill metadata.
3) Replay all journal entries after snapshot height in order.

Critically:
- We DO NOT attempt to use post-snapshot spill blobs to skip replay.
- Therefore, post-snapshot spill blobs are *cache artifacts*; correctness relies on journal replay.

## CAS Size and Garbage Collection Implications

With `SnapshotCommit`:
- Hot keys generate **at most one new state blob per snapshot interval** (latest state only).
- Cold keys may generate blobs on eviction, but typically only when they transition from hot→cold.
- The main driver of CAS growth becomes “#distinct versions per key per snapshot”, not “#events”.

GC root set SHOULD include:
- all snapshot blobs and anything reachable from them (including cell index nodes and referenced state blobs)
- any journal-pinned artifacts (if journal stores CAS refs; currently snapshots are refs, most events are inline)
- a runtime “pin set” for spill blobs that are only referenced in memory (optional; see below)

### Runtime spill blobs and GC safety

In this design, spill blobs written between snapshots are not necessarily reachable from any persisted root.

Options:
1) **Run GC only right after snapshot commit**
   - safest: after commit, either spill blobs became reachable (in new base_root) or delta is cleared, so no need to keep unrooted spill blobs.
2) **Maintain a runtime pin set**
   - GC consults this in-memory list to avoid collecting spill blobs still needed before next snapshot.

Given the complexity, option (1) is preferred initially.

## Observability / Introspection

We should be able to answer:
- how many cells are resident vs spilled vs base-only
- how many CAS writes are happening due to eviction vs snapshot commit
- how many delta entries exist per reducer (dirty set size)
- top-N hot cells by access frequency/bytes

Suggested metrics:
- `cells_resident_bytes_total`
- `cells_resident_count`
- `cells_delta_count`
- `cells_spill_put_blob_count`
- `cells_snapshot_put_blob_count`
- `cells_snapshot_commit_keys`
- `cells_cache_hit_count`, `cells_cache_miss_count`

## Things to Pay Attention To (Sharp Edges)

1) **Correctness after eviction**
   - Never evict away the last bytes of a delta entry unless it has been spilled.
2) **Deterministic snapshot building**
   - Delta application order must be deterministic.
3) **Index/listing correctness**
   - Any `list_cells` implementation must merge base + delta (or clearly document that it only shows snapshot state).
4) **Memory use**
   - Hot cells can retain large state; add size-based eviction triggers.
5) **Snapshot spikes**
   - If eviction is too conservative, snapshot may need to flush many large resident states at once.
6) **Time-based eviction**
   - Safe as cache-only, but keep timestamps out of deterministic persisted state.

## Detailed Implementation Plan (Kernel)

This is a suggested step-by-step migration plan. Exact file boundaries may change.

### 0) Preconditions / Refactors

- Unify keyed + non-keyed state access behind a single internal “cell state view” API:
  - `get_cell_state(reducer, key) -> Option<Vec<u8>>`
  - `set_cell_state(reducer, key, Option<Vec<u8>>) -> ()`
- Introduce a sentinel key for non-keyed reducers so both paths share the same machinery.

### 1) Add a CellPersistenceMode config

- Add config to kernel builder and/or manifest defaults:
  - `WriteThrough` (current)
  - `SnapshotCommit` (new)

### 2) Introduce base_root + delta per reducer

- Maintain:
  - `base_root[reducer]` loaded from snapshot
  - `delta[reducer]` in-memory overlay

### 3) Update read path to consult delta then base

- Modify state read logic used by reducer invocation and queries to:
  - check delta first
  - fall back to `CellIndex` lookup from `base_root`

### 4) Update write path to mutate delta only (SnapshotCommit)

- In reducer output handling:
  - compute hashes
  - store bytes resident
  - do **not** persist blobs or index nodes

### 5) Implement eviction with spill-to-CAS

- Replace/extend the current simplistic LRU cache with:
  - eviction triggers: idle time, size threshold, memory pressure
  - eviction action:
    - if dirty + resident: `put_blob` then drop bytes
    - if base-only: drop bytes

### 6) Implement snapshot commit (merge delta into new base_root)

- On snapshot request:
  - flush dirty resident cells (materialize blobs)
  - deterministically apply delta to base via `CellIndex` ops
  - write snapshot containing new base roots
  - clear delta

### 7) Update introspection surfaces

- `list_cells` should merge base + delta.
- “get cell state” should report where it came from (resident/spill/base) for debugging.

### 8) GC integration

- Initially: run GC only after snapshot commit.
- Later: add runtime pin set if needed.

## Testing Strategy (Deterministic)

Add tests that assert behavior independent of eviction timing:

1) **Replay identity**
   - Run: apply events → snapshot → apply more events → snapshot.
   - Restart from the last snapshot and replay → final state bytes identical.
2) **Eviction correctness**
   - In `SnapshotCommit` mode, apply an update, then force eviction, then apply another event → reducer sees correct prior state.
3) **Snapshot commit determinism**
   - Populate delta with multiple keys in random insertion order, snapshot commit, and assert the resulting `base_root` hash is stable.
4) **CAS write accounting**
   - Hot key updated N times without eviction produces at most 1 state blob per snapshot.

## Rollout / Milestones

1) Implement `SnapshotCommit` behind a feature flag (keep `WriteThrough` default).
2) Enable for a single reducer (or a test world) and validate correctness.
3) Add eviction policies (time/size/pressure) and instrumentation.
4) Make `SnapshotCommit` the default for large-world scenarios.
5) Add GC constraints/pinning once CAS bloat becomes observable.
