# Hosted Checklist

Concrete gap checklist for `aos-node-hosted` against
`roadmap/v0.18-execution/architecture/hosted-architecture.md`.

Current read: the active hosted worker core is substantially closer to the target center, but the
cutover is still incomplete. The compiled library now has a synchronous per-worker scheduler core,
per-world mailboxes, a mailbox-driven `serve_forever()` path, a real hosted
`HostControl` / `WorldControl` submission path, and consumer-group assignment as the default worker
topology. The major hosted cutover items in this checklist are now landed: staged-slice
revocation cleanup is explicit, command read-side updates no longer advance ahead of the durable
fence, checkpoint publication rides the normal slice/journal fence, and the full
`aos-node-hosted` package test surface is green again. The remaining work is follow-up cleanup and
hardening rather than a known checklist blocker on the hosted center itself.

## Landed Or Mostly Landed

- [x] Remove the old `WorldHost` / owner-loop center from the active compiled worker path.
  The active worker is now `worker::{core,runtime,supervisor}` rather than the old
  partition/owner execution center.
- [x] Normalize work as ingress-backed or local continuation work.
  `WorkItem`, `IngressToken`, `CompletedSlice`, and `DurableDisposition` exist and are used by the
  active runtime.
- [x] Make the synchronous kernel service slice explicit.
  The active worker services one world at a time, drains kernel work until idle, stages a slice,
  then flushes it durably.
- [x] Move opened-effect handling to post-commit classification.
  Inline internal effects, owner-local timers, and external async intents are separated after
  durable append.
- [x] Rehydrate runtime work from durable kernel state on reopen/restart.
  Pending timers and external async work are reconstructed from kernel pending-receipt state.
- [x] Eliminate per-world Tokio runtime creation in the active worker core.
  The current worker no longer spins a current-thread runtime per world cycle.
- [x] Restore projection publication on the active worker path.
  Durable flush finalization and direct create-world seeding now publish world-meta, workspace,
  and cell projection records again, and reopen logic preserves or resets projection tokens based
  on continuity against the active baseline and projected head.
- [x] Promote the hosted library surface back onto the active crate.
  `bootstrap`, `control`, `materializer`, `services`, and `test_support` are compiled and exposed
  again against the new worker runtime instead of being dormant files outside the active lib path.
- [x] Pull Kafka transport ownership out of `HostedWorkerCore` and remove the broker-side ingress
  prebuffer from the active path.
  The broker consumer, assignment state, pause/resume flow control, and consumer-group metadata
  now live in a dedicated ingress driver owned alongside the runtime/supervisor rather than inside
  `HostedKafkaBackend` or the worker core, and scheduler mailbox ingress now carries raw
  `IngressRecord`s directly into the scheduler-owned per-partition pending queue.

## Highest-Priority Remaining Work

- [x] Reconnect the real hosted control surface to the active `HostControl` / `WorldControl` path.
  Hosted submission now preserves `HostControl` and `WorldControl` as first-class submission
  classes instead of depending on an in-process runtime shortcut. The normal `control` and `all`
  broker roles publish durable ingress records again, and the active worker scheduler now handles
  `HostControl::CreateWorld` explicitly before normal world admission while `WorldControl`
  continues through the world-owned ingress path.
- [x] Wire a real scheduler mailbox and continuous worker loop.
  `SchedulerMsg`, `Assignment`, `FlushTick`, and `CheckpointTick` are now wired into a live
  `serve_forever()` path. The active hosted center now has one supported mailbox-driven worker
  loop rather than a separate single-pass compatibility entrypoint.
- [x] Replace pass-driven timer and continuation polling with wakeup-driven scheduling in the live
  worker path.
  `serve_forever()` now forwards async effect continuations and timer wakeups into
  `SchedulerMsg::LocalInput` instead of depending on opportunistic supervisor-pass polling. The
  active worker no longer relies on a pass-polling compatibility path.
- [x] Split the worker into the three intended layers in live control flow.
  The active control flow now runs through explicit `worker/layers.rs`
  `IngressBridge`, `WorkerScheduler`, and `JournalCoordinator` types, and the old collapsed
  `run_supervisor_pass()` path is no longer the live supervisor entrypoint.
- [x] Finish the worker-local shared async runtime design.
  Worker-owned shared async effect runtimes now own adapter registries and inflight dedup keyed by
  universe/store, while per-world bindings live on the registered world metadata and active world
  slots remain lightweight scheduler-owned state.
