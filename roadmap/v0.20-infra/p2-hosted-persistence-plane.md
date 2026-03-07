# P2: Hosted Persistence Plane (CAS, Journal, Inbox, Snapshot, Compaction)

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (runtime-plane work will bake wrong persistence semantics)  
**Status**: In Progress

Implementation status as of 2026-03-07:

- Complete in `crates/aos-fdb`: scope `1)` FDB-first storage boundary, `2)` canonical keyspace layout, `3)` CAS, `4)` journal append/scan semantics, `5)` inbox queue semantics.
- Partial: targeted live FoundationDB integration coverage exists under `crates/aos-fdb/tests/`, but the full reusable conformance harness from scope `8)` is not finished.
- Pending: scope `6)` snapshot/baseline integration into hosted restore flow, scope `7)` segment export compaction, scope `8)` formal conformance harness, scope `9)` broader repository integration and docs/spec alignment.

## Goal

Ship a production-grade hosted persistence plane for universes/worlds that preserves existing AgentOS invariants while enabling movable worlds.

Design stance for P2:

- FoundationDB is the primary and only production metadata/ordering store target in this milestone.
- We do not introduce a "portable multi-DB" abstraction layer.
- We do keep a narrow operation boundary between host runtime and storage implementation so kernel/runtime logic is not coupled to raw FDB client APIs.
- That boundary should remain suitable for a later first-party embedded backend for isolated local universes, but that backend is not implemented in P2.
- P2 does not attempt live hosted/embedded communication or two-way sync; movement between modes is deferred to later export/import work.

Core outcomes:

1. Shared universe-scoped immutable CAS.
2. Shared world journal with deterministic append ordering.
3. Shared durable world inbox queue for all ingress.
4. Shared snapshot and baseline index with deterministic restore (`baseline + tail replay`).
5. Bounded hot-state growth via segment export (hot tail in KV, cold segments in object storage).

This milestone is the persistence substrate only. It does not include scheduling/orchestration policy (P3).

## Dependency

- Requires `v0.20-infra/p1-protocol-roots-and-baselines.md` merged.
- P2 assumes:
  - `BlobEdge` and `blob.put@1` normalization are active.
  - Baseline promotion checks are active.
  - Snapshot root completeness checks are active.
  - World creation guarantees `active_baseline`.

## Non-Goals (P2)

- Multi-worker scheduling/orchestrator decisions (P3).
- Cross-world `fabric.send` adapter (P3).
- Timer worker and global effect worker pools (P3).
- Embedded-universe mode implementation (follow-on milestone).
- Live communication or shared CAS between embedded and hosted universes.
- Mark-and-sweep deletion execution and retention policy UI.
- Multi-region replication strategy.
- A backend-portable storage layer across unrelated databases.

## Scope (Now)

### [x] 1) FDB-first storage boundary (freeze protocol first)

Add a concrete FDB-focused persistence implementation crate with deterministic semantics and a narrow operation surface for host/runtime integration:

- Suggested crate: `crates/aos-fdb` (name can vary, FDB-first stance cannot).
- Provide canonical types and operations for:
  - CAS
  - World journal
  - World inbox
  - Snapshot index + active baseline
  - Segment index (for cold log materialization)

Boundary design constraints:

- This is a runtime/storage protocol boundary, not a promise of arbitrary backend portability.
- Public protocol types should not expose raw FoundationDB client types directly, even if the FDB implementation uses them internally.
- The boundary should be reusable later by a first-party embedded backend for isolated local universes.
- World runtime integration should key off `(universe_id, world_id)`, not a filesystem world root.

Suggested core types:

- `UniverseId = Uuid`
- `WorldId = Uuid`
- `JournalHeight = u64`
- `InboxSeq = opaque, serializable, totally ordered cursor token` (FDB may encode this as versionstamp bytes internally)
- `SegmentId = { start: u64, end: u64 }`

Suggested operation surface (illustrative):

```rust
pub trait WorldPersistence {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;
    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError>;
    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: u64,
        entries: &[Vec<u8>],
    ) -> Result<u64, PersistError>; // returns first height

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: u64,
        limit: u32,
    ) -> Result<Vec<(u64, Vec<u8>)>, PersistError>;

    fn journal_head(&self, universe: UniverseId, world: WorldId) -> Result<u64, PersistError>;

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError>;

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError>;

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError>;

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError>;

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError>;
}
```

Boundary requirements:

- All writes are idempotent where logically expected (`cas.put` by hash, snapshot index at height).
- All read/write paths return typed errors (`conflict`, `not_found`, `validation`, `backend`).
- Contract is deterministic with respect to ordering and normalization boundaries.

FDB semantic leakage is accepted and explicit:

- optimistic conflict and retry loops are first-class (`expected_head` mismatch, cursor CAS failure),
- queue ordering is versionstamp-driven,
- transaction chunking limits shape large batch behavior,
- monotonic cursor/baseline constraints are enforced with compare-and-swap semantics.

