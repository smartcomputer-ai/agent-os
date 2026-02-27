# P3: Universe Runtime Plane (Leases, Scheduling, Effects, Timers, Fabric, Fork/Seed)

**Priority**: P3  
**Effort**: Very High  
**Risk if deferred**: High (shared persistence exists but worlds remain non-movable/non-operable at scale)  
**Status**: Proposed

## Goal

Ship the first complete hosted runtime plane on top of P2 persistence:

1. Worlds can run on any worker with lease fencing.
2. All ingress is durable and replay-safe.
3. Effects and timers execute out-of-process via durable queues and return receipts through inbox.
4. Cross-world messaging is durable, idempotent, and auditable.
5. World fork/seed from snapshots is cheap and deterministic.

This milestone turns hosted storage into a working distributed runtime.

## Dependencies

- Requires `v0.11-infra/p2-hosted-persistence-plane.md` merged.
- Requires P1 semantics already active (`BlobEdge`, baseline rules, snapshot root completeness).

## Non-Goals (P3)

- Multi-region active-active runtime.
- Byzantine trust model between internal services.
- Full tenant quota/billing engine.
- Rich workflow placement optimization (ML scheduling, cost-aware routing).
- Public internet control-plane APIs and auth hardening.

## Runtime Roles (In Scope)

### 1) Orchestrator

- Maintains worker inventory and world desired placement.
- Writes desired assignment and revocation intents.
- Does not execute world logic.

### 2) World Worker

- Acquires/renews world lease.
- Restores world from baseline + journal tail.
- Drains inbox to journal, advances kernel, emits effect intents.
- Schedules snapshots and compaction triggers.
- Stops world execution immediately when lease renewal fails.

### 3) Adapter Worker Pool

- Consumes global effect dispatch queue.
- Executes external effects (HTTP/LLM/email/payments/fabric).
- Enqueues receipts back into destination world inbox.

### 4) Timer Worker

- Scans due timers.
- Enqueues timer-fired ingress into destination world inbox.
- Ensures at-least-once durable delivery with dedupe.

## Scope (Now)

### 1) Lease protocol with fencing epoch

Lease record (per world):

- `holder_worker_id: text`
- `epoch: u64` (strictly increasing fencing token)
- `expires_at_ns: u64`

Assignment record:

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

1. Watch for assignments targeting worker.
2. Attempt `acquire_lease`.
3. Restore runtime (`active_baseline + tail`).
4. Enter run loop:
   - drain inbox to journal
   - tick kernel until idle or budget
   - publish effect intents to global queue
   - snapshot based on policy
   - renew lease
5. On reassignment/revoke/renew failure:
   - stop loop
   - flush in-memory state as needed
   - release local handles

Run loop budgets:

- `max_inbox_batch`
- `max_tick_steps_per_cycle`
- `max_effects_per_cycle`
- `max_cycle_wall_ms` (operational guardrail only; deterministic outputs remain journal-defined)

### 3) Durable ingress normalization path

All external inputs converge into `InboxItem` and are journaled only by lease holder.

Ingress producers:

- API/control (`event-send`, `receipt-inject`)
- Adapter workers (effect receipts)
- Timer worker
- Fabric adapter
- Optional external inbox relays

Normalization requirements:

1. Validate schema/shape before enqueue where possible.
2. Canonicalize once at journal-append boundary.
3. Correlation identifiers preserved (`intent_hash`, `event_hash`, `correlation_id`).

### 4) Global effect dispatch runtime

Queue keys from P2:

- `u/<u>/effects/pending/<seq> -> EffectDispatchItem`
- `u/<u>/effects/inflight/<seq> -> EffectInFlightItem`
- `u/<u>/effects/dedupe/<intent_hash> -> DispatchStatus`

Dispatch item fields:

- `universe_id`
- `world_id`
- `intent_hash`
- `effect_kind`
- `params_cbor`
- `origin_name`
- `policy_context_hash`
- `enqueued_at_ns`

Claim/execute protocol:

1. Adapter worker claims pending item by moving it to inflight with lease timeout.
2. Execute adapter call.
3. Build typed receipt event.
4. Enqueue `ReceiptIngress` into world inbox.
5. Mark dedupe status complete and remove inflight.

Crash recovery protocol:

- Reaper scans inflight entries with expired claim timeout and requeues to pending.
- Dedupe key on `intent_hash` prevents duplicate terminal deliveries.

Retry semantics:

- Adapter-level retries allowed before receipt emission.
- Runtime-level retries create new attempt records but same `intent_hash`.
- Terminal receipt states: `ok | error | timeout`.

### 5) Durable timer runtime

Timer queue keys from P2:

- `u/<u>/timers/due/<deliver_at_ns>/<intent_hash> -> TimerDueItem`
- `u/<u>/timers/inflight/<intent_hash> -> TimerClaim`
- `u/<u>/timers/dedupe/<intent_hash> -> DeliveredStatus`

