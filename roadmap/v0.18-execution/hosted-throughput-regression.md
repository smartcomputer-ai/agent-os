# Hosted Throughput Regression

Date: 2026-04-16

## Problem

After the `roadmap/v0.18-execution/architecture/` hosted refactor, the single-world
`hosted-prof` counter throughput benchmark regressed badly.

Historical baseline before the refactor:

- single-world throughput was approximately `5000 events/s`

Current measured result after the refactor:

```bash
cargo run -p aos-node-hosted --bin hosted-prof -- --scenario counter-throughput --runtime broker --iterations 1 --messages 500
```

Measured output:

- `messages=500`
- `throughput=7.39 msg/s`
- `stream_submit=10 ms`
- `stream_complete_wait=67651 ms`
- `partition_commit_batch=67137 ms`

## What We Measured

The important observation from the current profiler output is:

- ingress submission is fast
- kernel/event service is not the dominant cost
- almost all steady-state time is spent in `partition_commit_batch`

From the measured run:

- `stream_submit=10 ms` for all `500` messages
- `steady_state_stream=67651 ms`
- `partition_commit_batch=67137 ms`

That means the regression is centered on the durable hosted flush fence, not on event submission.

## Architectural Cause

Before the refactor, the legacy worker used a partition-batch execution model:

- drain many submissions from a Kafka partition
- process many submissions in memory
- commit them together with one batch journal/offset commit

Relevant legacy code:

- `run_partition_once_profiled()` in [legacy/execute.rs](../../crates/aos-node-hosted/src/worker/legacy/execute.rs)
- single batch commit at [legacy/execute.rs](../../crates/aos-node-hosted/src/worker/legacy/execute.rs:154)

After the refactor, the active hosted worker uses:

- one mailbox item -> one `CompletedSlice`
- at most one uncommitted slice per world
- `commit_blocked` prevents the next same-world item from being serviced
- any ingress-backed staged slice triggers flush pressure immediately
- flush is a Kafka transaction that appends journal records and advances ingress offsets

Relevant active code:

- immediate flush trigger in [scheduler.rs](../../crates/aos-node-hosted/src/worker/scheduler.rs:227)
- ingress-backed slices force flush pressure in [scheduler.rs](../../crates/aos-node-hosted/src/worker/scheduler.rs:259)
- one uncommitted slice per world via `commit_blocked` in [scheduler.rs](../../crates/aos-node-hosted/src/worker/scheduler.rs:707) and [scheduler.rs](../../crates/aos-node-hosted/src/worker/scheduler.rs:1065)
- transactional Kafka flush in [broker.rs](../../crates/aos-node-hosted/src/infra/kafka/broker.rs:186)

This matches the v0.18 hosted architecture note:

- “one uncommitted slice per world” in [hosted-architecture.md](architecture/hosted-architecture.md:379)
- intended batching at the journal fence in [hosted-architecture.md](architecture/hosted-architecture.md:609)

## Why Single-World Throughput Collapsed

For a single hot world, the current active design effectively becomes:

1. service one event
2. stage one slice
3. flush one Kafka transaction
4. clear `commit_blocked`
5. service the next event

So the benchmark is no longer measuring the old partition-batch center. It is measuring
single-world per-event transaction-fence throughput.

With the observed numbers:

- `67651 ms / 500 ~= 135 ms` per durable commit

That is consistent with the measured `7.39 msg/s`.

## Temporary No-Flush Experiment

To isolate the flush fence cost, a temporary profiler-only escape hatch was added:

```bash
cargo run -p aos-node-hosted --bin hosted-prof -- --scenario counter-throughput --runtime broker --iterations 1 --messages 500 --unsafe-no-flush
```

This mode is intentionally unsafe:

- it bypasses Kafka flush commit
- it is not durable
- it is for profiling only
- it is only supported for the counter scenarios

Measured output in `--unsafe-no-flush` mode:

- `messages=500`
- `throughput=44.92 msg/s`
- `stream_submit=4 ms`
- `stream_complete_wait=11130 ms`
- `partition_commit_batch=10855 ms`

Interpretation:

- disabling the flush fence improves throughput materially
- but it does not recover the old ~`5000 events/s` baseline
- therefore the regression is not only Kafka transaction cost

## Current Conclusion

There are at least two costs in the post-refactor single-world path:

1. the per-event Kafka flush transaction is expensive
2. the per-event staged-slice/finalize/post-commit path is still substantially slower than the
   old partition-batch execution center, even when the durable commit is bypassed

So the regression is real, but it should be understood correctly:

- this is not simply “Kafka got slower”
- this is a consequence of the new hosted execution shape
- the benchmark itself changed meaning because the runtime semantics changed

## Likely Next Investigations

1. Measure a many-world throughput scenario.
   The new architecture is explicitly designed to batch across many worlds, not to maximize
   same-world speculative throughput.
2. Isolate post-commit overhead further.
   The next profiler experiment should bypass or separately time projection publication and other
   post-commit work.
3. Revisit whether “one uncommitted slice per world” is acceptable for the intended workloads.
   If high single-world ingress throughput remains a requirement, the active model will need either:
   - more than one uncommitted same-world slice, or
   - batching multiple same-world ingress items into one durable slice/commit fence

## Important Caveat

The temporary `--unsafe-no-flush` profiler mode is diagnostic only. It must not be treated as a
valid hosted execution mode because it violates the hosted durability rule:

- ingress offsets must only advance with the same durable Kafka-side consequence for that record

That rule remains correct. The open question is how much batching or same-world pipelining should
be allowed before that durable fence.
