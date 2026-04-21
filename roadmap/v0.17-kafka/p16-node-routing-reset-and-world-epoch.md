# P16: Node Routing Reset and World Epoch

**Priority**: P16  
**Effort**: High  
**Risk if deferred**: High (the runtime keeps carrying route-topic complexity that the product no
longer wants, and higher-level routing will be harder to build cleanly later)  
**Status**: Implemented

## Implemented Now

The active hosted runtime now matches the P16 direction in the following ways:

- `route_epoch` has been replaced by `world_epoch` in the active hosted transport path
- `SubmitEventRequest` now uses optional `expected_world_epoch`
- hosted broker mode no longer reads, writes, or configures `aos-route`
- hosted broker partition selection is derived directly from `world_id`
- hosted worker recovery and activation no longer depend on route-topic recovery
- hosted checkpoints now carry `world_epoch`
- hosted startup/config no longer mentions or requires a route topic
- local dev scripts now provision only `aos-ingress` and `aos-journal`
- hosted profiling and hosted tests have been updated to the `world_epoch` contract

Verified with:

- `cargo build -p aos-node-hosted`
- `cargo test -p aos-node-hosted --no-run`

Primary implemented files:

- `crates/aos-node/src/planes/model.rs`
- `crates/aos-node/src/api/mod.rs`
- `crates/aos-node/src/planes/world.rs`
- `crates/aos-node/src/planes/memory.rs`
- `crates/aos-node/src/embedded/{runtime,sqlite}.rs`
- `crates/aos-node-hosted/src/kafka/{types,broker,embedded,local_state}.rs`
- `crates/aos-node-hosted/src/worker/{runtime,lifecycle,execute,checkpoint,supervisor,types}.rs`
- `crates/aos-node-hosted/src/control/facade.rs`
- `crates/aos-node-hosted/src/main.rs`
- `crates/aos-node-hosted/src/bin/hosted-prof.rs`
- `dev/hosted/hosted-topics-ensure.sh`
- `dev/hosted/hosted-topics-reset.sh`

## Remaining Work

No runtime work remains for the P16 scope. Follow-on cleanup, if desired, is cosmetic:

- continue renaming any leftover route-era helper/test language for consistency
- run or expand runtime integration coverage beyond compile-time verification

## Goal

Aggressively simplify the base hosted node runtime:

- remove Kafka-managed route metadata from the node
- remove `route_epoch` from the correctness model
- keep the base node on a simple two-topic runtime: `aos-ingress` and `aos-journal`
- make partition ownership come directly from Kafka consumer-group assignment plus deterministic
  `world_id` hashing
- introduce `world_epoch` as the only node-level incarnation fence needed for future higher-order
  routing systems

This item is intentionally not backward-compatible.

The desired outcome is:

- the base node is simple and unsurprising
- the node owns deterministic execution, replay, checkpoints, and materialization
- placement policy, reroute policy, and richer routing metadata live above the node rather than
  inside it

## Superseded Earlier Stance

This item supersedes the route-topic / route-epoch parts of the earlier `v0.17` routing work.

In particular, this item replaces the following earlier design choices:

- compacted `aos-route` as runtime-critical hosted metadata
- `WorldRoute` as the authoritative hosted placement object
- `partition_override` as part of the base runtime model
- `route_epoch` as a required write-admission fence
- pause-and-reroute as a built-in base-node responsibility

Those ideas were useful while the Kafka/S3 design was still finding its shape, but they are now
the wrong default.

The base node should be a deterministic world worker over:

- one ingress topic
- one journal topic
- one partition checkpoint plane
- one deterministic `world_id -> partition` function

Anything fancier should be layered above that.

## Primary Stance

### 1) The base node is non-routed

The node should not carry a runtime-managed routing directory.

Required stance:

- no route topic
- no route records
- no world-level placement table inside the node
- no node-managed reroute protocol
- no per-world manual partition override in the base runtime

### 2) Partition ownership comes from Kafka only

The node should use Kafka consumer groups the ordinary way:

- workers join one shared ingress consumer group
- Kafka assigns ingress partitions
- the worker owns the matching journal partitions with the same partition ids
- `world_id` hashing decides which partition a world's traffic belongs to

There should be no second ownership protocol inside the node.

### 3) `world_epoch` replaces `route_epoch`

The node should keep one minimal future-proof fence:

- `world_epoch` identifies the active incarnation of a world
- it is not a routing token
- it is not a partition-selection token
- it is not tied to Kafka topic metadata

Simple hosted mode will usually keep `world_epoch` stable forever.

Future higher-order routing/control systems may bump it when they intentionally activate a new
incarnation of a world.

## End State

After P16, the base hosted node should look like this:

1. submissions are written to `aos-ingress`
2. authoritative frames are written to `aos-journal`
3. both topics are keyed by `world_id`
4. both topics use the same partition count
5. partition ownership comes only from Kafka consumer-group assignment
6. world placement is `hash(world_id) % partition_count`
7. recovery is `checkpoint + read_committed journal replay`
8. `world_epoch` is the only world-incarnation fence
9. the node has no route topic, route table, route overrides, or reroute epoch

## `world_epoch` Contract

## 1) Meaning

`world_epoch` is the activation generation of a world.

It answers only one question:

- "Is this submission / frame / checkpoint associated with the currently intended incarnation of
  this world?"

It does **not** answer:

- where the world is placed
- which Kafka partition owns the world
- which topic family owns the world
- whether a worker should process a world outside normal partition ownership

## 2) Initialization

Required rule:

- newly created worlds start at `world_epoch = 1`

There is no epoch `0` for an active world.

## 3) Bump Conditions

Required rule:

- `world_epoch` changes only on an explicit activation transition

Examples of valid future bump triggers:

- an external routing/controller layer drains and checkpoints a world, then activates a new
  incarnation elsewhere
- an operator performs an intentional force-reopen / force-recover action
- a higher-level control layer wants to fence stale writers against a newly authoritative world

Examples of events that must **not** bump `world_epoch`:

- normal submissions
- checkpoint publication
- worker restart
- worker rebalance within the same base deployment
- materializer restart
- timer firing
- receipt ingress

## 4) Transport Contract

`world_epoch` belongs in the canonical transport objects:

- `SubmissionEnvelope`
- `WorldLogFrame`

It may also appear in checkpoint metadata where useful for validation.

Required rules:

- every `SubmissionEnvelope` for an existing world carries one `world_epoch`
- every `WorldLogFrame` carries exactly one `world_epoch`
- all records inside a frame belong to the same `(world_id, world_epoch)` stream segment
- for one world, observed frame epochs must be monotonic non-decreasing

## 5) Admission Contract

The base hosted API should not require ordinary callers to supply `world_epoch`.

Required stance:

- simple callers submit events/commands/receipts against a world id
- the node resolves the current active `world_epoch` for that world and writes it into the
  canonical submission envelope

Implemented in hosted now:

- advanced callers may provide `expected_world_epoch`
- hosted rejects the submission when it does not match the currently active epoch

Optional future extension:

- expand that same fence into other advanced control paths if higher-order routing needs it

This keeps the simple path simple while preserving the primitive needed by higher-order control
systems later.

## 6) Replay / Validation Contract

`world_epoch` is a validation boundary, not a replay root by itself.

Required rules:

- `world_seq` remains contiguous per world across frames
- `world_epoch` may stay the same across many frames
- when `world_epoch` increases for a world, the new epoch must be greater than the last observed
  epoch for that world
- materializers, replay tools, and recovery code must reject epoch regression for a world

Recommended stance:

- keep `world_seq` globally contiguous per world even across epoch changes
- do not reset `world_seq` when `world_epoch` changes

This keeps replay simpler than inventing per-epoch local sequence ranges.

## 7) Checkpoint Contract

Partition checkpoints should preserve enough information to validate epoch continuity during
recovery.

Required stance:

- checkpoints carry per-world `world_epoch`
- checkpoints do not need a partition-level epoch field
- recovery restores the latest known epoch for each checkpointed world and then validates replayed
  frames against it

