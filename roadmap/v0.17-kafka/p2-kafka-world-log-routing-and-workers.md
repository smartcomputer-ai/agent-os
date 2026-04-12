# P2: Kafka World Log, Routing, and Workers

**Priority**: P2  
**Effort**: Very High  
**Risk if deferred**: High (implementation will accidentally recreate inboxes, leases, and queue tables)  
**Status**: Complete for v0.17 (real Kafka path exists; consumer-group partition assignment,
partition-scoped transactional commit, caller-carried route-epoch fencing, and assignment-scoped
hot-world activation/recovery are in; remaining producer-fencing/rebalance/ops items are deferred
follow-on hardening, not blockers for the v0.17 cut)

## Goal

Define the authoritative Kafka-side runtime model:

- what topics exist
- what records exist
- who owns partitions
- how worlds are routed
- how workers read and write the log

## Completed In Code

Implemented on the experimental branch:

1. Default `aos-ingress` / `aos-journal` topic vocabulary and `WorldRoute`.
2. `WorldLogFrame` transport shape with contiguous `world_seq` ranges.
3. Submission admission with stable `submission_id` handling.
4. Route-epoch fencing and reroute epoch bumps.
5. Partition-owned worker execution semantics via `MemoryShardWorker` and the hosted shard worker.
6. Authoritative replay from `WorldLogFrame`s rather than the submission queue.
7. Real Kafka-backed submission, journal, and route topics in `aos-node-hosted`.
8. Hosted worker recovery by rebuilding routes, restoring checkpoints, and replaying Kafka journal
   tail.
9. Transactional journal publish plus ingress offset commit on the broker-backed path.
10. `read_committed` recovery/consumption and failure-focused rollback/restart tests.
11. Broker-backed hosted workers now join a shared ingress consumer group and only process the
    partitions Kafka assigns them.
12. Broker-backed journal publish now uses partition-scoped transactional producer identities
    rather than one process-wide transactional producer.
13. Hosted hot worlds are now activated/deactivated from Kafka partition assignment rather than
    being eagerly opened cluster-wide at startup.
14. Broker recovery now rebuilds local journal/frame state through the same contiguous
    `world_seq` validation path used by the embedded/runtime seam.
15. Hosted submit surfaces for existing worlds now require caller-supplied `route_epoch` rather
    than silently defaulting to the current route epoch server-side.
16. Broker startup/activation recovery is now route-first plus per-assigned-partition journal
    catch-up rather than eagerly rebuilding broader broker journal state before partition
    ownership is known.

Deferred follow-on hardening after P2:

1. Stronger broker-side producer fencing and dynamic rebalance hardening.
2. Operational compaction/retention validation for the route topic and runtime topics.

## First Working Kafka Version Scope

For `v0.17`, the first solidly working Kafka version should stop at the minimum correctness core.

Essential for the first version:

1. Keep the three-topic runtime: `aos-ingress`, `aos-journal`, and compacted `aos-route`.
2. Keep `aos-ingress` and `aos-journal` on the same partition count and stable partition function.
3. Treat consumer-group shard ownership as the ownership boundary; do not reintroduce per-world
   leases.
4. Require immediate stop-on-revocation behavior so a revoked worker stops admitting submissions
   and stops emitting authoritative `WorldLogFrame`s for that shard.
5. Require the steady-state Kafka transaction shape:
   - consume from `aos-ingress`
   - validate `route_epoch`
   - dedupe stable `submission_id`
   - process deterministically
   - emit canonical `WorldLogFrame`s to `aos-journal`
   - commit consumed ingress offsets in the same Kafka transaction
6. Treat speculative warm state as committed only after the Kafka transaction commits; abort must
   discard or rebuild speculative state before continuing.
7. Recover only from `checkpoint + read_committed aos-journal replay`, never from ingress replay
   and never from leftover worker memory.
8. Keep `WorldLogFrame` as the only authoritative outer wire shape and preserve contiguous
   per-world `world_seq` validation on replay.
9. Keep `route_epoch` as a hard admission fence for stale submissions, stale receipts, and
   reroutes.
