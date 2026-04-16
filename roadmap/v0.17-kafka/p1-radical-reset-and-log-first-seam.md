# P1: Radical Reset and Log-First Seam

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (future Kafka/S3 work will preserve the wrong hosted shape)  
**Status**: Completed
## Goal

Define the new hosted architecture at the right seam before implementation work begins.

This milestone is about changing the shape of the system, not selecting a different storage
backend under the current shape.

## Completed In Code

Implemented on the experimental branch:

1. New shared planes and core types now exist in `crates/aos-node/src/log_first.rs`.
2. The shared seam is log-first rather than inbox/lease/store shaped.
3. An embedded backend proves the seam with `MemoryLogRuntime` and `MemoryShardWorker`.
4. The seam includes `BlobPlane`, `SubmissionPlane`, `WorldLogPlane`, `RoutePlane`, and
   `CheckpointPlane`.
5. The embedded seam already exercises authoritative frames, route-epoch fencing, checkpoints,
   and replay.
6. `aos-node-hosted` now runs on this seam instead of the old FDB-shaped worker path.
7. Real hosted Kafka and blobstore implementations now plug into the same seam.

Still not completed in this milestone's forward direction:

1. Full `ProjectionPlane` implementation.
2. Any broader read-model or projection strategy beyond the correctness core planes.

Deferred follow-up:

1. External execution seam work moves to `roadmap/v0.18-execution/`.
2. Fabric/session/artifact/log follow-on work moves to `roadmap/v0.19-fabric/`.

## Why This Is A Reset

The existing hosted model treats the world runtime as a set of coordinated mutable structures:

- world lease records
- inbox rows and cursors
- journal heads and append batches
- ready indexes
- pending/inflight effect queues
- timer queues
- durable projection rows

That was a reasonable way to use FoundationDB.

It is the wrong contract for Kafka. Trying to preserve those semantics on top of Kafka would
produce an awkward system that is harder to understand, harder to operate, and less aligned with
the actual event-sourced nature of AgentOS.

Therefore this roadmap takes the stronger stance:

- do not adapt Kafka to the current hosted seam
- replace the seam with one that is AOS-native and log-first

## New Core Planes

The new hosted architecture should be described in terms of a few explicit planes.

### 1) `BlobPlane`

Immutable content and large payloads through a logical CAS:

- manifests
- modules
- snapshots
- artifacts
- logs
- large effect/fabric payloads

Logical blob identity should remain content-addressed. The public contract is still
`logical_hash -> bytes`.

The physical storage layout is allowed to vary:

- direct immutable objects for large blobs
- immutable packed objects for many small blobs
- local or shared caches keyed by logical hash and, optionally, by backing pack/object ref

Important stance:

- packing is a first-class BlobPlane behavior, not just a recovery-only bundle trick
- authoritative CAS metadata must be able to resolve a logical blob ref to either a direct object
  or a packed object locator
- this lookup metadata is part of the BlobPlane contract, not a hidden control-plane database

Default target:

- S3-compatible object storage with local/shared disk caching

### 2) `WorldLogPlane`

The authoritative ordered record stream for worlds.

This includes:

- accepted domain events
- effect intents
- effect receipts
- governance/control-plane proposal, shadow, approval, apply, and manifest transition records
- stream frames and other canonical world-history records
- recovery and lifecycle markers that affect replay semantics

Default target:

- Kafka

Important stance:

- only the current world owner emits authoritative world-log records
- only records on this plane consume `world_seq`
- raw ingress, raw receipts, and admin/control submissions are not authoritative history until the
  owner admits and normalizes them
- the authoritative journal vocabulary should stay intentionally smaller than the admission
  vocabulary and should express the semantic actions actually taken by the runtime and kernel

### 3) `SubmissionPlane`

Short-retention admission queue for externally submitted causes.

This includes:

- external domain ingress
- external effect or fabric receipts
- timer firings from external services
- admin/control submissions
- world bootstrap or create requests

Default target:

- Kafka

Important stance:

- submissions do not advance world state on their own
- submissions do not consume `world_seq`
- the owner converts accepted submissions into canonical world-log records

### 4) `RoutePlane`

