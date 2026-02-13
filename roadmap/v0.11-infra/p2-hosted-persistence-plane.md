# P2: Hosted Persistence Plane (CAS, Journal, Inbox, Snapshot, Compaction)

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (runtime-plane work will bake wrong persistence semantics)  
**Status**: Proposed

## Goal

Ship a production-grade hosted persistence plane for universes/worlds that preserves existing AgentOS invariants while enabling movable worlds:

1. Shared universe-scoped immutable CAS.
2. Shared world journal with deterministic append ordering.
3. Shared durable world inbox queue for all ingress.
4. Shared snapshot and baseline index with deterministic restore (`baseline + tail replay`).
5. Bounded hot-state growth via segment export (hot tail in KV, cold segments in object storage).

This milestone is the persistence substrate only. It does not include scheduling/orchestration policy (P3).

## Dependency

- Requires `v0.11-infra/p1-protocol-roots-and-baselines.md` merged.
- P2 assumes:
  - `BlobEdge` and `blob.put@1` normalization are active.
  - Baseline promotion checks are active.
  - Snapshot root completeness checks are active.
  - World creation guarantees `active_baseline`.

## Non-Goals (P2)

- Multi-worker scheduling/orchestrator decisions (P3).
- Cross-world `fabric.send` adapter (P3).
- Timer worker and global effect worker pools (P3).
- Mark-and-sweep deletion execution and retention policy UI.
- Multi-region replication strategy.

## Scope (Now)

### 1) Hosted persistence interfaces (freeze contracts first)

Add a backend-agnostic persistence contract crate with deterministic semantics:

- Suggested crate: `crates/aos-persistence` (name can vary, contract cannot).
- Provide traits and canonical types for:
  - CAS
  - World journal
  - World inbox
  - Snapshot index + active baseline
  - Segment index (for cold log materialization)

Suggested core types:

- `UniverseId = Uuid`
- `WorldId = Uuid`
- `JournalHeight = u64`
- `InboxSeq = [u8; 10]` (FDB versionstamp bytes or equivalent sortable token)
- `SegmentId = { start: u64, end: u64 }`

Suggested trait surface (illustrative):

```rust
pub trait CasStore {
    fn put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;
    fn get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError>;
    fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;
}

pub trait WorldJournalStore {
    fn append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: u64,
        entries: &[Vec<u8>],
    ) -> Result<u64, PersistError>; // returns first height

    fn read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: u64,
        limit: u32,
    ) -> Result<Vec<(u64, Vec<u8>)>, PersistError>;

    fn head(&self, universe: UniverseId, world: WorldId) -> Result<u64, PersistError>;
}

pub trait WorldInboxStore {
    fn enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError>;

    fn read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError>;

    fn commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError>;
}

pub trait WorldSnapshotStore {
    fn index_snapshot(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError>;

    fn promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;
}
```

Contract requirements:

- All writes are idempotent where logically expected (`cas.put` by hash, snapshot index at height).
- All read/write paths return typed errors (`conflict`, `not_found`, `validation`, `backend`).
- Contract is deterministic with respect to ordering and normalization boundaries.

### 2) FDB keyspace layout (authoritative metadata + ordering + queues)

Define canonical tuple-subspace layout (logical names; exact tuple encoding is implementation detail).

Universe global:

- `u/<u>/cas/meta/<hash> -> { size, storage, object_key?, inline_bytes? }`
- `u/<u>/effects/pending/<seq> -> EffectDispatchItem` (written in P2; consumed in P3)
- `u/<u>/effects/inflight/<seq> -> EffectInFlightItem` (written in P2; consumed in P3)
- `u/<u>/timers/due/<deliver_at_ns>/<intent_hash> -> TimerDueItem` (written in P2; consumed in P3)
- `u/<u>/segments/<world>/<end_height> -> SegmentIndexRecord`

Per world:

- `u/<u>/w/<w>/meta -> WorldMeta`
- `u/<u>/w/<w>/journal/head -> u64`
- `u/<u>/w/<w>/journal/e/<height> -> canonical_cbor_journal_entry`
- `u/<u>/w/<w>/snapshot/by_height/<height> -> SnapshotRecord`
- `u/<u>/w/<w>/baseline/active -> SnapshotRecord`
- `u/<u>/w/<w>/inbox/e/<seq> -> InboxItem`
- `u/<u>/w/<w>/inbox/cursor -> Option<seq>`
- `u/<u>/w/<w>/notify/counter -> u64`
- `u/<u>/w/<w>/gc/pin/<hash> -> PinReason` (optional, if explicit pin-index is materialized)

Rules:

- `journal/head` monotonic increasing only.
- `snapshot/by_height` immutable once written.
- `baseline/active` may advance, never regress.
- `inbox/cursor` monotonic increasing only.
- `notify/counter` best-effort latency signal only; not correctness-critical.

