# P3: Universe Runtime Plane (Leases, Scheduling, Effects, Timers, Fabric, Fork/Seed)

**Priority**: P3  
**Effort**: Very High  
**Risk if deferred**: High (shared persistence exists but worlds remain non-movable/non-operable at scale)  
**Status**: Proposed

## Goal

Ship the first complete hosted runtime plane on top of P2 persistence:

1. Worlds can run on any worker with lease fencing.
2. All ingress is durable and replay-safe.
3. Effects and timers execute through durable queues and return receipts through inbox.
4. Cross-world messaging is durable, idempotent, and auditable.
5. World fork/seed from snapshots is cheap and deterministic.

This milestone turns hosted storage into a working distributed runtime.

Constraint for later follow-on work:

- The single-world execution core established here should remain reusable by later non-hosted modes, even though P3 itself is hosted-only.
- P3 does not require the outer hosted worker process to be shared with filesystem/local runtime paths.

## Dependencies

- Requires `v0.20-infra/p2-hosted-persistence-plane.md` merged.
- Requires P1 semantics already active (`BlobEdge`, baseline rules, snapshot root completeness).

## Non-Goals (P3)

- Multi-region active-active runtime.
- Byzantine trust model between internal services.
- Full tenant quota/billing engine.
- Rich workflow placement optimization (ML scheduling, cost-aware routing).
- A mandatory central orchestrator for first hosted runtime bring-up.
- Public internet control-plane APIs and auth hardening.
- Embedded-universe runtime implementation.
- Live communication or shared CAS between embedded and hosted universes.
- Export/import movement between embedded and hosted modes.

## Design Stance (v1)

- Implement a single hosted worker program/crate named `aos-worker` for the first hosted runtime pass.
- In v1, each worker fulfills all runtime responsibilities for worlds it currently holds: world execution, effect dispatch/receipt handling, and timer delivery.
- `aos-worker` is the hosted/FDB worker process. It is not the primary local/filesystem runtime entrypoint in v1.
- Local/filesystem execution should continue to use `aos-host` as the embeddable single-world engine, with `aos-cli` invoking that path directly where appropriate.
- The same hosted worker program should remain capable of running in narrower modes later (for example `worker` vs `adapter` args), but that split is not required for the first implementation.
- A dedicated orchestrator is not required for correctness in v1. Workers may coordinate by observing active worker heartbeats, collaboratively choosing candidate worlds, and relying on lease fencing for safety.
- Desired assignment / placement policy is an optional later layer on top of the same lease protocol, not a prerequisite for P3.
- Reuse should happen at the `WorldHost` / single-world engine boundary, not by forcing hosted worker orchestration concerns onto the local/filesystem runtime path.

## Runtime Shape (In Scope)

### 1) Integrated Worker

- Acquires/renews world lease.
- Restores world from baseline + journal tail.
- Drains inbox to journal, advances kernel, emits effect intents.
- Claims and executes effect work for held worlds.
- Claims and fires timers for held worlds.
- Schedules snapshots and compaction triggers.
- Stops world execution immediately when lease renewal fails.

### 2) Optional Placement Controller (deferred)

- May later maintain worker inventory and desired placement.
- May later write assignment and revocation intents.
- Is not required for first correctness or first rollout.

## Scope (Now)

### 1) Lease protocol with fencing epoch

Lease record (per world):

- `holder_worker_id: text`
- `epoch: u64` (strictly increasing fencing token)
- `expires_at_ns: u64`

Default operational timings for the first rollout:

- `lease_ttl = 20s`
- `renew_every = 5s`
- `idle_release_after = 10s`

Optional later assignment record:

- `desired_worker_id?: text`
- `priority?: nat`
- `reason?: text`

Rules:

1. At most one valid lease at a time per world.
2. Worker must include current `epoch` in all mutating world operations.
3. Store rejects writes with stale epoch.
4. Lease renewal must happen before `expires_at_ns - renewal_margin`.
5. On failed renewal, worker transitions world to `fenced` and stops processing.

### 2) Worker lifecycle and host loop

Worker world lifecycle:

1. Heartbeat worker presence into hosted runtime metadata.
2. Discover active workers and ready worlds.
3. Compute candidate ownership for ready worlds using stable rendezvous hashing over the active worker set.
4. Attempt `acquire_lease`.
5. Restore runtime (`active_baseline + tail`).
6. Enter run loop:
   - drain inbox to journal
   - tick kernel until idle or budget
   - publish effect intents to durable queues
   - claim/execute effect work for held worlds
   - claim/fire due timers for held worlds
   - snapshot based on policy
   - renew lease