## Base Transport Model

## Topics

The base node keeps exactly two Kafka runtime topics:

- `aos-ingress`
- `aos-journal`

There is no `aos-route`.

## Keys

Required stance:

- ingress records are keyed by `world_id`
- journal records are keyed by `world_id`
- partitioning uses Kafka's normal partition selection over that key

`universe_id` remains semantic/storage scope, not transport placement metadata.

## Partition Function

The node should expose one deterministic function:

```text
partition_for_world(world_id, partition_count) -> u32
```

Required properties:

- stable for a fixed `partition_count`
- derived only from `world_id`
- no hidden route table lookup
- no topic override
- no manual placement override in the base node

## Worker Ownership Model

## 1) Shared consumer-group ownership

Workers should join one shared ingress consumer group using one default group id.

The currently existing consumer-group shape already points in this direction; P16 makes it the
only supported ownership model for the base node.

## 2) Paired ingress/journal partition ownership

If a worker owns ingress partition `P`, it also owns journal partition `P`.

The runtime should assume:

- `aos-ingress` and `aos-journal` have the same partition count
- partition id `P` on ingress is paired with partition id `P` on journal

## 3) No direct assignment in the product node

The base `aos-node-hosted` binary should not expose or depend on direct partition assignment,
manual partition pinning, or override-oriented ownership.

If debugging or test harnesses need direct partition assignment, that belongs in:

- tests
- profiling tools
- harness binaries

not in the product runtime contract.

## Recovery And Replay Model

Recovery should become purely partition-scoped.

For each assigned partition:

1. load the latest committed partition checkpoint from blobstore
2. restore the checkpointed worlds for that partition
3. seek `aos-journal` to `checkpoint.journal_offset + 1` in `read_committed` mode
4. replay all later `WorldLogFrame`s for that partition
5. validate:
   - `world_seq` continuity per world
   - `world_epoch` monotonicity per world
6. rebuild warm active world state
7. resume steady-state ingress consumption

World discovery must come from:

- checkpoint contents
- replayed journal frames
- materialized projections

not from a route topic and not from a separate blobstore world registry.

## World Creation Contract

World creation remains ingress/worker-authoritative.

Required stance:

- `create world` is a create submission on `aos-ingress`
- it is keyed by `world_id`
- the owning worker processes it
- the worker emits the initial authoritative journal frame
- the created world starts at `world_epoch = 1`
- world existence is discovered from journal/checkpoint/materialized state

There should be no side metadata insert that makes a world exist before the worker/journal path
does.

## Materializer Contract

The materializer should no longer depend on route metadata.

Required stance:

- it consumes journal partitions
- it validates `world_seq` continuity per world
- it validates non-decreasing `world_epoch` per world
- it materializes world summaries, state, workspace views, and journal indexes from checkpoint plus
  journal facts

It must not need:

- route-topic replay
- route-table hydration
- route-epoch interpretation

## Higher-Order Routing Above The Node

P16 does **not** say "never support richer routing."

It says:

- do not hard-wire richer routing into the base node

Later systems may still build:

- special placement policy
- world migration policy
- multi-deployment routing
- active/incarnation fencing
- external traffic steering

But those systems should sit above the base node and use node primitives rather than relying on a
node-owned route directory.

### Primitives the node should keep

The base node should retain only the primitives that higher-order routing actually needs:

- stable `world_id`
- deterministic `partition_for_world(world_id, partition_count)`
- partition checkpoints
- authoritative journal replay
- `world_epoch`
- duplicate-submission fencing via `submission_id`
- explicit quiesce/checkpoint/resume hooks when needed
- worker/partition health and assignment introspection

### Primitives the node should drop

The base hosted runtime should drop:

- `WorldRoute` as an active hosted correctness dependency
- `RoutePlane` as an active hosted correctness dependency
- `route_topic`
- `partition_override`
- reroute APIs
- `route_epoch`
- route-first recovery

## Refactor Scope

This refactor is deliberately aggressive.