### 3) CAS implementation (object store + inline small objects)

Storage policy:

- `len(bytes) <= INLINE_THRESHOLD` (default 16 KiB): inline in FDB metadata value.
- `len(bytes) > INLINE_THRESHOLD`: body in object store, metadata in FDB.

Object key convention:

- `cas/<universe>/sha256/<hash_hex>`

CAS write algorithm:

1. Compute `hash = sha256(bytes)` server-side.
2. Check `cas/meta/hash`.
3. If exists, verify metadata sane and return `hash`.
4. Else write object body (if external) and metadata atomically from caller perspective.
5. Never trust caller-provided hash.

CAS read algorithm:

1. Lookup metadata.
2. Read inline bytes or object bytes.
3. Optionally re-hash on read in debug mode / sampled mode.
4. Return bytes.

Integrity invariants:

- CAS metadata and body are immutable.
- Body content must match key hash.
- Duplicate puts are no-op idempotent.

### 4) Journal append/scan semantics

Single writer assumption:

- Only current lease holder appends journal entries for a world.
- All external writers target inbox, never direct journal append.

`append_batch` transaction:

1. Read `head = journal/head`.
2. Verify `head == expected_head`.
3. Write `journal/e/head+1 .. head+n`.
4. Write `journal/head = head+n`.
5. Commit atomically.

Conflict behavior:

- Head mismatch returns typed `Conflict::HeadAdvanced { expected, actual }`.
- Caller retries with refreshed head.

Read behavior:

- `read_range` provides contiguous `(height, bytes)` ordered ascending.
- Missing entry in requested existing range is a hard corruption error.

### 5) Inbox queue semantics (all ingress converges here)

Inbox item union:

- `DomainEventIngress { schema, value_cbor, key? }`
- `ReceiptIngress { intent_hash, effect_kind, adapter_id, payload_cbor, signature }`
- `InboxIngress { inbox_name, payload_cbor, headers? }`
- `TimerFiredIngress { timer_id, payload_cbor }`
- `ControlIngress { cmd, payload_cbor }`

Enqueue transaction:

1. Allocate sortable `seq` (versionstamp or equivalent).
2. Write `inbox/e/<seq> -> item`.
3. Increment `notify/counter`.
4. Commit and return `seq`.

Drain protocol (P2 persistence contract; runtime loop in P3):

1. Read `cursor`.
2. Read next `N` items strictly after cursor.
3. Convert items to canonical journal records (normalization/validation exactly once).
4. Append journal batch at expected head.
5. Advance `cursor` to max drained seq in same transaction boundary as append (or a coupled compare-and-swap transaction if backend needs chunking).

Important:

- Items are not deleted immediately; cursor defines consumed boundary.
- Background compactor can tombstone/delete items `< cursor` after grace period.
- This avoids at-least-once duplication under crash between append and delete.

### 6) Snapshot index and active baseline semantics

Snapshot write sequence:

1. Kernel produces `KernelSnapshot` bytes (canonical CBOR).
2. CAS put snapshot bytes, get `snapshot_hash`.
3. Write `snapshot/by_height/<h> -> SnapshotRecord`.
4. Append `JournalRecord::Snapshot`.
5. Optionally promote `baseline/active` only if receipt-horizon precondition passes.
6. Append `JournalRecord::BaselineSnapshot` on promotion.

`SnapshotRecord` persisted fields:

- `snapshot_ref: hash`
- `height: nat`
- `logical_time_ns: nat`
- `receipt_horizon_height?: nat`
- `manifest_hash: hash` (required for restore root completeness)

Restore algorithm contract:

1. Load `baseline/active`.
2. Load snapshot blob from CAS.
3. Validate snapshot root completeness.
4. Hydrate runtime.
5. Replay journal entries where `height >= baseline.height` in order.
6. Resulting head state must be replay-identical to full replay from genesis.

### 7) Segment export compaction (hot/cold split)

Implement bounded-KV-growth compaction with no semantic changes.

In scope in P2:

- Segment format and index contract.
- Export and restore path support.
- Safe deletion of compacted hot keys only after index is committed.

Out of scope in P2:

- Full retention planner with tenant-specific policies.

Segment model:

- Object key: `segments/<universe>/<world>/<start>-<end>.log`
- Payload: length-prefixed canonical CBOR entries in strict height order.
- FDB index: `u/<u>/segments/<world>/<end_height> -> { start, end, object_key, checksum }`

Compaction safety rules:

- Compact only ranges strictly older than active baseline (plus configurable margin).
- Ensure all heights in range are materialized in segment before deleting KV entries.
- Keep idempotent compaction markers to survive crash/retry.

Restore with segments:

1. Load active baseline snapshot.
2. Replay required segments in order (if baseline points below segment horizon).
3. Replay remaining hot journal tail from FDB.