10. Keep submission normalization strict: ingress/create/bootstrap/admin/receipt traffic is
    admission vocabulary and must normalize into canonical `WorldRecord`s rather than being copied
    through as request-shaped authoritative history.

Explicitly deferred for now:

1. Stronger broker-side producer fencing beyond the current first working path.
2. Dynamic rebalance hardening beyond the current basic assignment-sync and revoke-drop model.
3. Operational compaction/retention validation and automation for route/runtime topics.

Practical first-version operating stance:

- stable partition counts
- controlled deployments with infrequent rebalance
- restart/reassignment recovery rather than aggressive live churn handling

Follow-on after the current implementation:

1. Leave stronger producer fencing, rebalance hardening, and topic retention/compaction policy as
   the hardening bucket after the completed first working Kafka cut.

## Primary Design Stance

The minimal correctness core should keep submission traffic distinct from authoritative history.

Default target:

- one shared `aos-ingress` topic for non-authoritative submissions
- one shared `aos-journal` topic for authoritative world-history frames
- one shared compacted `aos-route` topic for current route metadata

Important corollaries:

- multiple universes may share one topic family
- a single universe does not require dedicated topics
- only the current owner emits authoritative `WorldLogFrame`
- only authoritative world-log records consume `world_seq`
- raw ingress, raw receipts, and admin/control submissions are not journal entries
- workers do not need to tail `aos-journal` in steady state; they recover from it
- route metadata does not belong in the world-history stream

## Topic Model

### Default

One submission topic:

- `aos-ingress`

with:

- short retention
- records keyed by `(universe_id, world_id)` and routed by the current `WorldRoute`
- producers such as ingress gateways, timer services, effect/fabric bridges, and admin/control
  services

One authoritative journal topic:

- `aos-journal`

with:

- long retention sized from replay and checkpoint policy
- one outer wire type: `WorldLogFrame`
- records keyed by `(universe_id, world_id)` so one world is routed to one partition at a time

One compacted route topic:

- `aos-route`

with:

- records keyed by `(universe_id, world_id)`
- values holding the latest `WorldRoute`
- compaction retaining the current route state per world
- one current route record should exist for every active world; `aos-route` is a full directory,
  not a sparse override list

### First implementation stance

The first implementation should keep the paired runtime simple:

- `aos-ingress` and `aos-journal` use the same partition count
- `aos-ingress` and `aos-journal` use the same stable partition function
- one owned shard is therefore the paired `(ingress_topic, journal_topic, effective_partition)`
  address
- by default, `partition_override` is absent and the effective partition is derived from
  `(universe_id, world_id)` via the Kafka key / stable partition function
- if `WorldRoute.partition_override` is present, it is an explicit manual placement override

### Allowed follow-on

A world may later be routed to a different paired lane, but that is not the default.

The route abstraction should still be defined from the start as:

```text
WorldRoute {
  ingress_topic,
  journal_topic,
  partition_override?,
  epoch
}
```

## Worker Ownership Model

### 1) Kafka consumer-group ownership replaces world leases

Workers join the submission-topic consumer group and are assigned partitions.

In the first implementation, that assignment also identifies the matching authoritative
journal partition for the same route.

### 2) A worker owns every world currently routed to its assigned shard

There is no separate per-world lease record in the correctness core.

### 3) Partition revocation is the new fencing event

When Kafka revokes a shard:

- the worker must stop processing submissions for it immediately
- the worker must stop emitting authoritative outcomes for worlds on that shard
- any warm local state for that shard becomes non-authoritative

### 4) World ownership is therefore indirect

Worlds are owned because their routed shard is owned.

## Worker Read/Write Model

### Steady state

Each worker:

1. consumes submissions from `aos-ingress`
2. validates route epoch and deduplicates stable submission ids
3. deterministically processes them against warm world state
4. assigns the next contiguous `world_seq` values for the affected world
5. emits canonical `WorldLogFrame`s to `aos-journal`
6. optionally emits dispatch records to `aos-effect` or `aos-fabric`
7. commits consumed `aos-ingress` offsets in the same Kafka transaction

