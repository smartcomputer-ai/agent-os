# P3: Universe Runtime Plane (Leases, Scheduling, Effects, Timers, Portal, Fork/Seed)

**Priority**: P3  
**Effort**: Very High  
**Risk if deferred**: High (shared persistence exists but worlds remain non-movable/non-operable at scale)  
**Status**: Complete

Implementation status as of 2026-03-08:

- Complete in `crates/aos-fdb`: hosted runtime metadata/protocol surface for worker heartbeats, world leases, guarded world mutations, world catalog scans, persisted `placement_pin`, worker advertised pin sets, durable ready hints, per-world `runtime/ready_state`, per-worker lease indexes, hosted ingress/list-worker control APIs, and minimal snapshot-maintenance readiness derived from journal growth plus unexported cold tail.
- Complete in `crates/aos-fdb-worker`: supervisor heartbeat loop, active-worker discovery, pin-aware rendezvous candidate selection, ready-driven acquisition from durable ready hints plus held-lease indexes, lease acquire/renew/release, hosted world restore, authoritative inbox-to-journal drain for supported ingress, durable effect publish/claim/execute/receipt-ack, durable timer publish/claim/fire/receipt-ack, minimal journal-growth-based snapshot triggering, cold-tail segment export, idle release, and leased persistence wrapper for hosted world mutations.
- Complete in current tests: in-memory runtime coverage for lease fencing, worker discovery, placement pin metadata, ready scheduling helpers, held-lease discovery, hosted `portal.send` delivery, dedupe-GC sweeping, and snapshot-maintenance compaction; real FoundationDB integration coverage for ingress drain, HTTP effect execution, timer delivery, lease failover, expired effect/timer claim recovery, pin-based reassignment, hosted `portal.send` delivery, bounded dedupe-GC release/reuse, snapshot-maintenance compaction, and ready-queue/lease-index control-surface regressions; targeted `aos-fdb` and `aos-fdb-worker` crate suites are green.
- Complete for P3: hosted ingress normalization is complete for `DomainEvent` and `Receipt`, including timer delivery via receipt enqueue. Raw `Inbox` ingress for `routing.inboxes`, `Control` ingress, and direct `TimerFiredIngress` bridging are deferred to later milestones.
- Complete: hosted `portal.send` is implemented for hosted-to-hosted typed event delivery with idempotent destination enqueue and durable receipting.
- Deferred beyond P3: local CAS cache remains future work; fork/seed admin-plane types plus seeded world creation and fork transactions are implemented in persistence backends, but higher-level CLI/HTTP surfaces are intentionally left to later milestones.
- Complete for P3: minimal snapshot/compaction maintenance is live via journal-growth triggers plus cold-tail segment export; richer policy, budgeting, and operator controls are deferred.
- Complete for P3: dedupe retention has bounded terminal-record GC with coarse expiry buckets and opportunistic worker sweeps; dedicated GC worker/mode and broader operational policy are deferred.

## Goal

Ship the first complete hosted runtime plane on top of P2 persistence:

1. Worlds can run on any worker with lease fencing.
   - Subject to optional worker pin eligibility.
2. All ingress is durable and replay-safe.
3. Effects and timers execute through durable queues and return receipts through inbox.
4. Cross-world messaging is durable, idempotent, and auditable.
5. World fork/seed from snapshots is cheap and deterministic.

This milestone turns hosted storage into a working distributed runtime.

Constraint for later follow-on work:

- The single-world execution core established here should remain reusable via `aos-world` by later non-hosted modes, even though P3 itself is hosted-only.
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

- Implement a single hosted worker program/crate named `aos-fdb-worker` for the first hosted runtime pass.
- In v1, each worker fulfills all runtime responsibilities for worlds it currently holds: world execution, effect dispatch/receipt handling, and timer delivery.
- `aos-fdb-worker` is the hosted/FDB worker process. It is not the primary local/filesystem runtime entrypoint in v1.
- Shared per-world execution logic should live in `aos-world`.
- Local/filesystem execution should be driven by `aos-cli` on top of `aos-world`, not by a permanent `aos-host` crate boundary.
- Shared adapters should live in `aos-effect-adapters`.
- Intended dependency direction is:
  - `aos-world -> aos-effect-adapters`
  - `aos-fdb-worker -> aos-world`
  - `aos-cli -> aos-world`
