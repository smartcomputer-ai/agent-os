# v0.17 Kafka/S3 Reset

## Background

The current hosted direction under `v0.14-infra` and the concrete implementation in
`crates/aos-fdb` / `crates/aos-node-hosted` proved that movable hosted worlds are
implementable, but it also hardened a storage/control seam that is shaped too much like
"transactional KV plus queues".

That seam works well for FoundationDB. It does not map cleanly onto the infrastructure that
enterprises already have and are comfortable operating at scale. In particular:

- Kafka is common.
- S3-compatible object storage is common.
- Kubernetes is common.
- FoundationDB is not common.

At the same time, the dark-factory direction pushes us toward workloads that are naturally
event-driven, asynchronous, and operationally distributed across many workers and many host
execution environments.

For that reason, `v0.17-kafka` is intentionally a radical reset of the hosted architecture.
This milestone is not "make Kafka back the existing hosted traits". It is "replace the hosted
shape with a log-first design that fits Kafka and S3 directly".

## Design Thesis

The new thesis is:

- Kafka is the hot runtime substrate, centered on canonical world history and submission topics.
- `aos-journal` is the only authoritative replay log.
- `aos-ingress` is a short-retention submission queue, not a second journal.
- S3 is the immutable blob, snapshot, checkpoint, and artifact plane.
- The `BlobPlane` is a logical content-addressed store: logical blob refs remain content hashes,
  while the physical layout may be direct objects or immutable packed objects.
- Workers own Kafka partitions, not individual worlds.
- Workers consume submissions, emit canonical world-log frames, and recover by replaying the
  authoritative world log.
- World state advances only by replaying records from the authoritative log.
- Large outputs, artifacts, logs, and snapshots live in S3 and are referenced from the log.
- Small blobs should be allowed to live inside immutable pack objects behind authoritative CAS
  metadata rather than forcing one remote object per logical blob.
- Local and shared caches remain first-class and should hide remote object latency from the hot
  path.
- Effects and host execution remain subordinate to the world log: they may run elsewhere, but
  world state only changes when receipts re-enter the authoritative log.

This means AOS stops looking like:

- a distributed KV database with inboxes, journal heads, leases, and durable queue tables

and starts looking like:

- a deterministic actor/world runtime over an authoritative distributed log.

That is a much better fit for agent workloads and enterprise deployment reality.

## Radical Change Note

This roadmap explicitly accepts:

- breaking the current hosted persistence seam
- dropping the assumption that hosted world ownership is a per-world lease protocol
- replacing inbox/journal/queue rows with log records
- rethinking local mode so it shares semantics with the new hosted model rather than the old
  hosted implementation

Historical note:

- `v0.14-infra` remains valuable as the record of the first hosted prototype.
- `v0.17-kafka` supersedes it as the forward-looking hosted direction.

## Milestone Map

- `p1-radical-reset-and-log-first-seam.md`
  - state the reset clearly and define the new core planes and contracts
- `p2-kafka-world-log-routing-and-workers.md`
  - define the authoritative Kafka world log, routing, worker ownership, and record model
- `p3-s3-checkpoints-snapshots-and-recovery.md`
  - define the S3 recovery plane, replay budget, and retention model
- `p5-hot-worlds-routing-overrides-and-lanes.md`
  - define the minimal route-override, route-epoch, and pause-and-reroute model for `v0.17`
- `p6-local-runtime-and-product-cutover.md`
  - define how local runtime and product surfaces converge on the new model
- `p7-kernel-journal-invariant-and-compaction.md`
  - define the single kernel journal invariant, snapshot vs checkpoint boundary, and in-place
    compaction semantics
- `p8-embedded-node-unification-and-test-cutover.md`
  - define the later `aos-node` unification so embedded local product flows and tests use the same
    infrastructure
- `p9-local-secrets-env-only-cutover.md`
  - define the local-only secret model so env/`.env` power local worlds without hosted-style
    secret storage or sync
- `p10-hosted-product-readiness-and-surface-cutover.md`
  - define what is required to make the hosted Kafka/S3 runtime a real hosted product surface,
    including worker-only, control-only, and combined deployments
- `p11-hosted-query-projections-and-materialization.md`
  - define the hosted derived query plane so gateways can serve current-state reads without
    routing every request to the owning worker, with a later separable materializer/reducer role
- `p12-hosted-worker-main-flow-test-coverage.md`
  - define the missing real-infra hosted worker-flow tests that are still required for confidence
    in the Kafka/S3 runtime
- `p13-hosted-timer-runtime-and-tests.md`
  - define the hosted timer execution path and the minimum timer tests needed once that path exists
- `p16-node-routing-reset-and-world-epoch.md`
  - remove route-topic complexity from the base node, replace `route_epoch` with `world_epoch`,
    and make higher-order routing an external layer above the simple two-topic runtime

## Progress Snapshot

Completed in code on the experimental branch:

- P1 shared log-first seam in `aos-node`
- embedded `MemoryLogRuntime` / `MemoryShardWorker` implementing the new planes
- authoritative `WorldLogFrame` flow, `world_epoch`, checkpoints, and replay
- `aos-node-hosted` replacement on top of the new seam with a simple two-topic HTTP/control surface
- real Kafka-backed submission and journal planes in `aos-node-hosted`
- durable blobstore-backed checkpoint and blob publication in `aos-node-hosted`
- hosted startup recovery from blobstore checkpoints plus Kafka journal replay
- packed-CAS metadata, range-resolved blob reads, and retained checkpoint-manifest pruning in
  `aos-node-hosted`
- hosted role split with `worker`, `control`, and `all`
- hosted local `.aos` state roots for cache/CAS/runtime files
- hosted create-by-manifest via ingress with immediate first checkpoint
- hosted create-by-seed and fork via the same ingress-driven lifecycle path
- hosted governance command submission plus command-record polling
- hosted manifest / defs / def-get / workspace-resolve / `latest_durable` state reads
- hosted regular checkpointing by time and committed event count, with kernel-journal compaction
- hosted workers recovering worlds from checkpoint + journal instead of local bootstrap metadata
- P16 routing reset in the active runtime:
  - no route topic
  - no `route_epoch`
  - deterministic `world_id` hashing for partition selection
  - optional `expected_world_epoch` fencing for advanced callers
  - higher-order routing moved out of the base node

Not yet completed:

- missing hosted worker-flow coverage for receipts, reopen correctness, and failover
- hosted timer execution and timer-specific hosted tests
- broader blob GC / object-store lifecycle policy beyond retained checkpoint manifests

Deferred to `roadmap/v0.18-execution/` and `roadmap/v0.19-fabric/`:

- core owner/executor seam and detached execution lifecycle
- dedicated fabric/effect lane cutover
- hosted fabric/session/artifact/log control surfaces
- fabric-side secret-provider cutover

Could be added later beyond current `v0.17` scope:

- lane topics
- placement classes
- automatic hot-world or hot-partition migration policy

## Non-Goals Of This Milestone

- preserve API compatibility with the current hosted storage traits
- make Kafka emulate FoundationDB transactions
- retain exact feature parity with the current hosted implementation before the new direction is
  usable
- build a first-party secret vault
- solve every future query/control-plane product concern before the new core runtime model exists

## Desired End State

At the end of this milestone, the intended architecture should be clear enough that we can
begin implementing:

- a shared log/checkpoint contract
- an embedded local backend that uses the same semantics
- a Kafka/S3 hosted backend
- optional lane and placement extensions without compromising the core correctness model