7. Once the world becomes quiescent, keep the host warm for `idle_release_after`.
8. If no new work appears during that idle window, release lease and unload local world state.
9. On renew failure or fencing:
   - stop loop immediately
   - drop local authority for the world
   - release local handles as cleanup only

Worker membership / claiming notes:

- Workers need the active worker set, not just a worker count.
- Rendezvous hashing is preferred for v1 because membership changes move the fewest worlds and avoid a central scheduler.
- Ready hints are advisory; workers must re-check authoritative world state before lease acquisition.
- Lease fencing remains the sole correctness boundary. Candidate ownership only reduces contention.

Run loop budgets:

- `max_inbox_batch`
- `max_tick_steps_per_cycle`
- `max_effects_per_cycle`
- `max_timers_per_cycle`
- `max_cycle_wall_ms` (operational guardrail only; deterministic outputs remain journal-defined)

Runtime-shape constraints:

- worker/runtime code should key off `(universe_id, world_id)` rather than a filesystem world root
- `aos-worker` is a hosted runtime process, not a commitment to unify the full worker loop with filesystem/local runtime behavior
- hosted storage remains the only authoritative persistence plane in P3
- the hosted worker should reuse `WorldHost` as the execution core rather than introducing a second world runner

### 2a) Local CAS caching

- CAS content is immutable and hash-addressed, so workers should be allowed to cache it locally.
- CAS caching is independent of world lease lifetime; releasing a world lease should not flush process-local CAS cache.
- First implementation can use a read-through process-local cache keyed by `(universe_id, hash)`.
- Eviction is operational only (for example size-bounded LRU); correctness never depends on cache residency.
- Journal heads, inbox cursors, leases, ready state, and other mutable runtime metadata must not be treated as cache-authoritative.
- A later disk-backed CAS cache is acceptable, but not required for first implementation.

### 3) Durable ingress normalization path

All external inputs converge into `InboxItem` and are journaled only by lease holder.

Ingress producers:

- API/control (`event-send`, `receipt-inject`)
- Integrated workers (effect receipts)
- Integrated workers (timer delivery)
- Fabric adapter
- Optional external inbox relays

Normalization requirements:

1. Validate schema/shape before enqueue where possible.
2. Canonicalize once at journal-append boundary.
3. Correlation identifiers preserved (`intent_hash`, `event_hash`, `correlation_id`).

### 4) Durable effect dispatch runtime

Queue keys from P2:

- `u/<u>/effects/pending/<shard>/<seq> -> EffectDispatchItem`
- `u/<u>/effects/inflight/<shard>/<seq> -> EffectInFlightItem`
- `u/<u>/effects/dedupe/<intent_hash> -> DispatchStatus`

Dispatch item fields:

- `shard`
- `universe_id`
- `world_id`
- `intent_hash`
- `effect_kind`
- `params_inline_cbor?`
- `params_ref?`
- `params_size?`
- `params_sha256?`
- `origin_name`
- `policy_context_hash`
- `enqueued_at_ns`

Claim/execute protocol:

1. Worker claims pending item from an assigned shard by moving it to inflight with claim timeout.
2. Execute adapter call.
3. Build typed receipt event.
4. Enqueue `ReceiptIngress` into world inbox.
5. Mark dedupe status complete and remove inflight.

v1 ownership note:

- In the first rollout, workers should normally execute effect items for worlds they currently hold.
- Dedicated effect-only shard ownership is a later optional mode split, not a prerequisite for P3.

Crash recovery protocol:

- Reaper scans inflight entries with expired claim timeout and requeues to pending.
- Dedupe key on `intent_hash` prevents duplicate terminal deliveries.

Retry semantics:

- Adapter-level retries allowed before receipt emission.
- Runtime-level retries create new attempt records but same `intent_hash`.
- Terminal receipt states: `ok | error | timeout`.

Sharding rules:

- `shard` is derived from a stable hash of `intent_hash` with a fixed configured shard count.
- Workers claim one or more shards and scan only those prefixes.
- There is no global ordering guarantee across effect shards.
- Initial rollout may use `shard_count = 1`; the shard-aware key layout exists to avoid redesign once hosted load requires parallel queue ranges.

### 5) Durable timer runtime

Timer queue keys from P2:

- `u/<u>/timers/due/<shard>/<time_bucket>/<deliver_at_ns>/<intent_hash> -> TimerDueItem`
- `u/<u>/timers/inflight/<shard>/<intent_hash> -> TimerClaim`
- `u/<u>/timers/dedupe/<intent_hash> -> DeliveredStatus`

Timer protocol:

1. `timer.set` intent is persisted and enqueued as due record.
2. Worker claims due item at/after due timestamp from an assigned shard and time bucket.
3. Enqueue `TimerFiredIngress` into world inbox.
4. Mark dedupe delivered, clear inflight.

Semantics:

- At-least-once enqueue to inbox; dedupe prevents duplicate logical delivery for same intent.
- World migration does not affect timer durability.
- There is no global total order across timer shards; only due-time ordering within a shard scan.
- In the first rollout, workers should normally fire timers for worlds they currently hold.

Recurring schedule note:

- `P3` only implements durable delivery for one-shot `timer.set` intents.
- Recurring schedules are deferred until after the first hosted runtime pass, once leases, wakeups, migration, and one-shot durability are proven end to end.
- When added later, the schedule engine should own recurring metadata and materialize one-shot timer instances onto the existing due queue, rather than replacing the timer runtime itself.

### 6) Fabric cross-world messaging (`fabric.send`)

Add plan-only effect kind:

- `fabric.send`

P3 scope note:

- In this milestone, `fabric.send` only targets worlds that live inside the same hosted persistence plane.
- Bridging to embedded worlds is deferred.

Proposed built-ins:

- `sys/FabricSendParams@1`
  - `dest_universe?: uuid` (default same universe)
  - `dest_world: uuid`
  - `mode: "typed_event" | "inbox"`
  - `schema?: Name` (required for typed_event)
  - `value_cbor?: bytes` (typed payload)
  - `inbox?: text` (required for inbox mode)
  - `payload_cbor?: bytes` (inbox payload)
  - `headers?: map<text,text>`
  - `correlation_id?: text`
- `sys/FabricSendReceipt@1`
  - `status: "ok" | "already_enqueued" | "error"`
  - `message_id: hash` (default intent_hash)
  - `dest_world: uuid`
  - `enqueued_seq?: bytes`

Delivery protocol:

1. Compute `message_id = intent_hash` unless explicitly overridden by policy.
2. Transaction checks `u/<u>/w/<dest>/fabric/dedupe/<message_id>`.
3. If exists, return `already_enqueued`.
4. Else enqueue inbox item on destination world and set dedupe key.
5. Return receipt to sender via normal receipt ingress.

Ordering:

- Per destination world: inbox sequence order.
- Cross-world: no global order guarantee.
- Causal metadata (`from_world`, `from_height`) attached for optional reducer-level enforcement.

### 7) Dedupe retention and GC

Dedupe records are correctness-critical but must not grow without bound.

Required shape:

- terminal status records store `completed_at_ns` and `gc_after_ns`
- each dedupe family has a matching GC index keyed by coarse expiry bucket
- deletion is always best-effort background work and never in the correctness-critical fast path

Retention rules:

- effect dispatch dedupe must outlive receipt enqueue and any expected runtime-level retries
- timer dedupe must outlive successful timer-fire enqueue and any expected reaper retries
- fabric dedupe must outlive destination enqueue visibility and sender retry windows
- exact retention windows are operational policy, but the storage model must support bounded cleanup

### 8) World fork and seed from snapshot

Add hosted world management primitives:

- `world.create(universe, seed_snapshot?)`
- `world.fork(src_world, src_snapshot)`

Fork transaction:

1. Allocate `new_world_id`.
2. Set `new_world.baseline/active = src_snapshot_record`.
3. Set `new_world.journal/head = src_snapshot.height`.
4. Initialize empty inbox cursor and runtime metadata.
5. Set lineage metadata (`parent_world_id`, `parent_snapshot_hash`, `forked_at_height`).

Default effect policy on fork:

- Clear pending effect dispatch state and idempotency caches for the new world.
- Rationale: avoid duplicating external side effects when branching.

### 9) Scheduling and readiness model

Readiness signal:

- world is ready if:
  - inbox has unread items, or
  - local kernel has runnable work, or
  - lease renewal/snapshot maintenance is due.

Suggested queue:

- `u/<u>/ready/<priority>/<shard>/<world_id> -> ReadyHint`

Rules:

- Ready queue is hint-based and idempotent (duplicate hints acceptable).
- Authoritative truth remains world state in journal/inbox metadata.
- `shard` for ready hints should be derived from a stable hash of `world_id`.
- Ready hints may be stale; workers must re-check authoritative world state before acting.
- Ready writes should be de-duplicated or rate-limited where practical so a single hot world does not become a write hotspot.
- Initial rollout may use `shard_count = 1` here as well.
- Candidate worker selection should use rendezvous hashing across the current active worker set.
- Membership changes should not require rewriting per-world desired placement in the common case.
- Optional explicit assignment may later override rendezvous ownership for drain, maintenance, or operator-directed pinning.

### 10) Hosted runtime metadata keyspace

Universe / worker records:

- `u/<u>/workers/by_id/<worker_id> -> WorkerHeartbeatRecord`
- `u/<u>/workers/by_expiry/<expiry_bucket>/<worker_id> -> ()` (optional operational index)
- `u/<u>/ready/<priority>/<shard>/<world_id> -> ReadyHint`

Primary per-world records:

- `u/<u>/w/<w>/runtime/lease -> LeaseRecord`
- `u/<u>/w/<w>/runtime/ready_state -> ReadyState`
- `u/<u>/w/<w>/runtime/assignment -> AssignmentRecord` (optional later override)
- `u/<u>/w/<w>/fabric/dedupe/<message_id> -> DeliveredStatus`
- `u/<u>/w/<w>/fabric/dedupe_gc/<gc_bucket>/<message_id> -> ()`

Secondary indexes:

- `u/<u>/lease/by_worker/<worker>/<world> -> LeaseIndexRecord`
- `u/<u>/assign/by_worker/<worker>/<world> -> AssignmentIndexRecord` (optional later override index)

Rules:

- per-world records are authoritative
- per-worker indexes are maintained transactionally as secondary indexes where present
- worker heartbeats are operational membership records, not authority records
- watches on worker heartbeat, assignment, or ready-state keys are latency hints only; workers must recover correctly by scanning durable state

### 11) Operational APIs (internal)

Required internal control surface:

- `heartbeat_worker(worker)`
- `list_active_workers(universe)`
- `acquire_lease(world, worker)`
- `renew_lease(world, worker, epoch)`
- `release_lease(world, worker, epoch)`
- `enqueue_ingress(world, item)`
- `list_worker_worlds(worker)`
- `world_fork(...)`
- `world_create_from_seed(...)`

Deferred/optional later control surface:

- `assign_world(world, worker)`
- `unassign_world(world)`

All APIs must be idempotent and return typed errors.

## Transaction Protocols (Normative)

### Protocol 0: `heartbeat_worker(worker)`

1. Upsert worker heartbeat with fresh expiry and operational metadata.
2. Maintain optional expiry-bucket index for efficient cleanup/scans.
3. Commit.

Guarantees:

- Active worker set can be reconstructed from durable heartbeats.
- Expired workers naturally age out without a central coordinator.

### Protocol A: `acquire_lease(world, worker)`

1. Read current lease and optional desired assignment.
2. Validate assignment allows worker if an override is configured.
3. If lease absent/expired:
   - write new lease with `epoch = old_epoch + 1`
   - update `lease/by_worker/<worker>/<world>`
   - set expiry
4. Commit.

Guarantees:

- Monotonic epoch fencing.
- Single active holder.

### Protocol B: `renew_lease(world, worker, epoch)`

1. Read lease record.
2. Require `holder == worker && epoch == lease.epoch`.
3. Extend expiry.
4. Commit.

Guarantees:

- Stale workers cannot renew.

### Protocol C: `publish_effect_intents(world, epoch, intents[])`

1. Verify lease epoch.
2. For each intent, compute `shard`, check dedupe key, and externalize large params to CAS if needed.
3. Insert pending queue records for new intents.
4. Commit.

Guarantees:

- No double-dispatch for same intent hash.

### Protocol D: `effect_claim_execute_ack(shard, effect_seq)`

1. Move pending entry to inflight with claim TTL.
2. Execute adapter.
3. Enqueue receipt into destination inbox.
4. Mark dedupe complete, delete inflight.
5. Commit ack records.

Guarantees:

- Durable handoff from effect queue to world inbox.

### Protocol E: `timer_claim_fire_ack(shard, intent_hash)`

1. Claim due timer.
2. Enqueue timer-fired ingress.
3. Mark timer dedupe delivered.
4. Remove due/inflight timer keys.

Guarantees:

- Timer firing survives crashes/retries.

### Protocol F: `fabric_send(dest_world, message_id)`

1. Check `u/<u>/w/<dest>/fabric/dedupe/<message_id>`.
2. If absent, enqueue destination inbox message and set dedupe key.
3. Return receipt payload with enqueue seq.