- The same hosted worker program should remain capable of running in narrower modes later (for example `worker` vs `adapter` args), but that split is not required for the first implementation.
- A dedicated orchestrator is not required for correctness in v1. Workers may coordinate by observing active worker heartbeats, collaboratively choosing candidate worlds, and relying on lease fencing for safety.
- World placement pinning is in scope for v1 as a scheduling eligibility filter, not as a second authority model.
- Desired assignment / placement policy is an optional later layer on top of the same lease protocol, not a prerequisite for P3.
- Reuse should happen at the `aos-world` / single-world engine boundary, not by forcing hosted worker orchestration concerns onto the local/filesystem runtime path.

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

### [x] 1) Lease protocol with fencing epoch

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

### [x] 1a) World placement pinning

World metadata:

- `placement_pin?: text`

Worker heartbeat metadata:

- `pins: set<text>`

Scheduling semantics:

- Persisted world pinning remains optional.
- Effective scheduling pin is `placement_pin` when set, otherwise `"default"`.
- Workers that should run neutral/unpinned worlds must advertise the `"default"` pin.
- Workers may advertise multiple pins.
- A pinned world is eligible to run on any active worker whose advertised pins contain that pin.
- If no active worker advertises a world's effective pin, no worker should attempt to acquire that world's lease.

Rules:

1. Pinning only constrains lease acquisition candidate selection; lease fencing remains the correctness boundary.
2. Rendezvous hashing must run only across workers eligible for the world's effective pin.
3. Workers may be configured to serve only explicitly pinned worlds by omitting `"default"` from their advertised pins.
4. Changes to worker pins or world pinning affect future placement decisions without introducing a new authority path.
5. If a worker becomes ineligible for a world it currently holds, it should stop renewing and release as soon as practical.

### [x] 2) Worker lifecycle and host loop

Current implementation status:

- Complete: heartbeat worker presence, discover active workers, filter by effective placement pin, pin-aware rendezvous hashing, acquire lease, restore hosted world, authoritative inbox drain for supported ingress, publish effect intents, claim/execute effect work, claim/fire timers, minimal snapshot maintenance with journal-growth triggers plus cold-tail segment export, renew lease, idle release, fenced stop on lease loss, and immediate release when a held world becomes pin-ineligible.
- Complete in current integration coverage: lease failover, expired effect-claim recovery, expired timer-claim recovery, and pin-driven reassignment across workers on real FoundationDB.
- Complete for P3: raw `routing.inboxes`, `Control` ingress, and direct `TimerFiredIngress` bridging are deferred, and richer snapshot/compaction policy remains later follow-on work.

Worker world lifecycle:

1. Heartbeat worker presence into hosted runtime metadata.
2. Discover active workers and ready worlds.
3. For each ready world, compute the effective placement pin, filter the active worker set to eligible workers, and run stable rendezvous hashing over that eligible set.
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
- Worker heartbeats should include advertised pins so every worker can derive the same eligible worker set for a world.
- Rendezvous hashing is preferred for v1 because membership changes move the fewest worlds and avoid a central scheduler.
- Ready hints are advisory; workers must re-check authoritative world state before lease acquisition.
- World pinning is an eligibility filter over the active worker set, not a replacement for rendezvous hashing.
- Lease fencing remains the sole correctness boundary. Candidate ownership only reduces contention.

Run loop budgets:

- `max_inbox_batch`
- `max_tick_steps_per_cycle`
- `max_effects_per_cycle`
- `max_timers_per_cycle`
- `max_cycle_wall_ms` (operational guardrail only; deterministic outputs remain journal-defined)

Runtime-shape constraints:

- worker/runtime code should key off `(universe_id, world_id)` rather than a filesystem world root
- `aos-fdb-worker` is a hosted runtime process, not a commitment to unify the full worker loop with filesystem/local runtime behavior
- hosted storage remains the only authoritative persistence plane in P3
- the hosted worker should reuse the single-world execution core from `aos-world` rather than introducing a second world runner

### [ ] 2a) Local CAS caching

