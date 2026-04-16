# v0.18 External Execution

This milestone defines async external execution as a core runtime seam rather than a fabric
feature.

Architecture note:

- `execution-architecture.md`
  - current cycle-driven runtime shape,
  - desired actor/channel-driven execution architecture,
  - world actor, Kafka ingress task, effect daemon, timer daemon, and continuation ingress model.

The recut is:

1. `p1-open-effect-lifecycle-and-owner-executor-seam.md`
  - owner-side semantic contract,
  - durable open-work lifecycle,
  - `intent_hash` as the authoritative effect/open-work id,
  - continuation admission, replay, and quiescence.
  - status: implemented in broad pass; naming cleanup remains.
2. `p2-start-eligibility-expiry-and-staleness.md`
  - execution runtime / effect daemon seam,
  - independent discovery, reconciliation, start, observe, and settle behavior,
  - executor operational state versus authoritative world state.
  - status: runtime split implemented; restart/reattach protocol and deployment policy remain.
3. `p3-runtime-execution-strategies.md`
  - start eligibility and stale-start as downstream policy,
  - embedded / sidecar / hosted deployment mappings,
  - optional operator labels if retained.
  - status: next primary cut.

Tentative follow-on note:

4. `p20-open-work-backpressure-and-overload-control.md`
   - optional overload-control design note,
   - authoritative open-work budgeting versus executor-local semaphores,
   - why this is orthogonal to the core async execution seam.

Operational follow-up note:

5. `hosted-throughput-regression.md`
   - measured single-world hosted throughput collapse after the v0.18 hosted refactor,
   - profiler evidence that the dominant cost is the per-event durable flush fence,
   - temporary `hosted-prof --unsafe-no-flush` comparison and resulting conclusions.

Hosted throughput follow-up notes:

6. `p1-hosted-projection-publication-modes.md`
   - define `projection_commit_mode = inline | background`,
   - move projection publication out of the authoritative commit path by default,
   - preserve repairability from journal/checkpoint rather than volatile post-commit state.
7. `p2-max-uncommitted-slices-per-world.md`
   - define `max_uncommitted_slices_per_world = 1..N`,
   - preserve per-slice deterministic admission while allowing same-world speculative staging,
   - spell out the ordering and restart invariants required for hot-world throughput.
8. `p3-hosted-flush-trigger-and-local-continuation-batching.md`
   - tie flush pressure to per-world staging-capacity backpressure plus `max_delay`,
     `max_slices`, and `max_bytes`,
   - define `max_local_continuation_slices_per_flush`,
   - improve local continuation fairness without changing
     authoritative commit rules.
   - includes a short "Current Implementation" section documenting the shipped defaults and
     behavioral caveats.

Fabric-specific host/session/artifact/log/secrets work moves to `roadmap/v0.19-fabric/`.

Primary rules:

- worlds journal open work first,
- `intent_hash` is the authoritative effect/open-work id,
- executors may run independently of the owner loop,
- stream frames and terminal receipts re-enter only through owner admission.
