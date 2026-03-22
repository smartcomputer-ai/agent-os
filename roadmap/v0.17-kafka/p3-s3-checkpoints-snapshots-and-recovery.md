# P3: S3 Checkpoints, Snapshots, and Recovery

**Priority**: P3  
**Effort**: High  
**Risk if deferred**: High (Kafka work will either replay too much forever or smuggle mutable state elsewhere)  
**Status**: Completed (blobstore-backed checkpoints/recovery plus packed CAS and retained checkpoint manifests)

## Goal

Define how S3-compatible object storage is used for:

- immutable blobs
- snapshots
- partition checkpoints
- artifacts and large logs

and define the recovery and replay model that goes with the Kafka-first runtime.

Implementation note:

- the current hosted runtime now uses an object-store-shaped blobstore seam rather than being
  hard-wired directly to the AWS SDK

## Completed In Code

Implemented on the experimental branch:

1. `PartitionCheckpoint`, `WorldCheckpointRef`, and `PromotableBaselineRef` shapes.
2. Committed checkpoint publication in the embedded runtime.
3. Recovery as `checkpoint + authoritative journal tail replay`.
4. Replay validation using contiguous per-world sequence numbers.
5. Real blobstore-backed publication of world registry, checkpoint manifests, and referenced blobs
   in `aos-node-hosted`.
6. Hosted restart recovery from blobstore checkpoints plus Kafka route/journal replay.
7. Packed-CAS metadata with direct or packed blob layout records.
8. Range-resolved reads from pack objects during blob restore.
9. Retained checkpoint-manifest history with pruning of older manifests per partition.

Still worth follow-on hardening:

1. Kafka-retention-to-checkpoint operational enforcement.
2. Broader object-store lifecycle policy such as blob GC across retained manifests and
   compaction-safe long-term object rules.

## Design Stance

### 1) Kafka is the authoritative hot log

S3 checkpoints are recovery accelerators, not the primary truth source.

### 2) S3 holds immutable state and large outputs

This includes:

- manifests
- modules
- world snapshots
- partition checkpoint manifests
- artifacts
- large logs
- bulky effect/fabric payloads

The blob plane should be treated as a logical CAS, not as a naive "one S3 object per logical
blob" design.

Required stance:

- logical blob refs remain content hashes of the unpacked bytes
- physical storage may be direct immutable objects or immutable packed objects
- small blobs should usually be packed; large blobs may remain direct
- the authoritative metadata for resolving a logical blob ref to its physical layout belongs to
  the CAS itself
- recovery manifests may reference logical blob refs, but they must not be the only way to find a
  packed blob

Suggested root shape:

```text
CasRootRecord {
  logical_hash,
  size_bytes,
  layout
}

DirectLayout {
  object_ref,
  codec?
}

PackedLayout {
  pack_ref,
  offset,
  stored_len,
  codec?
}
```

The important idea is that the runtime still reads `logical_hash -> bytes`. Packing is hidden
behind authoritative CAS metadata and cache layers.

### 3) Drop journal segment compaction from the new core direction

The new design does not depend on:

- hot/cold journal segmentation in the old FDB sense
- per-world KV log compaction

Instead:

- Kafka retention covers the hot operational replay window
- S3 checkpoints bound replay work

## Checkpoint Model

The first implementation should prefer partition-scoped checkpoints.

Suggested shape:

```text
PartitionCheckpoint {
  journal_topic,
  partition,
  route_epoch,
  journal_offset,
  created_at_ns,
  worlds: list<WorldCheckpointRef>
}

WorldCheckpointRef {
  universe_id,
  world_id,
  baseline: PromotableBaselineRef,
  world_seq,
  metadata?
}

PromotableBaselineRef {
  snapshot_ref,
  snapshot_manifest_ref?,
  manifest_hash,
  height,
  logical_time_ns,
  receipt_horizon_height
}
```

Important stance:

- the checkpoint manifest is partition-scoped
- individual world snapshots remain individually addressable blobs
- recovery should reference a promotable baseline, not just a bare snapshot blob
- the baseline object must carry, or point to, the metadata needed to validate restore
  correctness
- the semantic recovery root is `promotable baseline + authoritative journal offset`
- per-world `world_seq` should be treated as checkpoint-critical metadata, not optional decoration
- input consumer offsets are operational resume hints, not the semantic restore root

For keyed workflows, the checkpoint contract should preserve the current cells model:

- checkpoints persist the snapshot/baseline metadata and per-workflow `CellIndex` roots that anchor
  keyed state
- checkpoints do not need to persist a fully materialized hot cell cache
- hot clean-cell caches and dirty delta residency are derived runtime state rebuilt from
  `baseline + journal replay`

## Committed Checkpoint Publication

Recovery must distinguish between:

- immutable checkpoint contents being uploaded
- a checkpoint becoming visible as the committed recovery root for a shard

Required rule:

- checkpoint data objects and referenced world snapshot/baseline blobs are immutable
- a checkpoint becomes recovery-visible only when a final committed checkpoint manifest/pointer is
  published