- CAS content is immutable and hash-addressed, so workers should be allowed to cache it locally.
- CAS caching is independent of world lease lifetime; releasing a world lease should not flush process-local CAS cache.
- First implementation can use a read-through process-local cache keyed by `(universe_id, hash)`.
- Eviction is operational only (for example size-bounded LRU); correctness never depends on cache residency.
- Journal heads, inbox cursors, leases, ready state, and other mutable runtime metadata must not be treated as cache-authoritative.
- A later disk-backed CAS cache is acceptable, but not required for first implementation.
- Delivery of the first concrete worker-local CAS cache is deferred to `v0.20-infra/p4-blob-storage-and-cas-caching.md`.

### [x] 3) Durable ingress normalization path

Current implementation status:

- Complete: durable inbox enqueue remains authoritative, supported hosted ingress is translated into canonical journal records and committed through `drain_inbox_to_journal_guarded`, and integrated effect/timer workers feed receipts back through the same inbox path.
- Complete for P3: raw `Inbox` ingress for `routing.inboxes`, `Control` ingress, and direct `TimerFiredIngress` bridging are deferred. Timers currently re-enter through receipt ingress because that is the existing kernel timer receipt model.

All external inputs converge into `InboxItem` and are journaled only by lease holder.

Ingress producers:

- API/control (`event-send`, `receipt-inject`) producing `DomainEvent` / `Receipt` inbox items
- Integrated workers (effect receipts)
- Integrated workers (timer delivery)
- Fabric adapter
- Optional external inbox relays (deferred with raw `routing.inboxes` support)

Normalization requirements:

1. Validate schema/shape before enqueue where possible.
2. Canonicalize once at journal-append boundary.
3. Correlation identifiers preserved (`intent_hash`, `event_hash`, `correlation_id`).

### [x] 4) Durable effect dispatch runtime

Current implementation status:

- Complete: effect intents are published to durable pending queues, claimed by the lease holder for the world, executed through routed adapters, acknowledged back into world inbox via `ReceiptIngress`, and requeued from inflight on claim expiry.
- Complete for P3: dedicated shard-ownership mode is not required for this milestone, and queue dedupe GC / richer retry bookkeeping are deferred.

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
- `cap_name`
- `params_inline_cbor?`
- `params_ref?`
- `params_size?`
- `params_sha256?`
- `idempotency_key`
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

### [x] 5) Durable timer runtime

Current implementation status:

- Complete: `timer.set` intents are persisted as durable due records, claimed at or after due time, converted into timer receipts, enqueued back into world inbox, and requeued from inflight on claim expiry.
- Complete for P3: delivery currently reuses receipt ingress rather than a distinct `TimerFiredIngress` journal path, matching the existing kernel timer receipt model.

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

### [x] 6) Portal cross-world messaging (`portal.send`)

Add plan-only effect kind:

- `portal.send`

P3 scope note:

- In this milestone, `portal.send` only targets worlds that live inside the same hosted persistence plane.
- Bridging to embedded worlds is deferred.
- In this milestone, `portal.send` is a typed event delivery primitive, not a raw inbox forwarding primitive.
- Typed event delivery is implemented in `aos-fdb-worker` on top of durable effect execution plus destination inbox enqueue with per-destination dedupe.

Proposed built-ins:

- `sys/PortalSendParams@1`
  - `dest_universe?: uuid` (default same universe)
  - `dest_world: uuid`
  - `schema: Name`
  - `value_cbor: bytes`
  - `correlation_id?: text`
- `sys/PortalSendReceipt@1`
  - `status: "ok" | "already_enqueued" | "error"`
  - `message_id: hash` (default intent_hash)
  - `dest_world: uuid`
  - `enqueued_seq?: bytes`

Delivery protocol:

1. Compute `message_id = intent_hash` unless explicitly overridden by policy.
2. Transaction checks `u/<u>/w/<dest>/portal/dedupe/<message_id>`.
3. If exists, return `already_enqueued`.
4. Else enqueue the typed domain event on the destination world and set dedupe key.
5. Return receipt to sender via normal receipt ingress.

Ordering:

- Per destination world: inbox sequence order.
- Cross-world: no global order guarantee.
- Causal metadata (`from_world`, `from_height`) attached for optional reducer-level enforcement.

### [x] 7) Dedupe retention and GC

Dedupe records are correctness-critical but must not grow without bound.

Current implementation status:

- Complete: effect, timer, and portal terminal dedupe records carry `completed_at_ns` plus `gc_after_ns`.
- Complete: each dedupe family has a matching coarse-bucket GC index in `aos-fdb`, and both memory + FoundationDB backends expose bounded sweep APIs.
- Complete: `aos-fdb-worker` performs opportunistic bounded sweeps each supervisor pass.
- Complete for P3: retention windows are simple persistence config values, and dedicated GC worker roles, richer policy engines, and broader operational scheduling/metrics remain follow-on work.

Required shape:

- terminal status records store `completed_at_ns` and `gc_after_ns`
- each dedupe family has a matching GC index keyed by coarse expiry bucket
- deletion is always best-effort background work and never in the correctness-critical fast path

Retention rules:

- effect dispatch dedupe must outlive receipt enqueue and any expected runtime-level retries
- timer dedupe must outlive successful timer-fire enqueue and any expected reaper retries
- portal dedupe must outlive destination enqueue visibility and sender retry windows
- exact retention windows are operational policy, but the storage model must support bounded cleanup

### [x] 8) World fork and seed from snapshot

Add hosted world management primitives:

- `world.create_from_seed(universe, request)`
- `world.fork(universe, request)`

Implementation status:

- Complete in `crates/aos-fdb` and in-memory parity: persisted admin-plane request/result types, seeded world creation, fork-from-snapshot, lineage persistence, CAS-root validation, and real integration coverage for seeded create/fork.
- Complete for P3: persistence/admin-plane fork and seed primitives, default pending-effect clearing, and hosted restore coverage are implemented. Public CLI/HTTP management surfaces and richer import/export ergonomics are deferred.

Design clarification:

- A `WorldSeed` is an admin-plane descriptor, not a new CAS object class.
- The seed points at immutable replay roots that must already exist in universe CAS:
  - `baseline.snapshot_ref`
  - `baseline.manifest_hash`
- Mutable hosted state for the new world is persisted in world metadata/index keys:
  - `meta`
  - `snapshot/by_height/<height>`
  - `baseline/active`
  - `journal/head`
  - empty inbox/runtime state
- This keeps replay roots immutable/content-addressed while keeping world identity, placement, and lineage in hosted metadata.

Suggested persisted API shapes:

- `CreateWorldRequest { world_id?, seed, placement_pin?, created_at_ns }`
- `WorldSeed { baseline: SnapshotRecord, seed_kind, imported_from? }`
- `ForkWorldRequest { src_world_id, src_snapshot, new_world_id?, placement_pin?, forked_at_ns, pending_effect_policy }`
- `WorldLineage = genesis | import | fork`

Create transaction:

1. Validate seed baseline as promotable (`receipt_horizon_height == height`).
2. Verify `snapshot_ref` and `manifest_hash` already exist in universe CAS.
3. Allocate `world_id` if absent.
4. Fail if the world already exists.
5. Write `snapshot/by_height/<baseline.height> -> baseline`.
6. Write `baseline/active -> baseline`.
7. Write `journal/head = baseline.height`.
8. Write `meta` / catalog metadata with `manifest_hash`, `active_baseline_height`, optional `placement_pin`, `created_at_ns`, and `lineage`.
9. Initialize empty runtime state (`ready_state`, empty inbox cursor/lease/effect/timer state).

Fork transaction:

1. Allocate `new_world_id`.
2. Set `new_world.baseline/active = src_snapshot_record`.
3. Set `new_world.journal/head = src_snapshot.height`.
4. Initialize empty inbox cursor and runtime metadata.
5. Set lineage metadata (`src_universe_id`, `src_world_id`, `src_snapshot_ref`, `src_height`, `forked_at_ns`).

Fork selector semantics:

- `active_baseline`
- `by_height(height)`
- `by_ref(snapshot_ref)`

Default placement behavior:

- `world.create_from_seed` uses the explicit `placement_pin` when supplied, otherwise leaves the world unpinned.
- `world.fork` uses the explicit `placement_pin` when supplied, otherwise inherits the source world's current `placement_pin`.

Default effect policy on fork:

- Clear pending effect dispatch state and idempotency caches for the new world.
- Rationale: avoid duplicating external side effects when branching.

### [x] 9) Scheduling and readiness model

Current implementation status:

- Complete: workers derive placement from active heartbeats plus world metadata, including effective placement pin filtering before rendezvous hashing, immediate release on pin ineligibility, and reassignment via fresh lease acquisition by an eligible worker.
- Complete: durable ready hints and per-world `runtime/ready_state` records exist, including maintenance pressure from journal growth and unexported cold tail, and the supervisor acquires worlds from `ready/*` plus `lease/by_worker/*` rather than broad world-catalog scans.

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
- Candidate worker selection should use rendezvous hashing across the current active worker set after filtering to workers eligible for the world's effective pin.
- Membership changes should not require rewriting per-world desired placement in the common case.
- Optional explicit assignment may later override rendezvous ownership for drain, maintenance, or operator-directed single-worker steering.

### [x] 10) Hosted runtime metadata keyspace

Current implementation status:

- Complete: worker heartbeat records, world catalog scan records, per-world lease records, placement pin fields on world metadata/catalog records, per-world `runtime/ready_state` including maintenance state, `ready/*` hint records, and per-worker `lease/by_worker/*` secondary indexes exist in `aos-fdb`.
- Complete for P3: assignment override records are not implemented because explicit assignment remains deferred.

Universe / worker records:

- `u/<u>/workers/by_id/<worker_id> -> WorkerHeartbeatRecord`
- `u/<u>/workers/by_expiry/<expiry_bucket>/<worker_id> -> ()` (optional operational index)
- `u/<u>/ready/<priority>/<shard>/<world_id> -> ReadyHint`

Existing world metadata records used by runtime placement:

- `u/<u>/w/<w>/meta -> WorldMeta` (`placement_pin?: text`)
- `u/<u>/worlds/<world_id> -> WorldMetaCatalogRecord` (`placement_pin?: text`, used for scans/scheduling)

Primary per-world records:

- `u/<u>/w/<w>/runtime/lease -> LeaseRecord`
- `u/<u>/w/<w>/runtime/ready_state -> ReadyState`
- `u/<u>/w/<w>/runtime/assignment -> AssignmentRecord` (optional later override)
- `u/<u>/w/<w>/portal/dedupe/<message_id> -> PortalDedupeRecord`
- `u/<u>/portal/dedupe_gc/<gc_bucket>/<world_id>/<message_id> -> ()`

Secondary indexes:

- `u/<u>/lease/by_worker/<worker>/<world> -> LeaseIndexRecord`
- `u/<u>/assign/by_worker/<worker>/<world> -> AssignmentIndexRecord` (optional later override index)

Rules:

- per-world records are authoritative
- per-worker indexes are maintained transactionally as secondary indexes where present
- worker heartbeats are operational membership records, not authority records, but they must carry the worker's advertised pins, including `"default"` when the worker is willing to run neutral/unpinned worlds
- watches on worker heartbeat, assignment, or ready-state keys are latency hints only; workers must recover correctly by scanning durable state

### [x] 11) Operational APIs (internal)

Current implementation status:

- Complete: `heartbeat_worker`, `list_active_workers`, `set_world_placement_pin`, `acquire_lease`, `renew_lease`, `release_lease`, `enqueue_ingress(world, item)`, `list_worker_worlds(worker)`, `world_fork(...)`, `world_create_from_seed(...)`, and the ready/list operations needed by the current supervisor loop.
- Complete for P3: the internal control surface required by the hosted worker is in place; richer observability and broader management surfaces remain follow-on work.

Required internal control surface:

- `heartbeat_worker(worker)`
- `list_active_workers(universe)`
- `set_world_placement_pin(world, pin?)`
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
2. Include the worker's advertised pin set in the heartbeat payload.
3. Maintain optional expiry-bucket index for efficient cleanup/scans.
4. Commit.

Guarantees:

- Active worker set can be reconstructed from durable heartbeats.
- Pin eligibility can be reconstructed from the same durable active worker set.
- Expired workers naturally age out without a central coordinator.

### Protocol A: `acquire_lease(world, worker)`

1. Read current lease, world metadata, and optional desired assignment.
2. Validate the worker is eligible for the world's effective placement pin.
3. Validate assignment allows worker if an override is configured.
4. If lease absent/expired:
   - write new lease with `epoch = old_epoch + 1`
   - update `lease/by_worker/<worker>/<world>`
   - set expiry
5. Commit.

Guarantees:

- Monotonic epoch fencing.
- Single active holder.
- Ineligible workers cannot acquire a lease for a pinned world.

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

### Protocol F: `portal_send(dest_world, message_id)`

1. Check `u/<u>/w/<dest>/portal/dedupe/<message_id>`.
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