## 1) Shared transport/model changes

Update the shared seam in `aos-node`:

- replace `route_epoch` with `world_epoch`
- remove `SubmissionRejection::RouteEpochMismatch`
- remove route-oriented checkpoint fields
- later delete `WorldRoute` and `RoutePlane` entirely once local/shared compatibility code is
  collapsed

Expected primary files:

- `crates/aos-node/src/planes/model.rs`
- `crates/aos-node/src/planes/traits.rs`
- `crates/aos-node/src/api/mod.rs`

## 2) Hosted Kafka plane changes

Simplify broker-backed hosted Kafka:

- remove `route_topic` from config
- remove route publish/recovery
- remove route-table dependence from hosted execution
- compute partition only from `world_id`
- keep shared consumer-group ingestion

Expected primary files:

- `crates/aos-node-hosted/src/kafka/types.rs`
- `crates/aos-node-hosted/src/kafka/mod.rs`
- `crates/aos-node-hosted/src/kafka/broker.rs`
- `crates/aos-node-hosted/src/kafka/local_state.rs`

## 3) Hosted worker/runtime changes

Rewrite hosted runtime ownership and recovery:

- remove route lookup on submit/recover/activation
- reject reroute in simple hosted mode
- remove route-epoch admission
- drive activation from partition assignment + recovered journal/checkpoint state
- carry `world_epoch` instead

Expected primary files:

- `crates/aos-node-hosted/src/worker/runtime.rs`
- `crates/aos-node-hosted/src/worker/lifecycle.rs`
- `crates/aos-node-hosted/src/worker/execute.rs`
- `crates/aos-node-hosted/src/worker/supervisor.rs`
- `crates/aos-node-hosted/src/worker/checkpoint.rs`
- `crates/aos-node-hosted/src/worker/types.rs`

## 4) Hosted control/API changes

Simplify the control surface:

- remove any route-epoch requirement from event submit
- optionally add future-facing `expected_world_epoch`
- stop exposing route-oriented hosted errors

Expected primary files:

- `crates/aos-node-hosted/src/control/facade.rs`
- `crates/aos-cli/src/commands/world.rs`

## 5) Startup/config/product changes

The product node should:

- fail startup without real Kafka/blobstore config
- stop mentioning route topic in startup logs
- stop offering embedded-routing behavior in the product binary

Expected primary files:

- `crates/aos-node-hosted/src/main.rs`

## 6) Test reset

Tests that currently prove route-topic behavior should be rewritten or deleted.

Expected broad fallout:

- broker plane tests
- hosted worker integration tests
- materializer tests
- shared seam tests around route mismatch

## Deliberate Non-Goals

P16 does **not** try to solve:

- fancy placement policy
- hot-world auto-migration
- zero-downtime world movement
- multi-lane topic strategy
- inter-cluster traffic steering
- multi-region placement

Those are later higher-order routing concerns.

## DoD

P16 is done when:

1. `aos-node-hosted` runs on only `aos-ingress` and `aos-journal`
2. the product node no longer uses or configures a route topic
3. base hosted partition ownership comes only from Kafka consumer-group assignment
4. base hosted partition placement comes only from deterministic `world_id` hashing
5. `route_epoch` is gone from the runtime correctness model
6. `world_epoch` is present as the new world-incarnation primitive
7. checkpoints and replay validate `world_seq` continuity plus `world_epoch` monotonicity
8. materializers no longer depend on route metadata
9. reroute / override behavior is no longer part of the base node contract
10. the resulting node runtime is simpler to explain than the current one

Current implementation status:

- [x] 1
- [x] 2
- [x] 3
- [x] 4
- [x] 5
- [x] 6
- [x] 7
- [x] 8
- [x] 9
- [x] 10

## Desired End State

The desired base-node story after P16 is:

- "Kafka assigns partitions"
- "world ids hash to partitions"
- "workers recover from checkpoint plus journal replay"
- "world epoch fences world incarnations"
- "anything more sophisticated belongs above the node"

That is the right foundation for later routing systems.