### 8) Storage conformance harness

Add a reusable backend conformance test suite:

- package: `crates/aos-persistence/tests/conformance.rs`
- runs against:
  - in-memory reference backend (required in CI)
  - FDB backend (CI optional, required in nightly/integration pipeline)

Conformance cases:

1. CAS put/get/has idempotency and hash correctness.
2. Journal append_batch conflict semantics.
3. Inbox monotonic order with concurrent writers.
4. Cursor monotonicity and no duplication under simulated crash.
5. Snapshot index monotonicity and baseline non-regression.
6. Segment export + restore equivalence.

### 9) Repository touch points

Expected implementation touch points:

- New: `crates/aos-persistence/` (traits/types/conformance harness)
- New: `crates/aos-persistence-fdb/` (FDB implementation)
- New: `crates/aos-persistence-objstore/` (S3/GCS/R2 abstraction if separated)
- Update: `crates/aos-host/` to consume persistence traits in hosted mode
- Update: `crates/aos-kernel/` only via narrow boundary (no FDB/object-store coupling)
- Update: docs/spec alignment in `spec/02-architecture.md` and infra notes

## Transaction Protocols (Normative)

### Protocol A: `cas_put_verified(universe, bytes)`

1. Compute SHA-256 hash.
2. Read `cas/meta/hash`.
3. If found, return hash.
4. Else write blob body (if external) and metadata.
5. Commit.

Guarantees:

- Deterministic returned hash.
- Safe under retries.

### Protocol B: `append_batch(world, expected_head, entries[])`

1. Read `journal/head`.
2. Compare with expected.
3. Write contiguous entries and new head.
4. Commit.

Guarantees:

- Contiguous heights for each batch.
- No partial batch visibility.

### Protocol C: `enqueue_inbox(world, item)`

1. Allocate sortable seq.
2. Write item key.
3. Bump notify counter.
4. Commit.

Guarantees:

- Total order of inbox records per world.
- Multi-writer safe.

### Protocol D: `drain_inbox_to_journal(world, batch_size)`

1. Read `inbox/cursor` and `journal/head`.
2. Read next `batch_size` inbox items.
3. Materialize canonical journal records.
4. Append batch at expected head.
5. Advance `inbox/cursor` to drained max seq.
6. Commit.

Guarantees:

- Each ingressed item appended at most once.
- No loss under crash/retry.

### Protocol E: `snapshot_commit(world, snapshot_bytes, height, promote_baseline)`

1. CAS put snapshot bytes.
2. Write `snapshot/by_height/height`.
3. Append `Snapshot` journal record.
4. If `promote_baseline` and receipt horizon passes:
   - write `baseline/active`
   - append `BaselineSnapshot` journal record
5. Commit.

Guarantees:

- Snapshot index/journal coherence.
- Baseline promotion safety fence.

### Protocol F: `segment_export(world, [h0..h1])`

1. Verify range safe against active baseline/margin.
2. Stream entries to segment object and checksum.
3. Write segment index record.
4. Delete hot `journal/e/<h0..h1>` keys in chunks.
5. Commit each chunk with resumable checkpointing.

Guarantees:

- Recoverable compaction if interrupted.
- Replay-equivalent journal view.

## Testing and Validation

### Determinism and replay

1. Baseline + segment + hot tail restore equals full replay byte-for-byte.
2. Crash injected at each protocol step preserves correctness after retry.
3. Inbox drain protocol produces stable journal ordering across runs.

### Failure-mode tests

1. Object store write succeeds but metadata commit fails: retry must converge.
2. Metadata commit succeeds but read path sees missing object: hard error, no silent heal.
3. Head conflict under concurrent appends returns typed conflict.
4. Cursor regression attempt is rejected.

### Scale tests (target envelope)

1. 10k worlds, sparse activity: metadata reads/writes remain bounded.
2. 1k active worlds with sustained inbox ingress: no per-world starvation.
3. Segment export under sustained appends does not stall foreground writes.

## Deliverables / DoD

1. Persistence contracts crate merged with in-memory backend and conformance tests.
2. FDB/object-store-backed implementation supports CAS/journal/inbox/snapshot/segment index.
3. Hosted-mode restore uses active baseline + segments + hot tail deterministically.
4. Inbox drain protocol with cursor commit is implemented and crash-safe.
5. Segment export compaction is available behind config flag and tested.
6. Kernel remains backend-agnostic; hosted persistence coupling is outside `aos-kernel`.
7. Spec/docs updates merged for hosted persistence contracts.

## Explicitly Out of Scope

- Orchestrator assignment strategy and leases execution loop (P3).
- Effect/timer worker scheduling and retries (P3).
- Fabric adapter and cross-world semantics (P3).
- Automated mark-and-sweep deletion execution.
- Quota/billing enforcement.

