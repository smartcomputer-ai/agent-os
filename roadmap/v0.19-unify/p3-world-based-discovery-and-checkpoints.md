# P3: World-Based Discovery and Checkpoints

**Priority**: P3  
**Effort**: Large  
**Risk if deferred**: High (the runtime still carries partition-shaped checkpoint/discovery
semantics even after ingress and ownership were simplified)  
**Status**: Implemented  
**Depends on**:
- `roadmap/v0.19-unify/directive.md`
- `roadmap/v0.19-unify/p2-direct-http-ingress-and-explicit-ownership.md`
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`

## Goal

Define the third implementation phase for `v0.19` around one clear follow-up to P2:

1. replace `PartitionCheckpoint` as the primary checkpoint model,
2. make world discovery come from explicit world inventory rather than journal partitions,
3. keep journal partitioning, if still present, as backend storage behavior only,
4. preserve the hosted slice/flush/async execution model introduced and retained through P2.

This phase is not yet about switchable journal backends.
It is about removing the remaining partition-centered control plane from discovery, replay, and
checkpoint persistence.

## Why This Exists

P2 removed Kafka ingress and assignment-driven ownership.

That was necessary, but it did not complete the architectural simplification described in
`directive.md` phase 3.

Before P3 work started, the runtime had a mixed model:

1. worlds are owned explicitly via config/runtime state,
2. but checkpoints are still persisted as partition aggregates,
3. replay still consults partition checkpoints,
4. blobstore layout still keys checkpoints by `journal_topic/partition`,
5. runtime/control still exposes partition checkpoint operations.

So the execution model is clearer than before, but the persistence/discovery model is still shaped
by partitions.

## Current State In Code

P2 already put useful pieces in place:

1. explicit owned worlds exist in [config.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/config.rs:35),
   [runtime.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/runtime.rs:795), and
   [worlds.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/worlds.rs:28)
2. per-world checkpoint references already exist as `WorldCheckpointRef` in
   [log.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/log.rs:133)
3. the worker already reopens worlds using world checkpoint refs when available in
   [worlds.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/worlds.rs:653)
4. journal and checkpoint seams already exist in
   [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:80)

Implemented so far:

1. `CheckpointBackend` is world-first in
   [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:80)
2. blobstore persists world-keyed checkpoints and exposes world inventory in
   [blobstore/mod.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/infra/blobstore/mod.rs:1)
3. replay/bootstrap/meta use world checkpoints rather than partition checkpoint aggregates in
   [replay.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/services/replay.rs:218),
   [runtime.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/runtime.rs:1366), and
   [bootstrap.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/bootstrap.rs:157)
4. hosted worker checkpoint staging/publication is world-based in
   [checkpoint.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/checkpoint.rs:10) and
   [journal.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/journal.rs:297)
5. the active hosted runtime path no longer exposes partition checkpoint operations
6. Kafka-backed replay/open now uses world checkpoint `journal_cursor` metadata to request
   world tail frames rather than rebuilding from full per-world history in
   [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:98),
   [infra/kafka/mod.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/infra/kafka/mod.rs:135),
   [services/replay.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/services/replay.rs:198),
   and [worlds.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/worlds.rs:553)

Remaining partition-shaped surfaces are now intentionally non-primary:

1. debug/inspection paths still expose journal partitions explicitly, which is acceptable for now
   because they are journal-facing diagnostics rather than recovery/discovery identity
2. the remaining Kafka journal coverage lives in the backend-focused
   `kafka_broker_backend_e2e` suite; the stale partition-shaped `kafka_broker_e2e` suite has been
   removed
3. backend internals still use `partition_for_world(...)` and partition logs to implement Kafka
   journal placement and cursor-based tail replay, which is expected for the current journal
   backend

## Design Stance

### 1) Preserve the worker model from P2

P3 should not redesign the hosted worker core.

Keep:

1. per-world serialized execution,
2. speculative `CompletedSlice` staging,
3. durable flush before post-commit publication,
4. rollback/reopen on failed flush,
5. async/opened-effect publication only after durable flush.

The change is in discovery and checkpoint persistence, not scheduler semantics.

### 2) World is the primary recovery/discovery unit

The primary unit for persisted recovery metadata should be `world_id`, not partition.

That means:

1. the checkpoint object persisted to blobstore should be a world checkpoint record,
2. the inventory of worlds should come from config and/or checkpoint storage,
3. replay/open should start from a world checkpoint plus journal tail,
4. partitions may still exist internally for journal placement, but they are not the identity of
   discovery or checkpoints.

### 3) Discovery should be pluggable

P3 should introduce a real discovery/inventory backend.

The intended sources are:

1. static config / CLI-provided world IDs,
2. blobstore checkpoint inventory,
3. later, additional inventory/discovery implementations.

The important point is that discovery is no longer inferred from journal partitions.

### 4) Journal partitioning may remain, but only as backend metadata

If Kafka remains the journal backend for now, `partition_for_world(...)` can still decide where
frames are appended.

But P3 should treat that as:

1. ordering/storage behavior,
2. maybe metadata stored on world checkpoints,
3. never the primary discovery key,
4. never the shape of the checkpoint object itself.

## Required Model Changes

### 1) Replace `PartitionCheckpoint` as the primary checkpoint model

This is complete. The old primary checkpoint type was:

```rust
pub struct PartitionCheckpoint {
    pub journal_topic: String,
    pub partition: u32,
    pub journal_offset: u64,
    pub created_at_ns: u64,
    pub worlds: Vec<WorldCheckpointRef>,
}
```

That structure is no longer the canonical checkpoint object. The active model is world-based:

```rust
pub struct WorldCheckpointRecord {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    pub world_epoch: u64,
    pub checkpointed_at_ns: u64,
    pub baseline: PromotableBaselineRef,
    pub world_seq: u64,
    pub journal_cursor: Option<WorldJournalCursor>,
}
```

The important change is:

1. one persisted record per world checkpoint,
2. world identity is primary,
3. any journal partition/offset information becomes auxiliary backend metadata.

### 2) Refactor `CheckpointBackend` to be world-based

This is complete. The world-oriented operations are now primary:

1. `commit_world_checkpoint(...)`
2. `latest_world_checkpoint(world_id)`
3. `list_world_checkpoints()` or `list_worlds()`

Partition queries are no longer part of the primary checkpoint contract.

### 3) Add a discovery/inventory backend

This is complete. `aos-node` now has a dedicated world inventory contract distinct from journal
and checkpoint operations:

```rust
pub trait WorldInventoryBackend {
    fn list_worlds(&self) -> Result<Vec<WorldId>, BackendError>;
}
```

Hosted runtime composes inventory from:

1. configured `owned_worlds`,
2. checkpoint/blobstore inventory,
3. later dynamic discovery implementations.

## Required Hosted Runtime Changes

### 1) Rework checkpoint staging off partitions

This is complete. The worker now evaluates and stages checkpoints per world:

1. `publish_due_checkpoints()` in
   [checkpoint.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/checkpoint.rs:60)
   should iterate worlds directly,
2. `stage_partition_checkpoint_slice(...)` should become world-based or batch world-based commits
   without using partition as the organizing concept,
3. `CheckpointCommit` should no longer require `partition`.

### 2) Rework checkpoint publication in `journal.rs`

This is complete. `apply_checkpoint_post_commit()` now:

1. publish one checkpoint record per world,
2. update active-world checkpoint metadata from that record,
3. compact world journals based on per-world checkpoint success,
4. avoid any merge step centered on partition aggregates.

### 3) Make bootstrap/activation inventory-driven

This is complete. Startup world activation now comes from:

1. configured owned world IDs,
2. discovered checkpoint inventory from blobstore,
3. optionally both combined.

Bootstrap no longer relies on partition-shaped checkpoint identity as the normal discovery path.

### 4) Simplify replay to world checkpoint only

This is complete. `HostedReplayService` now:

1. load the latest world checkpoint,
2. load journal frames for that world,
3. replay only the tail beyond the checkpoint baseline.

Partition checkpoint lookup is gone from replay.

## Required Blobstore Changes

### 1) Replace partition-keyed checkpoint storage layout

This is complete. Blobstore checkpoint storage is world-keyed, for example:

1. `/checkpoints/worlds/{world_id}/latest.cbor`
2. `/checkpoints/worlds/{world_id}/manifests/...`

If journal metadata still needs to be preserved, store it inside the world checkpoint record rather
than in the object key shape.

### 2) Make listing worlds come from checkpoint storage

This is complete. The blob meta layer supports:

1. listing checkpointed world IDs,
2. loading latest checkpoint by world ID,
3. retaining/pruning checkpoint history per world.

This is the key storage primitive for a blobstore-backed discovery backend.

## Public/API Cleanup Required By P3

This is complete for the active hosted architecture.

Removed:

1. partition checkpoint runtime/control operations on the main hosted path
2. partition checkpoint callbacks from replay/meta/bootstrap wiring

Remaining partition-facing APIs are debug/journal inspection surfaces:

1. [services/kafka_debug.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/services/kafka_debug.rs:1)
   defines `KafkaDebugService` with `partition_count()`, `journal_topic()`,
   `partition_entries()`, and `recover_partition()`
2. [bootstrap.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/bootstrap.rs:98) exposes a
   builder for that debug service from a hosted runtime

These should be treated as debug/journal-inspection APIs, not recovery/discovery semantics.

## Scope

### [x] 1) Replace `PartitionCheckpoint` as the primary checkpoint object

Main intent:

1. define a persisted world checkpoint record in `aos-node`,
2. demote partition metadata to optional backend detail,
3. stop treating partition aggregates as the canonical checkpoint artifact.

### [x] 2) Make `CheckpointBackend` world-based

Main intent:

1. make world checkpoint commit and lookup the primary contract,
2. add world listing / inventory support,
3. remove or demote partition checkpoint APIs.

### [x] 3) Introduce a pluggable world discovery backend

Main intent:

1. inventory comes from config/blobstore/cli,
2. hosted runtime activates worlds from inventory,
3. journal partitions no longer act as a discovery mechanism.

### [x] 4) Rework hosted checkpoint staging and publication to be world-based

Main intent:

1. iterate checkpoint candidates by world,
2. publish one checkpoint record per world,
3. remove partition aggregation from worker checkpoint flow.

### [x] 5) Rework replay/bootstrap to depend on world checkpoints

Main intent:

1. replay uses latest world checkpoint plus world frame tail,
2. bootstrap can load worlds from checkpoint inventory,
3. partition checkpoint lookup disappears from replay logic.

### [x] 6) Change blobstore layout and retention to world-keyed checkpoints

Main intent:

1. store latest/history by world ID,
2. list available worlds from blobstore,
3. prune retained checkpoint manifests per world.

### [x] 7) Remove partition checkpoint runtime/service APIs

Main intent:

1. remove `latest_checkpoint(partition)` and `checkpoint_partition(...)`,
2. remove partition checkpoint callbacks from hosted meta/replay services,
3. keep public APIs world-oriented.

## Non-Goals

P3 does **not** attempt:

1. switchable journal backends,
2. replacing Kafka journal yet,
3. collapsing `aos-node-local` and `aos-node-hosted`,
4. redesigning the worker scheduler,
5. adding durable read semantics beyond P2’s `wait_for_flush`.

## Deliverables

1. A world-based persisted checkpoint model in `aos-node`.
2. A checkpoint backend whose primary operations are world-based.
3. A discovery/inventory backend that can source worlds from config and blobstore.
4. Hosted bootstrap/replay/checkpoint flows that no longer depend on partition checkpoint
   aggregates.
5. Blobstore checkpoint storage keyed by world rather than partition.

## Acceptance Criteria

1. No primary hosted control/replay/runtime path requires `PartitionCheckpoint`.
2. The list of available worlds can be sourced from persisted world checkpoints in blobstore.
3. Hosted world activation and replay can operate from world checkpoint inventory plus journal
   tail.
4. Checkpoint publication writes per-world checkpoint records instead of partition aggregates.
5. Any remaining partition logic is clearly backend storage metadata, not discovery identity.
6. The P2 worker slice/flush/async execution model remains intact.
7. Remaining partition-shaped code, if any, is isolated to debug/inspection surfaces or stale
   compatibility coverage rather than the active hosted architecture.

## Recommended Implementation Order

1. define the world checkpoint model and update `CheckpointBackend` in `aos-node`,
2. change blobstore persistence/layout and add world inventory listing,
3. rework hosted checkpoint commit/staging types off partition shape,
4. rewire `journal.rs` post-commit checkpoint publication,
5. rewire replay/bootstrap/meta services to world-based checkpoint operations,
6. remove partition checkpoint runtime APIs and tighten tests around world-based discovery.