The mapping from world identity to its current submission and journal address.

This is table-shaped current-state metadata, not world event history.

It is the decisive source of current placement for a world.

The key abstraction is:

```text
WorldRoute {
  ingress_topic,
  journal_topic,
  partition_override?,
  epoch
}
```

If `partition_override` is absent, the effective partition is derived from `(universe_id,
world_id)` via the Kafka key / stable partition function for the paired topics. If it is present,
it is an explicit manual placement override.

Default target:

- a compacted Kafka topic keyed by `(universe_id, world_id)` and holding one current route record
  for each active world

Important stance:

- routing metadata should live in an explicit `RoutePlane`
- it should not be pushed into S3
- it should not smuggle a hidden transactional metadata database back into the core design

This replaces the old notion that the primary scheduling object is the world lease record.

### 5) `CheckpointPlane`

Recovery metadata that says:

- what partition state has been snapshotted
- which world snapshots belong to it
- what authoritative journal offset is covered

Default target:

- S3

### 6) `ProjectionPlane`

Optional read-optimized materializations and indexes.

Important stance:

- projections are not part of the correctness core
- the system must still be correct if projections are missing or stale
- "not authoritative" does not mean "not needed": hosted AOS still needs a minimal operator/read
  surface for things like world state, manifest/head visibility, governance status, and route
  visibility

### 7) `FabricPlane`

Optional execution lanes for external work:

- specialized effect workers
- host/container/vm/sandbox execution
- artifact collection
- deployment/monitoring flows

This plane may also use Kafka, but it is subordinate to the authoritative world log.

## Core Design Stances

### 1) Kafka is the truth, not a side queue

The authoritative hot state transition history lives in Kafka.

Route metadata may also live in Kafka, but in a compacted current-state topic rather than the
authoritative world-history stream.

### 2) S3 is the recovery and artifact plane, not the authoritative mutable control database

S3 holds:

- immutable blobs
- snapshots
- checkpoints
- large logs and artifacts

It is not the hot coordination substrate.

Blob-plane lookup metadata that resolves a logical hash to a direct or packed immutable object is
part of the storage contract, not hot mutable coordination state.

### 3) The unit of scheduling is the partition, not the world

This is the central simplification.

Workers own Kafka partitions. Worlds live inside partitions.

### 4) World state only advances from records on the authoritative world log

External workers may execute work, but they never directly mutate world state.

Submissions and dispatch lanes are admission and execution plumbing, not replay truth.

### 5) We accept losing some fine-grained hosted behavior

The new architecture should optimize for:

- simplicity
- scalability
- operator fit
- correctness clarity

before optimizing for:

- arbitrary per-world placement
- per-world independent mobility
- rich synchronous point-read semantics in the hot path

## Scope

### In scope

1. Define the new planes and their responsibilities.
2. Declare the old hosted seam non-target for the future direction.
3. Define partition ownership as the primary runtime ownership model.
4. Define authoritative-log semantics as the core state transition rule.
5. Define projections and specialized worker lanes as derived or subordinate systems.
6. Call out that hosted read/operator surfaces still exist even when projections are derived and
   non-authoritative.

### Out of scope

1. Backport Kafka support into `aos-fdb`.
2. Preserve the current hosted trait surface for the future hosted runtime.
3. Implement a full control-plane product in this document.
4. Decide every crate name and package split before the runtime model is settled.

## Expected Repository Consequences

This reset likely implies:

- `aos-fdb` becomes historical prototype code, not the future hosted core
- `aos-node-hosted` will need a major redesign or replacement
- the hosted traits in `aos-node` will need to be rewritten around log/checkpoint/fabric
  semantics
- `aos-node-local` and `aos-sqlite` become transitional implementations that should converge on
  the new model

## DoD

1. The roadmap clearly states that `v0.17-kafka` is a hosted-architecture reset, not a backend
   swap.
2. The new hosted architecture is defined in terms of log/checkpoint/blob/route/fabric planes.
3. The future implementation target is partition-owned, log-first runtime semantics.
4. Follow-on roadmap items can refer to these planes directly instead of the old hosted
   lease/inbox/journal-head model.