Current coverage status:

- Complete on real FoundationDB: lease failover, effect completion with durable queueing/receipt re-ingress, timer durability across worker recovery, expired effect/timer claim requeue and recovery, pin-driven reassignment, hosted `portal.send` idempotent delivery to a destination world, bounded dedupe-GC release/reuse for effect, timer, and portal dedupe keys, minimal snapshot-maintenance snapshot-plus-segment-export behavior, hosted fork boot from a promoted baseline snapshot with pending external state cleared by default, and explicit replay-parity coverage for the remaining meaningful pre-publish crash boundaries.
- Deferred beyond P3: exhaustive injected crash-matrix coverage across every internal boundary.

1. Lease failover:
   - worker A loses lease, worker B resumes from baseline + tail with byte-identical end state.
2. Effects exactly-once logical completion:
   - duplicate dispatch attempts still yield one terminal receipt per intent hash.
3. Timer durability:
   - timer scheduled pre-crash still fires post-restart/migration.
4. Portal dedupe:
   - repeated hosted `portal.send` retries enqueue once at destination.
5. Fork semantics:
   - forked world boots exactly at source snapshot state; pending external effects are cleared by default.

### Chaos/failure injection matrix

Covered in P3:

1. After hosted inbox drain and world advance, before effect publish.
2. After hosted inbox drain and world advance, before timer publish.

Assertions:

- No lost ingress.
- No stale lease mutation.
- Deterministic replay parity holds.

Deferred beyond P3:

1. After lease acquire.
2. Internal atomic sub-boundaries inside guarded inbox drain.
3. After effect claim before external call.
4. After external call before receipt enqueue.
5. After timer claim before fire enqueue.

### Performance targets (initial)

1. Lease renewal p95 under healthy cluster: < 50 ms.
2. Inbox-to-journal append latency p95: < 100 ms under nominal load.
3. Effect dispatch to receipt-ingress p95 (excluding external SLA): < 200 ms.

## Rollout Plan

### Phase 1: Single runtime binary, integrated worker loops

- `aos-fdb-worker` hosts many worlds against the hosted persistence plane.
- The same process runs world execution plus effect/timer loops for worlds it holds.
- No dedicated orchestrator is required.
- Local/filesystem runtime paths remain on `aos-cli` over `aos-world`; they are not forced through `aos-fdb-worker` in this phase.

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

1. Complete: lease protocol with epoch fencing is implemented and enforced on world mutations.
2. Complete: worker membership heartbeat plus collaborative claiming are implemented.
3. Complete: worker runtime loop can host multiple worlds, survive failover, and release idle worlds after a bounded warm window.
4. Deferred beyond P3: process-local CAS caching as an optimization independent of lease lifetime.
5. Complete: runtime startup/operation is keyed by persistence identity rather than filesystem world-root assumptions.
6. Complete: durable effect dispatch queue with worker-integrated adapter execution and receipt re-ingress is live.
7. Complete: durable timer path is live and migration-safe.
8. Complete: `portal.send` is live for hosted-to-hosted typed event delivery with idempotent destination enqueue.
9. Complete for P3: bounded dedupe-retention GC exists for effect, timer, and portal dedupe families; broader GC operationalization remains deferred.
10. Complete for P3: minimal journal-growth snapshot triggering and cold-tail segment export are live; richer snapshot/compaction policy and operational controls remain deferred.
11. Complete: world fork/seed from snapshot with explicit default pending-effect policy is implemented and covered.
12. Complete for P3: targeted failure-injection and replay-parity coverage exists for failover, expired effect/timer claim recovery, portal idempotency, snapshot-maintenance compaction, and the remaining meaningful pre-publish crash boundaries; the exhaustive crash matrix is deferred.
13. Complete for P3: core internal ops APIs required by the hosted worker are in place; richer observability and broader control-surface endpoints remain deferred.
14. Complete: hosted worker concerns live in `aos-fdb-worker`, while reusable single-world execution lives in `aos-world` and shared adapters live in `aos-effect-adapters`.

## Explicitly Out of Scope

- Public API authn/authz productization.
- Multi-region consensus or geo-replication.
- Billing/chargeback and tenant quota enforcement policies.
- Complex policy DSL upgrades beyond current cap/policy primitives.
- Embedded-universe implementation and hosted/embedded bridge semantics.
