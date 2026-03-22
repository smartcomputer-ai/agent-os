# P5: Hot Worlds, Routing Overrides, and Lanes

**Priority**: P5  
**Effort**: High  
**Risk if deferred**: Medium/High (the first Kafka design may lock itself into one-topic simplicity without a credible pressure-relief path)  
**Status**: Complete for current scope (routing overrides and pause-and-reroute semantics are implemented; lane and placement extensions are deferred)

## Goal

Define how the log-first runtime handles:

- routing overrides
- world migration

without abandoning the simplified Kafka/S3 model.

## Completed In Code

Implemented on the experimental branch:

1. `partition_override` as an explicit route override.
2. Reroute as an epoch-bumping fence.
3. Pause-and-reroute semantics in the sense that stale submissions are fenced after reroute.
4. The same route-override semantics now exist on both the embedded and hosted runtime paths.

Deferred follow-up:

1. Lane topics.
2. Placement classes.
3. Automatic hot-world handling or migration policy.

## Problem Statement

The runtime needs enough routing structure to keep ownership and migration correct:

1. worlds need a stable default route
2. operators need an explicit override path when a world must move
3. reroutes need a fence so stale submissions and stale receipts stop being admitted

## Routing Overrides

The runtime should support explicit route assignment and route overrides:

- default initial route assignment by stable hash
- optional topic override for specific worlds
- optional explicit `partition_override` for specific worlds
- assignments and overrides written as latest-value records in the compacted route topic

This lets the system isolate elephant worlds from their neighbors without redesigning the whole
runtime.

## Migration Model

The first migration target should be pause-and-reroute, not zero-downtime magic.

Suggested flow:

1. quiesce ingress for the world
2. write a fresh checkpoint/snapshot to S3
3. write a new compacted route record with the new `(ingress_topic, journal_topic, partition_override?, epoch)`
4. resume routing new submissions to the new paired address
5. let the new owner restore from S3 and continue

This is simple, understandable, and sufficient for the early system.

## Internal Routing Requirement

Not only the ingress gateway must honor routes.

The runtime itself must route correctly for:

- `portal.send`
- effect receipt submissions
- timer-firing submissions
- fabric receipt submissions
- administrative commands

That is why the route directory is a runtime concern, not just an API-gateway concern.

The first implementation target should assume this directory is materialized from the compacted
route topic rather than a hidden side database.

## Current Scope Boundary

For `v0.17`, the needed routing scope is smaller than the original longer-term pressure-relief
story:

- default stable-hash placement
- explicit `partition_override`
- compacted route metadata
- `route_epoch` fencing
- pause-and-reroute as the first migration model

That is the minimum needed for correctness and operator-directed movement.

## Out of Scope

1. Full automatic hot-world detection and migration policy.
2. Placement-class scheduling policy.
3. Multi-lane topic strategy.
4. Multi-region placement.
5. Fully transparent zero-pause live migration.

## DoD

1. Route overrides are an explicit part of the design.
2. Pause-and-reroute migration is the first declared migration strategy.
3. The route directory is treated as runtime-owned metadata rather than an API-gateway-only
   concern.
4. The roadmap makes `route_epoch` fencing part of the correctness model.

## Could Be Added Later

If operational pressure justifies it later, follow-on work could add:

- lane topics as a controlled extension
- placement classes such as `gpu`, `regulated`, or `hot`
- automatic hot-partition or hot-world migration policy
- broader scaling guidance for truly hot single worlds, including when to split them into multiple
  coordinated worlds