Guarantees:

- Idempotent cross-world delivery under retries.

## Failure Semantics and Safety

### Fencing and split-brain prevention

- Every world mutation path requires lease epoch check.
- Any stale epoch write attempt is rejected.
- Worker must halt world loop on first fence failure.

### Crash windows covered

1. Worker crashes after journal append before effect publish:
   - recovery scans tail + queued effects and repopulates publish queue.
2. Worker crashes after external call before receipt enqueue:
   - inflight reaper retries with dedupe guard.
3. Worker crashes between timer claim and enqueue:
   - expired claim reaper retries.

### Idempotency anchors

- Effect dispatch: `intent_hash`.
- Fabric delivery: `message_id` (default `intent_hash`).
- Timer delivery: `intent_hash`.
- Journal append: `expected_head` + single writer.

Deferred boundary:

- P3 does not define durable delivery across hosted/embedded authority boundaries.
- Any future embedded/hosted bridge must be specified as an explicit relay/export surface, not assumed by these protocols.

## Testing and Validation

### Deterministic integration tests

1. Lease failover:
   - worker A loses lease, worker B resumes from baseline + tail with byte-identical end state.
2. Effects exactly-once logical completion:
   - duplicate dispatch attempts still yield one terminal receipt per intent hash.
3. Timer durability:
   - timer scheduled pre-crash still fires post-restart/migration.
4. Fabric dedupe:
   - repeated `fabric.send` retries enqueue once at destination.
5. Fork semantics:
   - forked world boots exactly at source snapshot state; pending external effects are cleared by default.

### Chaos/failure injection matrix

Inject crash/restart at each boundary:

1. After lease acquire.
2. After inbox drain before append.
3. After append before cursor advance.
4. After effect claim before external call.
5. After external call before receipt enqueue.
6. After timer claim before fire enqueue.

Assertions:

- No lost ingress.
- No stale lease mutation.
- Deterministic replay parity holds.

### Performance targets (initial)

1. Lease renewal p95 under healthy cluster: < 50 ms.
2. Inbox-to-journal append latency p95: < 100 ms under nominal load.
3. Effect dispatch to receipt-ingress p95 (excluding external SLA): < 200 ms.

## Rollout Plan

### Phase 1: Single runtime binary, integrated worker loops

- `aos-worker` hosts many worlds against the hosted persistence plane.
- The same process runs world execution plus effect/timer loops for worlds it holds.
- No dedicated orchestrator is required.
- Local/filesystem runtime paths remain on `aos-host` / `aos-cli`; they are not forced through `aos-worker` in this phase.

### Phase 2: Multi-worker lease handoff

- Enable collaborative claiming and failover across multiple workers.
- Validate fencing under induced network partitions/timeouts.

### Phase 3: Optional mode split and placement policy

- Allow the same hosted worker program to start in narrower modes if needed.
- Optional explicit assignment / drain / maintenance policy can be layered on top.
- Enable cross-world messaging for selected tenants/workloads.

### Phase 4: Hardening

- Backpressure controls.
- Worker admission and load shedding.
- Operational dashboards and runbooks.

## Deliverables / DoD

1. Lease protocol with epoch fencing implemented and enforced on world mutations.
2. Worker membership heartbeat plus collaborative claiming are implemented.
3. Worker runtime loop can host multiple worlds, survive failover, and release idle worlds after a bounded warm window.
4. Process-local CAS caching is in place as an optimization independent of lease lifetime.
5. Runtime startup/operation is keyed by persistence identity rather than filesystem world-root assumptions.
6. Durable effect dispatch queue with worker-integrated adapter execution and receipt re-ingress is live.
7. Durable timer path is live and migration-safe.
8. `fabric.send` effect with idempotent destination enqueue is implemented for hosted-to-hosted delivery.
9. World fork/seed from snapshot is implemented with explicit default effect policy.
10. Failure-injection test suite passes with replay parity guarantees.
11. Internal ops APIs and observability metrics/logging are in place.
12. Hosted worker concerns live in `aos-worker`, while `aos-host` remains the reusable single-world engine for local/filesystem and test flows.

## Explicitly Out of Scope

- Public API authn/authz productization.
- Multi-region consensus or geo-replication.
- Billing/chargeback and tenant quota enforcement policies.
- Complex policy DSL upgrades beyond current cap/policy primitives.
- Embedded-universe implementation and hosted/embedded bridge semantics.
