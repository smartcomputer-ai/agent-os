# P10: Hosted Product Readiness and Surface Cutover

**Priority**: P10  
**Effort**: High  
**Risk if deferred**: High (the hosted runtime may appear more complete than the actual product
surface and integration coverage justify)  
**Status**: Complete

## Goal

Define what is required to make the hosted Kafka/S3 runtime usable as a real hosted product
surface rather than only a narrow runtime prototype.

The current hosted path now proves important runtime correctness:

- Kafka-backed submission and journal flow
- route epochs and partition ownership
- checkpoint + restart recovery
- event-driven execution for hosted-created worlds

This phase closes the minimum hosted product/API gap for `v0.17`.

## Current Hosted Shape

Today, hosted mode is working for a limited path:

1. start as `worker`, `control`, or `all`
2. create a world from a manifest hash, seed, or fork via ingress
3. submit domain events and governance commands
4. poll command records
5. read manifest / defs / def-get / workspace-resolve
6. read `latest_durable` state via state-get / state-list
7. reroute the world
8. checkpoint and recover after restart

That path is implemented and tested in `crates/aos-node-hosted/tests/log_first_node.rs`.

Important architectural stance now implemented:

- hosted workers keep only local `.aos` cache/state roots for CAS, compiled modules, Wasmtime
  cache, and runtime scratch
- authoritative shared state is Kafka + blobstore only
- hosted world creation enters through ingress and journal, not local filesystem bootstrap
- control and worker are equal deployment roles; `all` is just colocation, not a special
  correctness path
- even in `all`, control must behave as if workers are remote and may only communicate through
  ingress today, and later through projections

What is intentionally deferred beyond this phase:

- projection-backed query serving for gateways and shared read replicas

## Design Stance

The hosted product surface should converge on the same logical model as the new runtime.

Required stance:

- hosted lifecycle operations should enter through ingress and the log-first/runtime seam, not an
  out-of-band side path
- hosted nodes should only require local `.aos` cache/state roots; they must not depend on a local
  authored world root or bootstrap registry
- command submission should be a first-class hosted surface, not just an internal worker ability
- gateway-facing reads should eventually be served by the hosted read/query plane rather than
  reopen-on-read behavior
- integration coverage should reflect the actual supported hosted contract, not only the happy-path
  event demo

## Process Roles

A hosted node/process should be deployable in distinct roles:

- `worker`: owns Kafka-assigned partitions and performs authoritative execution
- `control`: serves the hosted control/API surface without being required to own worker partitions
- `all`: colocates both roles in one process for simpler deployments
- later, `materializer` / `reducer`: consumes committed journal records and builds the derived query
  plane

These are deployment choices, not correctness boundaries.

Required architectural rule:

- authoritative execution stays with workers
- control nodes do not need to own worker partitions and must not rely on in-process worker access
- the later query materializer must remain derived and non-authoritative whether it is colocated or
  deployed separately

Current implementation note:

- today `aos-node-hosted` supports `worker`, `control`, and `all`
- `all` runs separate in-process control and worker runtimes and keeps the same communication rule
  as split deployment
- projections do not exist yet, so the current hosted read APIs are explicitly `latest_durable`
  reopen-on-read behavior rather than the final serving shape

## What Exists Now

Implemented today in hosted mode:

1. hosted role split: `worker`, `control`, and `all`
2. local hosted `.aos` state root for CAS/cache/runtime files
3. create-world by manifest hash, seed, and fork through ingress
4. event submission with `route_epoch`
5. governance command submission and command-record polling
6. manifest / defs / def-get reads from durable world state
7. workspace-resolve from durable world state
8. `latest_durable` state-get / state-list from durable world state
9. reroute with epoch bump
10. immediate initial checkpoint after create-world
11. regular background checkpointing plus kernel-journal compaction
12. checkpoint publication and Kafka/blobstore restart recovery
13. multi-worker partition assignment on the broker-backed path

## Completed In P10 So Far

The following P10 work is now done:

- finished the hosted role split so `control` and `worker` are peer deployment roles
- removed hosted dependence on local `world_root` bootstrap and persisted world registration as the
  primary model
- made hosted create-by-manifest a real ingress-driven path
- added hosted create-by-seed as a first-class ingress-driven path
- added hosted fork by turning fork requests into seed-backed create-world ingress
- added hosted command submission plus command-record polling for governance commands
- added hosted manifest / defs / def-get / workspace-resolve / `latest_durable` state reads
- made workers recover worlds from checkpoint + journal rather than local bootstrap metadata
- added configurable periodic checkpointing by time and committed event count, with kernel-journal
  compaction on checkpoint
- added/updated integration coverage for hosted create, split-role operation, recovery, reroute,
  duplicate submission handling, rollback-on-abort, hosted command polling, hosted seed/fork,
  hosted read APIs, count-based checkpointing, and multi-worker assignment

## Follow-On

Work that remains after P10 is intentionally outside this phase:

- P11 hosted query projections/materialization for scalable gateway/read serving
- deeper rebalance hardening
- stronger producer fencing
- topic retention/compaction validation
- broader object-store lifecycle / blob GC policy

## Scope Boundary

This phase is about making hosted mode a credible product-facing interface.

It is not about:

- projection internals themselves
- rich analytics or diagnostic materializations
- fabric/session/artifact control surfaces
- every future advanced live-read mode

Those belong in later phases or adjacent roadmap items.

## DoD

P10 is complete when:

1. Hosted deployment supports `worker`, `control`, and `all` process shapes.
2. Hosted world creation no longer depends on local-filesystem bootstrap/registration.
3. Hosted command submission exists as a supported control/API surface.
4. Hosted seed-create and fork flows exist as supported lifecycle operations.
5. Hosted manifest / defs / workspace-resolve / `latest_durable` state reads exist as supported
   control/API surfaces.
6. Hosted integration tests cover the main lifecycle, read, command, and recovery paths end-to-end.
7. The roadmap clearly distinguishes runtime correctness work from hosted product-surface
   completeness and from the later projection plane.

## Could Be Added Later

Follow-on hosted product work may later add:

- richer command history projections
- explicit `latest_live` query paths
- hosted shell / diagnostic sessions
- richer operational and trace read surfaces
