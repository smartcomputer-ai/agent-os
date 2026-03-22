# P1: Postgres-Backed Hosted Routing and Metadata Plane

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: Medium (simple hosted mode can exist without this, but full multi-universe
hosted routing, placement, and admin semantics remain incomplete)  
**Status**: Planned

## Goal

Design the full routed hosted control plane for multi-universe deployments using a distributed
metadata database such as Postgres.

This is the system that should support:

- multiple universes in one hosted deployment
- explicit universe/world metadata
- handles and uniqueness
- managed routing and placement
- worker/control plane separation
- richer lifecycle/admin semantics

This is **not** the simple single-universe hosted mode from P14.

## Why This Is Separate

The simple hosted mode and the routed hosted mode have different requirements.

P14 simple mode:

- one configured universe
- one ingress topic
- one journal topic
- no routing control plane
- no Postgres dependency

This item:

- many universes
- explicit admin metadata
- managed routing/placement
- shared control-plane state
- Postgres allowed and expected

Trying to do both in one design makes the simple mode too heavy and the routed mode too vague.

## Primary Stance

For the full routed system:

- Kafka remains authoritative for execution history:
  - ingress
  - journal
- blobstore remains authoritative for blobs/checkpoints/secrets
- Postgres becomes the authoritative control-plane metadata and routing store
- SQLite remains only a local cache/materialization store

That means Postgres should own:

- universe metadata
- world metadata
- handle indexes and uniqueness
- lifecycle/admin state
- routing/placement metadata
- optionally shared query projections if we later want them there

It should **not** own:

- world journal truth
- checkpoint blob bytes
- CAS blob payloads

## Problem Statement

The current hosted runtime has enough execution machinery to run many worlds, and the low-level
partitioning already keys by `(universe_id, world_id)`. But that is not enough for a real routed
multi-universe deployment.

Missing concerns include:

- durable universe metadata
- explicit world metadata beyond runtime registrations
- handle lookup/uniqueness
- desired-vs-actual lifecycle/admin state
- routing overrides and placement ownership
- worker placement and rebalancing semantics
- control-plane visibility into world location/state

Without an explicit metadata/routing plane, multi-universe hosted either becomes ad hoc or pushes
too much product logic into Kafka internals and local caches.

## What Postgres Should Be Used For

## 1) Universe Metadata

Postgres should store universe records and related indexes.

Needed data:

- `universe_id`
- handle
- admin lifecycle/status
- created/updated timestamps
- optional archival/deletion metadata

Required operations:

- create/get/list universes
- lookup by handle
- patch handle/admin metadata
- delete/archive transitions

## 2) World Metadata

Postgres should store explicit world records.

Needed data:

- `(universe_id, world_id)`
- handle
- lineage
- admin lifecycle/status
- placement pin
- active baseline pointers
- latest known journal head summary
- created/updated timestamps

This becomes the durable source for:

- world get/list
- handle lookup
- lifecycle/admin behavior
- fork/seed provenance

## 3) Routing Metadata

This is the major addition beyond simple mode.

Needed data:

- current route epoch
- journal topic / ingress topic set
- partition override or partition assignment
- placement owner / lease
- desired placement vs actual placement
- reroute history / intent metadata as needed

This should replace the current "strange routing system" with an explicit control-plane model.

## 4) Lifecycle Coordination

Postgres should also hold desired-vs-actual lifecycle state for worlds.

Examples:

- `active`
- `pausing`
- `paused`
- `deleting`
- `deleted`
- `archiving`
- `archived`

Workers should reconcile against this metadata rather than inventing lifecycle meaning from local
runtime state alone.

## 5) Optional Shared Projections

The first routed version does not need Postgres-backed query projections if local SQLite
materialization remains sufficient.

But Postgres is a reasonable later home for:

- shared world summaries
- global world/universe listing
- maybe shared control-plane query indexes

This is optional, not required for the first routing cut.

## Intended Runtime Shape

The routed hosted system should look like this:

- `control plane`
  - owns admin APIs
  - reads/writes Postgres metadata
  - reads/writes blobstore metadata as needed
  - submits ingress to Kafka
- `worker plane`
  - consumes Kafka ingress/journal
  - executes worlds
  - writes journal/checkpoints/blobstore artifacts
  - reconciles routing/lifecycle state from Postgres
- `materializer/query plane`
  - consumes journal
  - writes local SQLite or shared query stores

This is a real distributed control plane, unlike P14.

## Routing Model

The routing design should support:

- deterministic default partitioning
- explicit partition overrides
- route epoch fencing
- worker reassignment / placement changes
- reroute operations that are durable and auditable

Recommended stance:

- Postgres is the source of truth for route metadata
- Kafka submissions still carry route epoch
- workers still fence stale submissions using route epoch
- reroute is a control-plane metadata update, not an implicit runtime trick

## Why Postgres Helps

Postgres is the right fit here because it solves the metadata problems that are awkward in a
Kafka/blobstore-only design:

- uniqueness constraints
- handle indexes
- transactional metadata updates
- desired-vs-actual lifecycle state
- operator-friendly inspection and admin tooling

This is the class of state where a distributed database is justified.

## Relationship To Non-Routed Mode

The routed mode must not replace the simple mode.

Both modes should exist:

### Simple hosted mode

- no Postgres
- one configured universe
- no routed control plane
- close to local mode

### Routed hosted mode

- Postgres-backed metadata/routing
- many universes
- managed placement and reroute semantics
- richer admin surface

The codebase should share runtime and worker core logic where possible, but these are two distinct
operating modes.

## Non-Goals

- replacing Kafka as the execution/journal authority
- replacing blobstore for CAS/checkpoints/secrets
- forcing Postgres into the simple hosted mode
- making Postgres the source of truth for world state

## DoD

1. Hosted routed mode has explicit Postgres-backed universe metadata.
2. Hosted routed mode has explicit Postgres-backed world metadata.
3. Hosted routed mode has explicit Postgres-backed route/placement metadata.
4. Handle lookup and uniqueness are enforced in the metadata plane.
5. Lifecycle/admin desired-vs-actual state is explicit and durable.
6. Workers and control plane can run separately while sharing the same metadata authority.
7. The simple single-universe non-routed hosted mode remains available without Postgres.

## Follow-On

Likely follow-on items after this:

- richer scheduling/placement policies
- archive/restore semantics
- optional shared query serving in Postgres
- commercial secret-manager integration in routed hosted deployments