- [x] Make the async effect system explicit in the hosted architecture cutover.
  External async effects now run through a worker-owned shared async effect runtime with explicit
  ownership of routing, inflight dedup, adapter lifecycle, and continuation delivery back into the
  scheduler.
- [x] Finish materializer/control promotion on top of the restored projection publisher.
  The active worker, materializer, and control/read-side surface now compile and run together on
  the active hosted center, and the hosted test surface verifies projection rebuild/read paths
  against the new execution core.

## Correctness And Semantics Gaps

- [x] Clear or recover staged slices correctly on partition revocation and failover.
  Assignment revocation now clears staged and local-ready slices that touch revoked partitions,
  resets affected active-world commit state, removes pending created-world staging when needed,
  and prevents a revoked owner from later flushing orphaned work after losing assignment.
- [x] Rehydrate inline deterministic effects after restart and failover.
  Reopen/rehydrate now reconstructs `InlineInternal` work in addition to timers and external async
  effects, and re-enters the scheduler through local `WorldInput::Receipt` delivery rather than
  treating inline deterministic effects as a no-op after restart.
- [x] Fix assignment handling in the active worker path.
  Kafka assignment sync now reports full assigned state plus deltas, and the active worker no
  longer mistakes `newly_assigned` for the full assignment set.
- [x] Implement explicit assignment/revocation handling in scheduler state.
  `AssignmentDelta` now enters the scheduler mailbox and updates worker-owned assignment state.
- [x] Restore flush timing semantics.
  `FlushLimits.max_delay` now drives flush ticks in the live worker loop, and flushes also trigger
  on staged-slice pressure.
- [x] Restore checkpoint timing semantics in the active core.
  Scheduler-owned checkpoint ticks and timed publication are wired, `checkpoint_on_create` now
  respects worker config on the active path, and create-time pending checkpoints survive reopen
  until they are published or manually checkpointed.
- [x] Decide and implement the final handling of local continuation slices.
  The scheduler now keeps source-less completions on an explicit local-ready queue and batches at
  most one local continuation slice per flush after the ingress-backed contiguous prefix, with
  direct unit coverage in `worker/core.rs`.
- [x] Rotate flush fairness cursor or simplify it away.
  `flush_rr_cursor` now advances on successful ingress-backed flushes so partition scan order
  actually rotates instead of being fixed.
- [x] Stop advancing command read-side state ahead of the durable fence.
  Hosted command submission now records `Queued` only after durable ingress submission succeeds,
  the active worker no longer writes eager `Running` state ahead of commit, and terminal command
  records remain a post-commit read-side consequence of the durable fence.

## Runtime Shape Cleanup

- [x] Move real scheduler and journal logic out of `runtime.rs`.
  `worker/scheduler.rs` now owns ingress intake, assignment handling, mailbox service, slice
  staging, and strict-quiescence checks, while `worker/journal.rs` owns flush, rollback, and
  post-commit effect/timer follow-up. `runtime.rs` is now primarily lifecycle/open/reopen/public
  surface code rather than the monolithic worker state machine.
- [x] Move world lifecycle and domain/store helpers out of `runtime.rs`.
  Active world seeding, registration, reopen/rehydrate, and activation now live in
  `worker/worlds.rs`, while domain path/store/blob-meta helpers live in `worker/domains.rs`.
  `runtime.rs` is now mostly the `HostedWorkerRuntime` facade plus a small amount of shared setup.
- [x] Remove or rewrite remaining legacy top-level runtime shells.
  The gated `legacy-bins` `main.rs` path now runs worker, materializer, and control directly as
  Tokio tasks on the top-level runtime instead of wrapping worker/materializer inside nested
  current-thread Tokio runtimes.
- [x] Delete or quarantine stale hosted worker code that is no longer on the active path.
  Stale partition-oriented worker files now live under `worker/legacy/` instead of sitting beside
  the active compiled worker center.
- [x] Make the active/legacy boundary obvious in code layout.
  `worker/mod.rs` now defines the compiled center explicitly, and `worker/legacy/README.md`
  documents the quarantined pre-cutover files.
- [x] Move checkpoint publication fully onto the new slice/journal fence.
  Partition checkpoint creation now stages source-less checkpoint slices, flushes them through the
  normal journal coordinator path, applies checkpoint metadata/compaction from post-commit, and
  uses the same durable fence and failure-recovery mechanics as other hosted slices.

