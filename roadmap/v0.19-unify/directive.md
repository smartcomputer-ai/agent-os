# Refactor Directive

(do not edit, only edited by human user)

## Background
We recently moved to storing effects on a Kafka ingress and journal. And we split the runtime into "local" and "hosted" versions (aos-node-local and aos-node-hosted).

We also made major refactors regarding how effects and world input sequencing works (see roadmap/v0.18-execution/architecture/).

But I think I made a larger mistake in with the architecture: we've made it too complex for AOS being only an early, experimental version.

I think the main culprit is to look at Kafka support as a fused together whole. Specifically, building everything around the Kafka topic "aos-ingress".

The recent advancements in aos-node-hosted around async effects with world slices, flush buffers, etc., showed me that the way we ingress is separate from the way we process events and persist them.

The simplification therefore is to move towards a single node structure that can run many worlds, but has switchable backends.

## Goals
Specifically here is what I want to do:
- Simplify state reads to work more like the local hosted node: read directly from the hot world state. Currently both local and hosted workers have all their worlds open and hot anyhow (because of timers).
    - latest reads are served in-process from the unified node, against hot active worlds, we move completely off the materialized reads that we have now
    - the old hosted control/worker split is no longer the default read path
    - that is much closer to aos-node-local
- For now, completely remove projections and materializations. This is a super advanced use-case that should likely be handled in the future by dedicated materializers that consumer the journal topic rebuild the world and persist materializations somewhere. Right now. This is a major complexity sink that gives us relatively little.
- For now, also remove Kafka aos-ingress completely. (or make it optional) This is also an advanced use-case with questionable utility.
- Create a unified staging layer/bridge based on aos-node-hosted slices that still keeps the unified flush semantics which we invented to synchronize ingress and journal, but use it to (optionally) signal to any waiting producer, such as a producer that wants write and read semantics from http endpoint or something.
- Move all external "ingress" of domain events to simple http post right now, that sends directly to worker instead of first writing to an ingress topic.
- Do not tie world discovery to aos-ingress, but rather to a world discovery backend that is pluggable too. In the case of a worker using kafka ingress, the world ids are discovered from the topic partitions as they are now. In the future normal mode, the discovery should basically (by default) just run all worlds, or a select set of world ids via worker config input.
   - move away from submission_offset and rename it to something like accept_token or accept_seq that can be implemented differently depending on the backend.
- The list of available worlds should come from the persisted checkpoints in the blobstore (not some sqlite table)
- As a consequence, the checkpoint system needs to move away from per-partition checkpoints and move to per-world checkpoints.
- The journal backend should be switchable between kafka and sqlite.
- With all that in place, we can unify the node into a single node code base and stop distinguishing between embedded/local and hosted.
- two secret modes should be supported, blobs store hosted vault (encrypted etc), or only .env file (for testing)

Note: breaking changes are fully acceptable and expected. The goal is to aim for the ideal architecture directly and not to maintain any compatibility shims or structures.

## Phases

1) Unify the read/control surface first. Make hosted control read directly from HostedWorkerRuntime/active worlds, not HostedProjectionStore, and rename submission_offset to a backend-neutral accept_token or accept_seq in types.rs. This proves the target UX early and lets us delete materialized reads from facade.rs instead of designing around them.

2) Split ingress from staging/flush semantics. Extract the “accept input -> enqueue work -> stage slices -> flush -> optionally notify waiter” flow into a backend-neutral seam, keeping the current hosted slice machinery. Default mode should accept HTTP directly into that path; Kafka ingress becomes an optional producer of the same internal work items. 

3) Refactor discovery and checkpoints to be world-based. Replace journal-partition discovery and PartitionCheckpoint with per-world checkpoint records plus a pluggable discovery backend. The key migration is moving bootstrap/activation logic in worlds.rs and checkpoint publishing in checkpoint.rs off partition ownership and onto explicit world inventory from blobstore/config/cli.

4) Introduce switchable journal backends. Once ingress and discovery are abstracted, define a true journal backend seam and implement kafka and sqlite behind it, instead of today’s broker-vs-embedded Kafka split in kafka/mod.rs. This is where you adapt the local SQLite frame log in embedded/sqlite.rs into the new backend model rather than trying to bolt it onto the current hosted services.

5) Collapse the product/crate split and delete obsolete seams. After the runtime model works with direct reads, direct HTTP ingress, world checkpoints, and switchable journals, merge the aos-node-local and aos-node-hosted product surfaces into one node binary/crate shape, keeping only backend/config differences. Then remove projections/materializer, hosted-only control services, and compatibility paths that exist only to support the old split.