This means:

- workers are steady-state consumers of submissions and producers of authoritative world history
- workers do not need to self-consume `aos-journal` during normal operation

### Warm state shape

For keyed workflows, "warm world state" should explicitly include the current cell-head model:

- snapshot/checkpoint-anchored per-workflow `CellIndex` roots as the persisted base layer
- shard-local in-memory clean cell cache for recently used cells
- shard-local dirty cell delta entries that shadow both cache and base until snapshot materialization
- optional spill of large dirty cell state to CAS/object storage while remaining logically dirty

Important stance:

- the Kafka architecture must preserve this layered head view because rewriting `CellIndex` state on
  every admitted submission would destroy hot-path performance
- a worker may carry warm cell caches and dirty deltas across many committed submission batches
- those caches are performance state, not the semantic recovery root

### Warm state promotion rule

Warm local state may be used for speculative processing, but it does not become authoritative in
memory merely because the worker produced a frame.

Required rule:

- a worker must treat state derived from a submission batch as committed only after the Kafka
  transaction that publishes the resulting `WorldLogFrame`s and consumed offsets commits
- if that transaction aborts, the worker must discard or rebuild the speculative shard/world state
  before processing continues
- after commit, the worker may keep the resulting cell-cache and delta-layer state warm in memory;
  it does not need to rewrite checkpoint roots on each batch
- recovery truth remains `checkpoint + read_committed aos-journal replay`, never "whatever memory
  looked like before the abort"

### Recovery

On startup, failover, or rebalance catch-up, a worker:

1. loads the latest committed checkpoint from S3
2. restores promotable baselines for the worlds on the assigned shard, including checkpointed
   `CellIndex` roots and other persisted workflow runtime state
3. seeks `aos-journal` to the stored journal offset
4. replays `WorldLogFrame`s forward and validates contiguous `world_seq`
5. resumes steady-state consumption from `aos-ingress`

The canonical replay source is `aos-journal`, not `aos-ingress`.

Recovery should not require eager loading of every cell into memory:

- restored `CellIndex` roots provide the persisted base layer
- hot cell caches may repopulate lazily as cells are read or replayed
- replayed post-checkpoint updates rebuild dirty head state exactly as they do in the current kernel

## Submission Model

Submissions are admission objects, not authoritative history.

Required properties:

- stable `submission_id` or `ingress_id`
- target `(universe_id, world_id)`
- target `route_epoch` for existing routed worlds
- type-specific payload, inline or by CAS/S3 ref
- enough fencing metadata to reject stale routes or stale receipts when needed

Submission kinds should cover at least:

- external domain ingress
- external effect receipts
- external fabric receipts or lifecycle events
- timer firings from external services
- admin/control actions
- world bootstrap or create requests

Important stance:

- submissions do not receive `world_seq`
- submissions are not replayed as canonical world history
- the owner decides whether a submission becomes one or more authoritative world records

### Submission normalization rule

The submission vocabulary is an admission and transport vocabulary.

It is not required, or even desirable, for it to match the authoritative replay vocabulary
one-for-one.

Required rule:

- the journal should keep the smallest practical canonical `WorldRecord` set that expresses the
  semantic actions the runtime and kernel actually undertook
- a submission may normalize into zero, one, or many authoritative world records
- normalization is allowed to discard transport-only detail once the authoritative semantic record
  has been produced
- command names, request envelopes, dispatch bookkeeping, and status/result tracking are ingress or
  projection concerns unless they are themselves part of the canonical replay model
- the owner is responsible for translating submission shapes into canonical replay records rather
  than copying submission payloads into the journal unchanged

### Route epoch protocol

`route_epoch` is not advisory metadata. It is the reroute fence.

Required rule:

- every submission path that targets an existing world must carry the sender's intended
  `route_epoch`
- world-create/bootstrap paths may use a dedicated bootstrap flow before the first route exists
- partition owners admit submissions and receipts only when the carried `route_epoch` matches the
  current `WorldRoute`
