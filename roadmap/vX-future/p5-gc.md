# p5-gc: World GC + CAS Roots (Future)

## TL;DR
Make snapshots semantic baselines and require explicit CAS edges. GC walks only
baseline roots + the journal tail, following typed nodes and small blob-edge
nodes. Opaque blobs are leaves. Log compaction + mark/sweep keeps worlds bounded.

---

## Goals
- Bound on-disk growth for journals, snapshots, and CAS.
- Deterministic replay: baseline snapshot + journal tail reproduces state.
- GC cost proportional to live data, not total historical blobs.
- Clear story for multi-world shared CAS.

## Non-Goals
- Real-time GC or per-event collection; offline/periodic GC is fine.
- Guaranteed full audit history in the hot store (archive is optional).
- Parsing arbitrary blob payloads to discover references.

---

## Terms
- Baseline snapshot: the active restore anchor for a world.
- Journal tail: events at heights >= baseline height.
- CAS node: canonical CBOR with a schema (typed); safe to traverse for refs.
- CAS blob: raw bytes (opaque); not traversed by GC.
- Edge node: a small typed node recording blob -> refs for opaque blobs.

---

## Design Overview (Decisions)
1) Baselines are semantic: the world restores from the latest baseline snapshot
   plus the journal tail. Events before the baseline are no longer required for
   state and may be deleted or archived.
2) Explicit edges: all references between CAS objects must be visible in typed
   nodes or in a dedicated BlobEdge node. GC never parses raw blobs.
3) Opaque blobs are leaves: if you embed refs in raw JSON/bytes, GC will not see
   them and may collect the targets. This is explicitly unsupported.

---

## CAS Graph Model

### Nodes vs Blobs
- Nodes are typed, canonical CBOR (e.g., AIR defs, snapshots, receipts).
  GC can parse and follow `hash` fields according to schema.
- Blobs are opaque bytes (e.g., images, large LLM outputs). GC does not
  parse blob payloads and cannot discover refs inside them.

### Blob Edges (New)
Introduce a small typed node to record the edges for opaque blobs:

```
sys/BlobEdge@1 = {
  blob_ref: hash,
  refs: list<hash>
}
```

`refs` is the transitive list of CAS objects the blob depends on (e.g. image
bytes used by a message, tool call payloads, embedded attachments).

### Blob Put (`@1` in-place update)
Update `blob.put@1` so writers can provide refs at write time:

```
sys/BlobPutParams@1 = { bytes: bytes, blob_ref?: hash, refs?: list<hash> }
sys/BlobPutReceipt@1 = { blob_ref: hash, edge_ref: hash, size: nat }
```

Behavior:
- The adapter stores the blob bytes as before.
- The kernel creates a `sys/BlobEdge@1` node with `refs` (empty if omitted).
- The receipt returns `edge_ref` so callers can persist it in state.

### Rule: No Opaque Refs
Any new feature that needs to reference other CAS objects must:
- Use typed nodes with `hash` fields, or
- Use `blob.put` with `refs`.
Opaque blobs without `refs` are treated as leaf objects.
This is the core constraint that makes GC tractable.

---

## Baseline Snapshots (New Semantics)

### Baseline Journal Entry
Introduce a journal entry (or extend `Snapshot`) to mark a baseline:

```
BaselineSnapshot = {
  snapshot_ref: hash,
  height: nat,
  logical_time_ns: nat,
  receipt_horizon_height?: nat
}
```

`receipt_horizon_height` is optional but recommended. It is the latest height
below which no more receipts are expected (see Receipt Horizon).

### Restore Semantics
```
load snapshot_ref -> state
replay journal entries with height >= baseline.height
```

### Retention Policy
Keep at least one baseline; optionally retain the last N baselines. All data
not reachable from the retained baselines + tail is eligible for GC.

---

## Log Compaction

### Baseline Procedure
1) Select snapshot S at height H (old enough to be safe for receipts).
2) Write `BaselineSnapshot { snapshot_ref: S, height: H }`.
3) Rotate/rewire journal segments so that entries < H are excluded from the
   hot journal (delete or archive).
4) Update world metadata to point at the new baseline.

### Compaction Modes
- Ephemeral: keep 1-2 baselines, delete segments < H.
- Hot + Archive: move segments < H (and their CAS objects) to cold storage.
- Audit: keep governance history indefinitely, GC runtime data only.

---

## CAS GC: Mark and Sweep

### Root Set (Per World)
1) All retained baseline snapshots.
2) Journal tail events (>= baseline height).
3) Active manifest hash (control-plane root).
4) Explicit pins (operator-managed).
5) Optional governance archives (if retained locally).

### Traversal Rules
- Typed nodes: decode canonical CBOR by schema and follow `hash` fields.
- BlobEdge nodes: follow `blob_ref` and every entry in `refs`.
- Blobs: never parse; only kept if reachable via nodes/edges.

### Algorithm
1) Build root worklist (above).
2) Mark reachable CAS objects by traversing nodes and edges.
3) Sweep unmarked objects from `.aos/store/{nodes,blobs}/sha256/*`.
4) Optional: only sweep objects older than a watermark for safety.

### Shared CAS (Multi-World)
If CAS is shared across worlds, the root set is the union of all world roots.
GC must see all worlds or the store should be per-world to avoid cross-world
accounting complexity.

---

## Receipt Horizon (Safety)
Baselines must not drop intents that could still produce receipts:

- Define a per-effect TTL or a global receipt horizon.
- A baseline at height H is only valid if no receipts for intents < H can
  arrive (or you accept permanently ignoring them).
- Late receipts after a baseline are ignored by intent fences as today.

This is a precondition for safe log truncation and CAS GC.

---

## Migration Strategy

### Backwards Compatibility
- `blob.put@1` remains valid; blobs written without `refs` are treated as leaves.
- GC correctness requires that any new refs are visible via typed nodes or
  `blob.put refs`.

### World Upgrade Plan
1) Add `sys/BlobEdge@1` + updated `blob.put@1` schemas.
2) Update reducers/plans/adapters to persist `edge_ref` in state/events.
3) Migrate data that embedded refs inside opaque JSON blobs:
   - Re-encode as typed nodes, or
   - Re-write blobs with explicit `refs`.
4) Declare a baseline after migration; old baselines can then be dropped.

---

## Tooling (CLI / Ops)
- `aos snapshot baseline` -> create BaselineSnapshot entry.
- `aos gc plan` -> list roots, live size estimates, candidate deletions.
- `aos gc run` -> mark/sweep CAS based on current baselines.
- `aos gc pin/unpin <hash>` -> operator-managed roots.

---

## Validation & Tests
- Replay-or-die: baseline + tail yields byte-identical snapshots.
- Mark/sweep correctness: unreferenced objects are deleted; referenced are kept.
- Receipt horizon enforcement: baselines rejected if receipts may still arrive.

---

## Open Questions
- Should BlobEdge nodes live in the node store or blob store?
- Do we need a `blob.meta`/`blob.edge_get` effect for debugging?
- Do we allow typed nodes to provide an optional `refs` list to avoid decode?
- How many baselines to retain by default (1 vs N)?
