# Parallelism (Speculative Direction)

This chapter outlines future directions for intra-world parallel execution. The v1/v1.1 kernel remains single-threaded by design for determinism, simplicity, and strong replay. These notes document why parallelism might matter later, the challenges, and practical paths that preserve AOS properties.

## Why Parallelism Matters

- Throughput: advance more Run/Cell steps per second when pure compute in reducers/policies grows.
- Latency: reduce head-of-line blocking for long queues of ready steps.
- Hardware utilization: exploit multi-core hosts without sharding everything to multiple worlds.

Most workloads already spend time outside the kernel (adapters/LLM/HTTP), and the single-threaded core interleaves thousands of awaiting Runs/Cells effectively. Parallelism is therefore optional, not foundational.

## Constraints (Non-Negotiables)

- Deterministic replay: same journal + receipts ⇒ identical state.
- Stable identities: effect intent hashes, RNG, IDs must not depend on OS thread timing.
- Governance correctness: capability budgets/policy counters must settle consistently.
- Debuggability: time-travel and the why-graph must remain tractable.

## Challenges vs Single-Thread

- Ordering: parallel execution reorders reads; you must define and record a canonical commit order.
- Shared counters: budgets/rate limits are shared state; parallel decrements need a serial commit.
- Revalidation: invariants checked against a snapshot can be invalidated by a concurrent commit.
- Identity: per-step seeds/ids must be derived deterministically (not from timing).

## Paths Forward

### 1) Ticketed Parallel Execution (recommended if needed)

- Sequencer (single-threaded): assigns a monotonic ticket (event_id) to each ready unit (Run step or Cell event) and a deterministic RNG seed.
- Workers (N threads): compute results in parallel against the read snapshot “as of ticket−1” (conceptual). No I/O; just reducer/plan evaluation.
- Committer (single-threaded): applies results strictly in ticket order; re-checks budgets/policies/invariants at commit; writes the journal; enqueues effects; updates state.
- Fallback: on re-validation failure, re-run serially or re-compute against the current snapshot.

Properties
- Replay: journal already encodes commit order (ticket order). Determinism holds.
- Identity: seeds derived from (event_id, reducer_name, key) keep effect ids stable.
- Isolation: effects are created/intended during compute but gate at commit.

### 2) Lanes/Partitions

- Hash keys into K lanes; each lane has its own scheduler, budgets, and (optional) sub-journal; lanes run on separate threads.
- Cross-lane communication via events (like separate worlds).

Trade-offs
- Simpler concurrency but splits global policy/budget; global queries/snapshots must stitch lanes.
- Essentially “many worlds” in one process; consider just using multiple worlds instead.

### 3) Scale Across Worlds (default, today)

- Keep kernel single-threaded; run more worlds; route by tenant/user/hash(key).
- Pros: trivial determinism and isolation; operationally simple.
- Cons: requires a small control-plane to route DomainIntents and manage shared caps.

## Spec Hooks To Add Now (Parallel-Ready)

- Event IDs: assign every scheduled unit a monotonic event_id; record it in the journal.
- Deterministic RNG: seed per step as seed = H(event_id, reducer_name, key).
- Effect identity: compute intent hash from (kind, params_cbor, cap_name, idempotency_key); derive idempotency_key from stable business data + event_id.
- Budgets at commit: specify budget/policy checks and decrements occur at commit time to avoid races.
- Snapshot semantics: define read snapshot as “state as of ticket−1”; v1 already behaves this way; write it down.
- Journal fields: include event_id and optional lane_id (default 0) for future lanes.

### Journal Hooks (Schema Stub)

To make hooks concrete, journal entries can include `event_id` (monotonic, assigned by the sequencer) and `lane_id` (partition, default 0). Example shapes (CBOR/JSON conceptual):

- StepScheduled { event_id, lane_id, kind: "run_step"|"cell_event", subject: { run_id? , reducer?: Name, key?: bytes } }
- StepCommitted { event_id, lane_id, diffs_ref, domain_events_ref[], effects_intended_ref[] }
- EffectQueued { event_id, lane_id, intent_hash, kind, cap_ref, params_ref }
- ReceiptAppended { lane_id, intent_hash, receipt_ref, status }

Event IDs and lane IDs are recorded to preserve replay order and support future partitioning without changing semantics.

## Validation & Testing (when adding parallelism)

- Replay-or-die: parallel runs must replay serially to byte-identical snapshots.
- Randomized schedulers in CI: vary worker counts and interleavings; assert journal equivalence.
- Fault injection: re-validation failures, adapter delays, budget edge cases.
- Invariant checkers: fail fast if commit would violate declared invariants.

### Scheduling Fairness

Use round-robin across ready Runs and Cells with bounded per-entity work (one step per tick). Optionally add simple weights or priority classes (e.g., aging or per-tenant caps); record scheduling decisions in metrics to diagnose starvation.

## Failure Handling

- On commit conflict: retry compute on latest snapshot or serialize the ticket.
- On invariant failure: journal a deterministic failure event; surface to operator; optionally auto-rollback to prior snapshot (same as today).
- Backpressure: bound ready queues; shed load by deferring low-priority Runs.

## Migration Strategy (if adopted later)

- Start with hooks in v1/v1.1. No behavior change.
- Introduce the sequencer/committer (still single-threaded), then add N workers.
- Begin with validators off, capture metrics; switch on commit-time re-validation; gate rollout per world.

## When To Pursue

- Reducer/plan CPU dominates adapter time (profiling shows core as bottleneck).
- You cannot shard across worlds for organizational reasons but must use many cores for one world.
- You can afford the complexity and testing surface (estimate 10–12 weeks for a solid first implementation).

## Recommendation

- v1/v1.1: keep the kernel single-threaded. Add small parallel-ready hooks (event_id, RNG seeding, commit-time budgets, journal fields).
- If needed later, implement Ticketed Parallel Execution (sequencer + parallel workers + ordered committer). It preserves determinism and fits the event-sourced model with minimal conceptual change.
- Prefer scaling across worlds and lanes over immediate intra-world parallelism; only add intra-world parallelism when profiling demands it.

## References

- Reducers: spec/04-reducers.md (v1), spec/05-cells.md (v1.1 Cells)
- AIR Plans & Manifest: spec/03-air.md (raise_event/await_event, triggers, routing)
- Architecture: spec/02-architecture.md (Compute Layer, Cells)