- stale submissions, stale receipts, and stale timer/admin messages must be deterministically
  rejected or bounced; they must never be best-effort admitted against a stale cache
- pause-and-reroute must advance `route_epoch` before new submissions are admitted on the new
  route

## Canonical Frame Model

The authoritative world-log topic should carry one top-level value shape:

```text
WorldLogFrame {
  format_version: u16,
  universe_id: UniverseId,
  world_id: WorldId,
  route_epoch: u64,
  world_seq_start: u64,
  world_seq_end: u64,
  records: list<WorldRecord>
}
```

Required invariants:

- one `WorldLogFrame` covers exactly one `(universe_id, world_id, route_epoch)`
- `records` are in canonical replay order
- the semantic seq of `records[i]` is `world_seq_start + i`
- `world_seq_end = world_seq_start + records.len() - 1`
- only the current owner emits `WorldLogFrame`
- the frame is the Kafka transport and commit unit; `WorldRecord` is the replay unit

## Required World Records

The authoritative world log must be able to carry at least:

- `DomainEvent`
- `EffectIntent`
- `EffectReceipt`
- `StreamFrame`
- `CapDecision`
- `PolicyDecision`
- `Proposed`
- `ShadowReport`
- `Approved`
- `Applied`
- `Manifest`
- `SnapshotPromoted` or equivalent recovery marker
- any explicit world lifecycle records the final runtime keeps as canonical history

Large payload rule:

- large payloads do not live inline in Kafka records
- records carry refs to S3 blobs when needed

Important stance:

- the exact `WorldRecord` schema should stay close to current canonical journal payloads
- raw ingress envelopes, raw control commands, and command status/result tracking are not
  authoritative `WorldRecord`s
- bootstrap, create-world, admin, and fabric submissions should normalize into canonical lifecycle,
  governance, snapshot, manifest, intent, receipt, or domain records rather than being reified as
  request-shaped journal entries

### Bootstrap and create-world normalization

World bootstrap and creation belong on the submission plane, not as raw authoritative history.

Required rule:

- accepted bootstrap/create submissions must normalize into the minimal canonical replay records
  needed for recovery and audit
- route publication is current-state metadata in `aos-route`, not a `WorldRecord`
- if world creation activates a manifest and establishes an initial baseline, the resulting
  authoritative history should be expressed through canonical lifecycle, `Manifest`, `Snapshot`, or
  equivalent recovery records
- a raw `CreateWorld` request envelope should not be copied into the journal as the authoritative
  replay fact unless the final runtime explicitly chooses such a lifecycle record as part of the
  canonical world-history vocabulary

## Governance And Manifest Semantics

The authoritative world log should support the full governance/control-plane semantics used by
AOS today, even if a specific path toward a new manifest makes some precursor records optional.

Important stance:

- `Proposed`, `ShadowReport`, and `Approved` remain part of the supported world-history model
- specific deployments or policy paths may choose lighter flows where some of those steps are
  optional
- `Applied` and `Manifest` are not optional when the active manifest actually changes
- `Manifest` marks the exact replay boundary where the new manifest becomes active
- admin/control submissions or governance effect receipts are not substitutes for the resulting
  authoritative control-plane records
- raw command names, command envelopes, and command completion bookkeeping are not themselves the
  canonical governance history unless the final journal vocabulary explicitly defines them that way

Large governance payload rule:

- large shadow reports, diffs, or audit artifacts may be externalized to S3/CAS
- the world log should still carry the semantic record, hashes, and refs needed for replay and
  audit

## Ordering Model

Important rule:

- correctness depends on authoritative partition order plus contiguous per-world `world_seq`, not
  on a global topic order

Therefore:

- a world must only be active on one route at a time
- all authoritative records for a given world route to that world's current log address
- replay operates on `aos-journal`, not on the submission queue
- `route_epoch` fences reroutes and helps reject stale submissions or stale receipts

## Transaction Model

For Kafka-native workers, the intended hot-path execution shape is:

1. consume submissions from `aos-ingress`
2. deterministically process them
3. produce resulting `WorldLogFrame`s to `aos-journal`
4. optionally produce dispatch records to `aos-effect` or `aos-fabric`
5. commit consumed `aos-ingress` offsets in the same Kafka transaction

Important stance:

- Kafka transactions are for Kafka read/process/write coordination
- they should stay short and should not span long-running inline effects or host work
- they are not a replacement for arbitrary external exactly-once side effects
- if transactions are unavailable, stable submission ids and owner-side dedupe are still required

Kafka-native ownership fencing should also use the broker boundary, not only worker etiquette.

Required rule:

- the Kafka-native implementation should use a shard-scoped transactional producer identity for the
  authoritative writer of that shard
- shard takeover must fence the previous writer before it can emit more authoritative
  `WorldLogFrame`s
- application-level revocation handling still matters, but broker-side producer fencing is the
  authoritative safety boundary for stale owners

## Routing Directory

The runtime needs an explicit route directory:

- `(universe_id, world_id) -> WorldRoute`

The latest route value is the decisive answer to "where does this world live right now?"

The effective partition is `WorldRoute.partition_override` when present; otherwise it is derived
from `(universe_id, world_id)` via the Kafka key / stable partition function for that route's
paired topics.

Default target:

- a compacted Kafka topic such as `aos-route`

Why:

- route state is table-shaped "latest value" metadata
- it is part of the hot runtime control surface
- a compacted topic is a cleaner fit than S3 or a hidden transactional side database
- the directory should be complete for active worlds, not only populated for exceptions

This route directory must be consulted by:

- ingress gateway
- internal `portal.send`
- effect and fabric receipt writers
- timer-firing logic
- administrative tools

Initial assignment rule:

- when a world is created, assign the default paired topics `aos-ingress` and `aos-journal`
- write an initial `WorldRoute` record to `aos-route`
- leave `partition_override` absent for normal keyed placement
- set `partition_override` only when explicitly pinning the world to a partition

## What Disappears From The Core

The following are intentionally not part of the new correctness core:

- per-world lease records
- inbox rows
- inbox cursor CAS
- journal-head CAS
- command status or result tracking as replay truth
- ready-world secondary indexes
- durable pending/inflight effect tables
- durable pending/inflight timer tables
- journal segment compaction

If needed later, derived indexes may exist, but the core must not depend on them.

Important clarification:

- removing durable pending tables from the core does not remove replayable runtime state
- strict quiescence still depends on replayable state such as workflow instance status, inflight
  intents, queued effects, pending workflow receipts, and comparable timer/fabric waiting state
- that state must be recoverable from checkpoints and authoritative journal replay, not held only in
  worker memory

## Out of Scope

1. Multi-topic hot-world lane policy beyond defining the route abstraction.
2. Exact record schema details down to every field encoding.
3. Query-plane materialization strategy beyond "derived, not authoritative".

## DoD

1. The roadmap defines `aos-ingress` as the default submission topic, `aos-journal` as the
   authoritative world-log topic, and `aos-route` as the compacted route topic.
2. Partition ownership is the declared replacement for world leases.
3. Steady-state workers consume `aos-ingress`, emit `WorldLogFrame`s to `aos-journal`, and use
   `aos-journal` as the recovery source.
4. `WorldLogFrame` is defined as the canonical outer wire shape for `aos-journal`.
5. The required authoritative `WorldRecord` variants are listed, including manifest transition
   records.
6. The first implementation stance keeps `aos-ingress` and `aos-journal` on the same partition
   count and stable partition function.
7. The route directory is recognized as part of the runtime core, given a concrete compacted
   Kafka target, and treated as a full current-placement directory for active worlds.
8. The roadmap defines explicit `route_epoch` admission semantics for stale submissions and
   reroutes.
9. The roadmap states that warm state becomes authoritative only after Kafka commit, with abort
   recovery rules.
10. The roadmap states that strict-quiescence state must remain recoverable from
    checkpoint + journal replay rather than only worker memory.
11. The roadmap explicitly preserves the current layered cell-cache/head-state model for keyed
    workflows rather than implying per-record `CellIndex` rewrites.
