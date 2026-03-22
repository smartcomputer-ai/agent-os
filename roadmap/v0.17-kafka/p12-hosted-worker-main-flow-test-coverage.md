# P12: Hosted Worker Main-Flow Test Coverage

**Priority**: P12  
**Effort**: Medium  
**Risk if deferred**: High (the hosted runtime may look healthy while the main worker durability
and runtime-work flows remain under-tested)  
**Status**: Complete

## Goal

Define the missing hosted Kafka/S3 worker-flow tests that are still required for confidence in the
real hosted runtime.

The current broker-backed coverage proves important infrastructure seams:

- route publication and recovery
- ingress polling and journal commit
- checkpoint publication and restart recovery
- create-world, simple event processing, and abort/retry

That is necessary, but not yet sufficient. The main gap this phase targeted was worker-level
runtime behavior once worlds have pending work and restart/handoff conditions.

## Design Stance

These tests should be:

- real Kafka
- real blobstore
- worker-oriented rather than HTTP-only
- narrower than full product/end-to-end tests

They should validate the worker as the owner of authoritative execution, not just the transport
planes beneath it.

## What Exists Already

Current hosted broker coverage already includes:

1. create-world from ingress plus first checkpoint
2. simple ingress event processing
3. aborted batch rollback and retry after restart
4. lower-level Kafka plane tests for route/journal/transaction visibility
5. lower-level blobstore plane tests for blob/checkpoint/command-record primitives

That is the right base layer.

## What Is Still Missing

### 1) [X] Effect receipt progression (DONE)

Add a real hosted worker test that proves:

1. a workflow emits an external effect
2. pending runtime work exists after the initial event
3. a receipt is submitted back through the hosted ingress path
4. the worker applies the receipt
5. the world reaches the expected post-receipt state
6. pending runtime work clears

This is the hosted equivalent of the old "effect executes and receipt applies" worker tests.

### 2) [X] Reopen correctness with no pending-work resurrection (DONE)

Add hosted recovery tests that prove:

1. a world reaches a completed or quiescent post-receipt state
2. a checkpoint is published
3. the runtime is restarted and reopened from checkpoint + journal replay
4. pending receipts, queued effects, and inflight intents do not reappear

This is one of the most important correctness seams in the old FDB suite and is still missing on
the hosted broker path.

### 3) [X] Worker failover / ownership handoff of in-flight work (DONE)

Add a hosted worker test that proves:

1. worker A starts processing a world and leaves it with pending work
2. worker A stops or loses ownership
3. worker B takes over the assigned partition
4. the world resumes and completes exactly once from durable state

The old FDB tests expressed this via leases and queue claims. The hosted Kafka equivalent should
express it via partition ownership handoff, not lease tables.

### 4) Durable command execution on the worker path (MOVED TO CLEANUP)

Add a hosted worker test that proves:

1. a governance/admin command is queued
2. the worker executes it on the broker-backed path
3. command records transition durably in blobstore
4. restart or reread observes the final command state

This complements the current HTTP command-surface tests by proving the worker execution path, not
just control submission and polling.

This item is no longer part of P12. It remains useful, but it is now tracked under `P20` as
follow-on cleanup/hardening rather than an open phase requirement here.

## Implemented Coverage

The hosted worker-flow suite now includes:

- `worker_processes_receipt_and_clears_pending_runtime_work`
- `worker_reopen_after_completion_does_not_resurrect_pending_runtime_work`
- `worker_failover_continues_inflight_work_from_durable_state`

These run on the real hosted path:

- real Kafka ingress / journal / route topics
- real blobstore-backed checkpoints and world materialization
- worker-driven state convergence rather than HTTP-only control polling

## Additional Regression Coverage Added

During P12 work we also added a focused checkpoint stress regression:

- `worker_periodic_checkpoint_under_large_hot_stream_preserves_world_sequence`

This test is intentionally `ignored` because it is a larger stress repro, but it covers a real
hosted correctness seam that surfaced while improving throughput:

- periodic checkpoint publication under a large hot stream
- preservation of per-world `world_seq_*` continuity
- recovery of the broker-local mirror without resurrecting sequence gaps

That test is not a DoD item for P12 by itself, but it materially strengthens confidence in the
same worker durability path.

## Explicit Non-Goals

- duplicate the lower-level Kafka plane tests
- duplicate the lower-level blobstore primitive tests
- recreate the old FDB lease/claim mechanics directly
- cover portal-send in this phase
- cover timers in this phase

Timers are intentionally split into P13 because the hosted timer execution path does not exist yet.

## Recommended Test File Shape

The main hosted worker-flow suite should stay small and explicit.

Suggested additions to `crates/aos-node-hosted/tests/kafka_broker.rs` or a renamed worker-flow
file:

- `worker_processes_receipt_and_clears_pending_runtime_work`
- `worker_reopen_after_completion_does_not_resurrect_pending_runtime_work`
- `worker_failover_continues_inflight_work_from_durable_state`

These should continue to use:

- isolated per-test Kafka topics
- isolated per-test consumer-group and transactional identities
- isolated per-test blobstore prefixes

## DoD

P12 is complete when:

1. Hosted broker-backed worker tests cover receipt progression.
2. Hosted broker-backed worker tests cover reopen correctness for cleared pending work.
3. Hosted broker-backed worker tests cover worker handoff/failover of in-flight work.
4. The hosted worker suite remains smaller and more worker-focused than the older monolithic test
   file style.