## Public Surface And Build Hygiene

- [x] Repair the crate public surface after the core cutover.
  The library now exposes the hosted `bootstrap`, `control`, `materializer`, `services`, and
  `test_support` modules again, and those surfaces compile against the new worker runtime.
- [x] Make the shipped `aos-node-hosted` binary use the intended hosted assignment topology by
  default.
  The broker-backed binary now leaves `direct_assigned_partitions` empty by default, so workers
  join the Kafka consumer group and consume whatever partitions Kafka assigns. Direct partition
  selection remains available only as an explicit advanced override for repro/debug flows.
- [x] Reconnect the materializer and projection stack to the active hosted worker.
  The read side now compiles, boots, consumes active projection records, and exposes current
  projection state again from the active public surface.
- [x] Get the full `aos-node-hosted` test surface green again.
  `cargo test -p aos-node-hosted -- --nocapture` now passes end to end again, including the
  broker-backed restart/failover coverage that previously kept the hosted suite red.
- [x] Reconcile tests and helpers with the new active worker contract.
  Hosted tests/helpers now drive the active runtime, restart/failover semantics, projection-token
  continuity, and create-time checkpoint behavior rather than the pre-cutover center.
- [x] Add focused tests for the new core rather than inheriting old center assumptions.
  `worker/core.rs` now directly covers scheduler batching/ordering behavior, and the hosted Kafka
  suite now has focused restart/failover crash-window tests for timer, external async, and inline
  deterministic post-commit recovery.

## End-State Validation

- [x] Prove that ingress offsets advance only for contiguous durably committed prefixes.
  `worker/core.rs` now has focused scheduler tests covering contiguous serviced-prefix batching and
  offset advancement.
- [x] Prove that deterministic inline internal effects also survive restart/failover correctly.
  The hosted Kafka suite now forces the crash window between durable append and post-commit inline
  execution with a workspace-backed `workspace.*` flow and proves both restart and failover repair.
- [x] Prove that reopened or failed-over workers do not duplicate timer or external async work.
  Hosted Kafka tests now assert single-intent/single-receipt behavior across timer restart/failover
  and external async failover, so the durable ownership and dedup path is covered on the active
  worker.
- [x] Re-establish strict quiescence and checkpoint behavior on the new center.
  Hosted manifest apply now blocks on worker-visible strict-quiescence violations
  (non-terminal workflow state, open effects/receipts, mailbox backlog, staged commit state, and
  scheduled owner-local timers), and live checkpoint ticks now drain pending scheduler work and
  force a flush before checkpoint publication.
- [x] Remove the need for externally sleeping loops in normal worker operation.
  The production hosted worker now has a live mailbox-driven `serve_forever()` path, and the
  active crate no longer carries a supported single-pass worker-driving path.
- [x] Delete embedded hosted `run_once()` and the stale compatibility tests/helpers that depend on it.
  No active `run_once()` path remains under `crates/`, and the embedded hosted tests/helpers now
  drive the live spawned worker path instead of single-pass compatibility behavior.

## Practical Next Sequence

- [x] Fix the real control-path wiring first.
  `HostControl` and `WorldControl` now have a production submission path again through the normal
  hosted broker control surface rather than a test-only or in-process helper path.
- [x] Fix restart repair for inline deterministic effects next.
  The active worker now reconstructs inline deterministic post-commit work after restart/failover,
  and the hosted Kafka suite covers the crash window directly.
- [x] Remove forced direct assignment from the default binary startup path or clearly demote it to
  an explicit dev/test mode.
- [x] Fix assignment contract and scheduler state ownership first.
- [x] Introduce a real scheduler mailbox loop with explicit ingress, local-input, flush-tick, and
  checkpoint-tick messages.
- [x] Move timer wakeups and async effect completions to scheduler messages instead of pass
  polling.
- [x] Hoist external async execution resources to worker scope and make world slots lightweight.
- [x] Restore projection publication and projection-token continuity handling on the active center.
- [x] Re-enable the hosted library/control/materializer/test-support surface on the active crate.
- [x] Collapse worker-owned async sidecars into a truly shared async effect runtime.
- [x] Rebuild checkpointing on the new center.
- [x] Repair public exports/tests after the hosted core behavior is stable.