However, these remain implementation semantics of the FDB backend, not stable public protocol shapes for the runtime boundary.

### [x] 2) FDB keyspace layout (authoritative metadata + ordering + queues)

Define canonical tuple-subspace layout (logical names; exact tuple encoding is implementation detail).

Universe global:

- `u/<u>/cas/meta/<hash> -> { size, storage, object_key?, inline_bytes? }`
- `u/<u>/effects/pending/<shard>/<seq> -> EffectDispatchItem` (written in P2; consumed in P3)
- `u/<u>/effects/inflight/<shard>/<seq> -> EffectInFlightItem` (written in P2; consumed in P3)
- `u/<u>/effects/dedupe/<intent_hash> -> DispatchStatus` (written in P2; consumed in P3)
- `u/<u>/effects/dedupe_gc/<gc_bucket>/<intent_hash> -> ()`
- `u/<u>/timers/due/<shard>/<time_bucket>/<deliver_at_ns>/<intent_hash> -> TimerDueItem` (written in P2; consumed in P3)
- `u/<u>/timers/inflight/<shard>/<intent_hash> -> TimerClaim` (written in P2; consumed in P3)
- `u/<u>/timers/dedupe/<intent_hash> -> DeliveredStatus` (written in P2; consumed in P3)
- `u/<u>/timers/dedupe_gc/<gc_bucket>/<intent_hash> -> ()`
- `u/<u>/segments/<world>/<end_height> -> SegmentIndexRecord`

Per world:

- `u/<u>/w/<w>/meta -> WorldMeta`
- `u/<u>/w/<w>/journal/head -> u64`
- `u/<u>/w/<w>/journal/e/<height> -> canonical_cbor_journal_entry`
- `u/<u>/w/<w>/snapshot/by_height/<height> -> SnapshotRecord`
- `u/<u>/w/<w>/baseline/active -> SnapshotRecord`
- `u/<u>/w/<w>/inbox/e/<seq> -> InboxItem`
- `u/<u>/w/<w>/inbox/cursor -> Option<seq>`
- `u/<u>/w/<w>/notify/counter -> u64` (atomic-add hint only)
- `u/<u>/w/<w>/gc/pin/<hash> -> PinReason` (optional, if explicit pin-index is materialized)

Rules:

- `journal/head` monotonic increasing only.
- `snapshot/by_height` immutable once written.
- `baseline/active` may advance, never regress.
- `inbox/cursor` monotonic increasing only.
- global effect and timer queues are sharded; there is no universe-wide total order across shards.
- `shard` should be derived from a stable hash (normally `intent_hash`) with a fixed configured shard count.
- timer queues are additionally bucketed by time to keep scans bounded.
- `notify/counter` uses atomic add and is a best-effort latency signal only; not correctness-critical.
- watches on `notify/counter` are optional wakeup hints only; correctness comes from scanning durable state.
- queue and inbox values must stay small; large payloads are externalized to CAS and referenced from queue records.

Initial rollout note:

- The shard dimension is part of the keyspace from the start, but the first implementation may run with `shard_count = 1`.
- This keeps the initial runtime simple while avoiding a later keyspace migration for global hot queues.

Recurring schedule note:

- `P2` only defines durable one-shot timer storage for `timer.set`.
- First-class recurring schedules (`schedule.upsert`, `schedule.cancel`, cron/interval metadata, misfire policy) are intentionally deferred until after the first hosted infra pass.
- Future recurring schedules should compile down to the same durable timer substrate by materializing the next one-shot due item into `u/<u>/timers/due/...`; `P2` should not require a timer keyspace redesign for that later step.

### [x] 3) CAS implementation (object store + inline small objects)

Storage policy:

- `len(bytes) <= INLINE_THRESHOLD` (default 4 KiB): inline in FDB metadata value.
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
- Inline metadata values should remain comfortably within normal FDB value-size guidance; large bodies belong in object storage.

### [x] 4) Journal append/scan semantics

Single writer assumption:

- Only current lease holder appends journal entries for a world.
- All external writers target inbox, never direct journal append.

`append_batch` transaction:

1. Read `head = journal/head`.
2. Verify `head == expected_head`.
3. Write `journal/e/head+1 .. head+n`.
4. Write `journal/head = head+n`.
5. Commit atomically.

Batch sizing constraints:

- Journal appends must be bounded by bytes as well as entry count.
- Hosted runtime should target transactions well below FoundationDB's 1 MiB "red flag" affected-data threshold and never approach the hard 10 MB limit.
- Retry loops must assume FoundationDB's five-second transaction lifetime.

Conflict behavior:

- Head mismatch returns typed `Conflict::HeadAdvanced { expected, actual }`.
- Caller retries with refreshed head.

Read behavior:

- `read_range` provides contiguous `(height, bytes)` ordered ascending.
- Missing entry in requested existing range is a hard corruption error.

### [x] 5) Inbox queue semantics (all ingress converges here)

Inbox item union:

- `DomainEventIngress { schema, value_cbor, key? }`
- `ReceiptIngress { intent_hash, effect_kind, adapter_id, payload_cbor, signature }`
- `InboxIngress { inbox_name, payload_cbor, headers? }`
- `TimerFiredIngress { timer_id, payload_cbor }`
- `ControlIngress { cmd, payload_cbor }`

Payload sizing rule:

- Any large `*_cbor` field should be externalized to CAS above a queue payload threshold and represented by companion `{ *_ref, *_size, *_sha256 }` metadata instead of large inline values.

Enqueue transaction:

1. Allocate sortable `seq` (versionstamp or equivalent).
2. Write `inbox/e/<seq> -> item`.
3. Increment `notify/counter`.
4. Commit and return `seq`.

Drain protocol (P2 storage protocol; runtime loop in P3):

1. Read `cursor`.
2. Read next `N` items strictly after cursor.
3. Convert items to canonical journal records (normalization/validation exactly once).
4. Append journal batch at expected head.
5. Advance `cursor` to max drained seq in same transaction boundary as append (or a coupled compare-and-swap transaction if backend needs chunking).

Important:

- Items are not deleted immediately; cursor defines consumed boundary.
- Background compactor can tombstone/delete items `< cursor` after grace period.
- This avoids at-least-once duplication under crash between append and delete.

### [ ] 6) Snapshot index and active baseline semantics

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

### [ ] 7) Segment export compaction (hot/cold split)

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

### [ ] 8) Storage protocol conformance harness

Current note:

- We do have targeted live FoundationDB integration tests for CAS, journal, inbox, and a broader end-to-end smoke test under `crates/aos-fdb/tests/`.
- We do not yet have the single reusable `tests/conformance.rs` harness that runs the same protocol contract across multiple backends.

Add a reusable protocol conformance test suite:

- package: `crates/aos-fdb/tests/conformance.rs`
- runs against:
  - FDB implementation (required in integration/nightly)
  - in-memory behavioral reference implementation used for CI/unit tests (not a portability target)

The harness should be structured so a later first-party embedded backend can be added without redefining the protocol.

Conformance cases:

1. CAS put/get/has idempotency and hash correctness.
2. Journal append_batch conflict semantics.
3. Inbox monotonic order with concurrent writers.
4. Cursor monotonicity and no duplication under simulated crash.
5. Snapshot index monotonicity and baseline non-regression.
6. Segment export + restore equivalence.
7. Sharded effect/timer queue scans preserve correctness under concurrent producers.

### [ ] 9) Repository touch points

Expected implementation touch points:

- New: `crates/aos-fdb/` (FDB-first persistence implementation + protocol types + conformance harness)
- Optional New: `crates/aos-fdb-objstore/` (object-store helper if split for operational concerns, but I prefer it to be part of aos-fdb to avoid too many crates unless there is a very good reason for this.)
- Update: `crates/aos-host/` to consume `aos-fdb` operations in hosted mode
- Update: `crates/aos-host/` startup paths to open hosted worlds by persistence identity rather than assuming a filesystem world root is the runtime authority
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
5. Sharded pending/timer queues continue operating when one shard is hot or one shard reaper crashes.

### Scale tests (target envelope)

1. 10k worlds, sparse activity: metadata reads/writes remain bounded.
2. 1k active worlds with sustained inbox ingress: no per-world starvation.
3. Segment export under sustained appends does not stall foreground writes.
4. Global effect/timer dispatch throughput scales with shard count rather than a single hot range.

## Deliverables / DoD

1. [~] `aos-fdb` protocol/types merged with targeted integration tests and in-memory behavioral reference backend. Remaining gap: the formal reusable conformance harness is not finished.
2. [x] `aos-fdb` FDB/object-store-backed implementation supports CAS/journal/inbox/snapshot/segment index.
3. [ ] Hosted-mode restore uses active baseline + segments + hot tail deterministically.
4. [x] Inbox drain protocol with cursor commit is implemented and crash-safe.
5. [ ] Segment export compaction is available behind config flag and tested.
6. [x] Kernel remains backend-agnostic; hosted persistence coupling is outside `aos-kernel`.
7. [ ] Spec/docs updates merged for the FDB-first hosted persistence protocol.

## Explicitly Out of Scope

- Orchestrator assignment strategy and leases execution loop (P3).
- Effect/timer worker scheduling and retries (P3).
- Fabric adapter and cross-world semantics (P3).
- Embedded-universe runtime implementation and hosted/embedded bridge semantics.
- Export/import movement between embedded and hosted modes.
- Automated mark-and-sweep deletion execution.
- Quota/billing enforcement.