Timer protocol:

1. `timer.set` intent is persisted and enqueued as due record.
2. Timer worker claims due item at/after due timestamp.
3. Enqueue `TimerFiredIngress` into world inbox.
4. Mark dedupe delivered, clear inflight.

Semantics:

- At-least-once enqueue to inbox; dedupe prevents duplicate logical delivery for same intent.
- World migration does not affect timer durability.

### 6) Fabric cross-world messaging (`fabric.send`)

Add plan-only effect kind:

- `fabric.send`

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
2. Transaction checks `world/<dest>/fabric_dedupe/<message_id>`.
3. If exists, return `already_enqueued`.
4. Else enqueue inbox item on destination world and set dedupe key.
5. Return receipt to sender via normal receipt ingress.

Ordering:

- Per destination world: inbox sequence order.
- Cross-world: no global order guarantee.
- Causal metadata (`from_world`, `from_height`) attached for optional reducer-level enforcement.

### 7) World fork and seed from snapshot

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

### 8) Scheduling and readiness model

Readiness signal:

- world is ready if:
  - inbox has unread items, or
  - local kernel has runnable work, or
  - lease renewal/snapshot maintenance is due.

Suggested queue:

- `u/<u>/ready/<priority>/<world_id> -> ReadyHint`

Rules:

- Ready queue is hint-based and idempotent (duplicate hints acceptable).
- Authoritative truth remains world state in journal/inbox metadata.

### 9) Operational APIs (internal)

Required internal control surface:

- `assign_world(world, worker)`
- `unassign_world(world)`
- `acquire_lease(world, worker)`
- `renew_lease(world, worker, epoch)`
- `release_lease(world, worker, epoch)`
- `enqueue_ingress(world, item)`
- `list_worker_worlds(worker)`
- `world_fork(...)`
- `world_create_from_seed(...)`

All APIs must be idempotent and return typed errors.

## Transaction Protocols (Normative)

### Protocol A: `acquire_lease(world, worker)`

1. Read current lease and desired assignment.
2. Validate assignment allows worker (if configured).
3. If lease absent/expired:
   - write new lease with `epoch = old_epoch + 1`
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
2. For each intent, check dedupe key.
3. Insert pending queue records for new intents.
4. Commit.

Guarantees:

- No double-dispatch for same intent hash.

### Protocol D: `adapter_claim_execute_ack(effect_seq)`

1. Move pending entry to inflight with claim TTL.
2. Execute adapter.
3. Enqueue receipt into destination inbox.
4. Mark dedupe complete, delete inflight.
5. Commit ack records.

Guarantees:

- Durable handoff from effect queue to world inbox.

### Protocol E: `timer_claim_fire_ack(intent_hash)`

1. Claim due timer.
2. Enqueue timer-fired ingress.
3. Mark timer dedupe delivered.
4. Remove due/inflight timer keys.

Guarantees:

- Timer firing survives crashes/retries.

### Protocol F: `fabric_send(dest_world, message_id)`

1. Check destination dedupe key.
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
2. Adapter crashes after external call before receipt enqueue:
   - inflight reaper retries with dedupe guard.
3. Timer worker crash between claim and enqueue:
   - expired claim reaper retries.

### Idempotency anchors

- Effect dispatch: `intent_hash`.
- Fabric delivery: `message_id` (default `intent_hash`).
- Timer delivery: `intent_hash`.
- Journal append: `expected_head` + single writer.

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

### Phase 1: Single orchestrator, single worker process hosting many worlds

- Leases active but no contention.
- Adapter and timer workers can be in-process sidecars.

### Phase 2: Multi-worker lease handoff

- Enable reassignment and failover.
- Validate fencing under induced network partitions/timeouts.

### Phase 3: Dedicated adapter/timer pools and fabric routing

- Separate worker classes.
- Enable cross-world messaging for selected tenants/workloads.

### Phase 4: Hardening

- Backpressure controls.
- Worker admission and load shedding.
- Operational dashboards and runbooks.

## Deliverables / DoD

1. Lease protocol with epoch fencing implemented and enforced on world mutations.
2. Worker runtime loop can host multiple worlds and survive failover.
3. Durable effect dispatch queue with adapter workers and receipt re-ingress is live.
4. Durable timer worker path is live and migration-safe.
5. `fabric.send` effect with idempotent destination enqueue is implemented.
6. World fork/seed from snapshot is implemented with explicit default effect policy.
7. Failure-injection test suite passes with replay parity guarantees.
8. Internal ops APIs and observability metrics/logging are in place.

## Explicitly Out of Scope

- Public API authn/authz productization.
- Multi-region consensus or geo-replication.
- Billing/chargeback and tenant quota enforcement policies.
- Complex policy DSL upgrades beyond current cap/policy primitives.