- that committed manifest must name the covered journal topic/partition/offset and the complete set
  of checkpoint contents needed for restore
- recovery selects the latest committed checkpoint for a shard, not "the newest object that happens
  to exist in S3"
- partial uploads without a committed manifest are ignored during recovery

## Baseline Semantics

The checkpoint contract must preserve the existing AgentOS restore semantics.

Required stance:

- the active baseline snapshot is the semantic restore root, not just an optimization
- root completeness is non-negotiable
- baseline promotion remains fenced by `receipt_horizon_height`
- the first implementation should treat `receipt_horizon_height == height` as the promotable
  baseline shape unless and until a broader rule is specified
- for keyed workflows, snapshot/checkpoint materialization must flush pending dirty cell deltas into
  new `CellIndex` roots before publishing the promotable baseline

This means a checkpoint entry for a world should either:

- embed the required baseline metadata directly, or
- reference a richer snapshot manifest that proves root completeness and carries the required
  baseline fields

The important point is that recovery must not be underspecified at the baseline boundary.

## Recovery Algorithm

For each assigned partition:

1. load the latest committed partition checkpoint from S3
2. validate and restore all referenced promotable baselines into warm memory
3. seek `aos-journal` to `checkpoint.journal_offset + 1` in `read_committed` mode
4. replay `WorldLogFrame`s forward in partition order and validate contiguous `world_seq`
5. resume steady-state submission processing from `aos-ingress`

For keyed workflows this means:

- restore the persisted `CellIndex` roots as the base layer
- do not force eager loading of every cell at worker startup
- let replay and subsequent reads warm the shard-local cell cache lazily

If no checkpoint exists:

- bootstrap from empty/default partition state and replay from the configured `aos-journal` start
  point

## Blob Read Path

Normal blob retrieval should not depend on recovery-specific bundle manifests.

Required stance:

- resolve the logical blob ref through authoritative CAS metadata
- fetch either a direct object or a byte range from an immutable pack object
- verify the recovered bytes against the logical content hash
- populate local/shared caches so repeated logical reads avoid remote latency

This keeps the logical CAS contract stable while allowing S3 request count to stay reasonable for
many small blobs.

## Replay Budget

The system must define an explicit replay budget.

The roadmap stance should be:

- checkpoint frequency is an operational knob
- replay work is allowed, but not unbounded by accident
- checkpoint cadence should be chosen so common restart/rebalance paths are acceptable

## Kafka Retention Contract

Kafka retention must be explicitly linked to checkpoint policy.

Required safety rule:

- Kafka retention must exceed the maximum checkpoint age plus recovery margin

This must be documented as an invariant, not left as an operator guess.

## Snapshots

World snapshots remain first-class and deterministic.

The difference from the old hosted model is:

- snapshots are primarily used for fast recovery and migration in a log-first system
- they are no longer tied to a separate transactional journal/inbox persistence plane

But:

- snapshot restore semantics remain strict
- checkpoints must preserve the metadata needed to prove a snapshot is a valid active baseline
- replay-or-die remains the correctness standard after `baseline + Kafka tail` recovery
- snapshot/checkpoint creation remains the point where pending cell deltas are materialized into new
  persisted `CellIndex` roots; steady-state Kafka commits should not do that work eagerly

## S3 Naming And Layout

The exact key layout can vary, but the repository should expect S3 objects for:

- `cas/root/...`
- `cas/direct/...`
- `cas/pack/...`
- `snapshots/...`
- `checkpoints/...`
- `artifacts/...`
- `logs/...`

The path design should remain universe-aware and world-aware where appropriate.

## Large Payload Rule

Kafka records should stay small and semantic.

Therefore:

- large host logs go to S3
- large effect results go to S3
- large artifact bundles go to S3
- snapshots and checkpoint bundles go to S3

Kafka records carry refs and metadata.

This should not be read as "all small CAS content is modeled as ad hoc recovery bundles".
Checkpoint and snapshot manifests may still group related roots, but packed CAS is part of the
normal blob-storage design.

## Out of Scope

1. Multi-region replication policy.
2. Rich storage lifecycle/tiering policy.
3. Full snapshot scheduling heuristics.
4. A full artifact product surface.

## DoD

1. The roadmap defines S3 as the checkpoint/snapshot/blob plane.
2. The recovery algorithm is stated in terms of `checkpoint -> Kafka replay`.
3. Per-partition checkpoints are the first declared implementation target.
4. Kafka retention and checkpoint cadence are tied together as explicit invariants.
5. Checkpoints reference promotable baselines rather than underspecified bare snapshot refs.
6. The new direction explicitly drops old journal-segment compaction from the core design.
7. The roadmap states that BlobPlane is a logical CAS whose physical layout may be direct or
   packed, resolved by authoritative CAS metadata rather than recovery-only manifests.
8. The roadmap defines a committed-checkpoint publication rule so recovery never guesses from
   partial S3 uploads.
