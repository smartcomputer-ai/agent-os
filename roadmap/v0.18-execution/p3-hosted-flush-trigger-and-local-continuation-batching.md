# P3: Hosted Flush Trigger and Local Continuation Batching

**Priority**: P3  
**Effort**: Medium-High  
**Risk if deferred**: Medium-High (hosted keeps paying unnecessary per-flush overhead and
continuation-heavy worlds remain artificially throttled even after projection decoupling and
same-world pipelining work)  
**Status**: Implemented with a simplified flush model  
**Depends on**:
- `roadmap/v0.18-execution/hosted-throughput-regression.md`
- `roadmap/v0.18-execution/p1-hosted-projection-publication-modes.md`
- `roadmap/v0.18-execution/p2-max-uncommitted-slices-per-world.md`

## Goal

Define the hosted batching controls that remain after projection publication is moved off the hot
path and same-world speculative staging is made explicit:

1. when staged slices should trigger a flush,
2. how aggressively source-less local continuation slices may share a flush,
3. how to improve throughput without changing the authoritative commit fence.

This note is intentionally about worker batching policy, not about changing authoritative state
semantics.

## Why This Exists

The current hosted worker still has two policy choices that are too restrictive for the intended
workload shape:

1. any ingress-backed staged slice creates immediate flush pressure,
2. at most one source-less local continuation slice may be included per flush.

That means hosted is still biased toward:

- frequent small Kafka transactions,
- poor same-world continuation throughput,
- poor receipt/frame fan-in behavior for tool-heavy or LLM-heavy worlds.

Those choices were reasonable first-cut simplifications.
They are not good long-term defaults for the throughput targets described in the regression note.

## Current Problem

### 1) Immediate flush pressure for ingress-backed slices

Today the worker effectively says:

- if any staged slice has `source.is_some()`, flush pressure is reached immediately.

That defeats a large part of the intended batching model.

The hosted architecture note already says the desired model is:

- continuous ingress buffering,
- per-record service slices,
- batched transactional journal commit,
- flush on `max_slices`, `max_bytes`, or `max_delay`.

So immediate flush on every ingress-backed slice is not just expensive.
It also works against the intended hosted center.

### 2) Only one local continuation slice per flush

Today the collector keeps local continuation batching extremely tight:

- at most one source-less local continuation slice per flush,
- except for checkpoint slices.

That is especially bad for worlds that generate:

- many tool receipts,
- many stream frames,
- many timer completions,
- many quick async continuations relative to ingress volume.

Those are exactly the workloads that external execution is supposed to support well.

## Implemented Surface

The final implemented model keeps one explicit batching knob and one implicit flush rule.

### 1) Flush pressure comes from capacity and limits

Hosted now flushes when any of these are true:

- a world has staged work pending and cannot stage more because commit catch-up is required,
- `max_delay` elapsed for the oldest staged slice,
- `max_slices` is reached,
- `max_bytes` is reached,
- an explicit forced flush is requested such as shutdown/checkpoint/manual paths.

This replaces the earlier `flush_trigger_policy` split. There is no longer a public policy switch
between `ImmediateOnIngress` and `LimitsOnly`.

### 2) `max_local_continuation_slices_per_flush`

```rust
max_local_continuation_slices_per_flush: usize
```

Recommended meaning:

- `0`
  - do not include local continuation slices in ordinary flushes,
  - mainly useful as a debugging or stress mode.
- `1`
  - current conservative behavior.
- `N > 1`
  - allow up to `N` source-less local continuation slices to share one flush.

Recommended default end state:

- greater than `1`,
- or a computed value derived from `max_slices`,
- but not the hard-coded current global `1`.

## Current Implementation

As currently implemented in hosted:

- `max_uncommitted_slices_per_world` is the per-world speculative staging limit,
- `max_local_continuation_slices_per_flush` is a per-worker flush-batch cap, not a per-world cap,
- `flush_limits.max_slices`, `flush_limits.max_bytes`, and `flush_limits.max_delay` remain
  global per-flush limits,
- flush pressure is raised when any world is blocked on commit catch-up or when one of the global
  flush limits is reached.

The current default profile is:

- `projection_commit_mode = background`
- `max_uncommitted_slices_per_world = 256`
- `max_local_continuation_slices_per_flush = 64`
- `flush_limits.max_slices = 256`
- `flush_limits.max_bytes = 1 MiB`
- `flush_limits.max_delay = 5ms`

### What These Mean In Practice

- `max_uncommitted_slices_per_world`
  - per world,
  - limits how many slices may be staged but not yet committed,
  - directly affects hot-world throughput and also now contributes to flush pressure when a world
    is blocked on that limit.
- `max_local_continuation_slices_per_flush`
  - per worker flush batch,
  - caps how many source-less local continuation slices may be appended after the ingress-backed
    contiguous prefix is built,
  - does not currently act as an independent flush trigger.
- `flush_limits.max_delay`
  - measured from the oldest staged slice using monotonic `Instant`,
  - enforced cooperatively by periodic `FlushTick` wakeups rather than a hard preemptive deadline.

### Effect Start Relationship

These knobs only affect when hosted decides to flush and how much staged work may share a flush.
They do not change the authoritative effect-start rule:

- external async effects and owner-local timers start only after a successful flush commit,
- not merely when a slice is staged in memory.

### Notes From Current Benchmarking

On ingress-heavy durable broker benchmarks, the simplified capacity-and-limits flush model is not
automatically faster than the earlier modal policy split. The main benefit of the current design is
that the flush rule is simpler and tied to real backpressure rather than to ingress-vs-continuation
classification.

Continuation-heavy workloads are expected to benefit more clearly from the current `P3` behavior
than pure ingress microbenchmarks.

## What These Knobs Change

They change:

- how aggressively the worker batches already-staged work at the journal fence.

They do **not** change:

- one-mailbox-item-per-service-slice semantics,
- per-world durable ordering,
- effect-open identity,
- effect-start-after-commit rules,
- ingress offset advancement rules.

## Core Design Stance

### 1) Flush cadence and source-offset correctness are separate concerns

An ingress-backed slice still must not acknowledge source offsets until the durable Kafka-side
consequence exists.

That rule remains unchanged.

But that does **not** imply:

- every ingress-backed slice must trigger its own immediate transaction.

The worker can safely wait for:

- more staged slices,
- a timeout,
- or byte/slice limits,

as long as the final transaction still commits a contiguous serviced prefix.

### 2) Local continuations should be throughput participants, not flush afterthoughts

Receipts and stream frames are not lower-class work.
They are the normal continuation path for external execution.

So the batching model should not treat them as:

- one small extra slice per transaction forever.

That policy may prevent local continuations from monopolizing the fence, but it also suppresses the
exact workloads hosted needs to run well.

### 3) Fairness still matters

This note is not advocating:

- unbounded inclusion of local continuation slices,
- or a collector that starves ingress-backed progress.

The right model is:

- bounded but materially larger local continuation participation,
- explicit fairness rather than a near-total ban.

## Recommended Semantics

### Capacity-and-limits flushing

In the simplified model the worker should flush because:

1. the oldest currently-staged slice exceeded `max_delay`,
2. `max_slices` would be reached,
3. `max_bytes` would be reached,
4. at least one world has staged work pending and is blocked on commit catch-up,
5. an explicit forced flush is requested such as shutdown/checkpoint/manual path.

This is the natural batching policy once `max_uncommitted_slices_per_world` is greater than `1`,
but it is still useful even when that value remains `1` because it improves many-world batching.

### `max_local_continuation_slices_per_flush = N`

In this mode the collector should:

1. build the ingress-backed contiguous committed prefix first,
2. then add up to `N` source-less local continuation slices,
3. still respect `max_slices` and `max_bytes`,
4. preserve same-world slice ordering,
5. never let local continuation inclusion change source offset advancement rules.

The collector may still choose fair ordering among local slices, but the hard cap should be a real
tuning parameter rather than a fixed hidden constant.

## Why This Preserves Correctness

### Authoritative commit rules do not change

The authoritative fence is still:

- durable frame/disposition append,
- contiguous offset advancement,
- only after commit may effects start or be considered durably opened.

Neither knob weakens that.

### Delayed flush is not weaker than immediate flush

Waiting for more staged slices does not weaken correctness if:

- slices are already staged deterministically,
- per-world order is preserved,
- no offsets are advanced before the final durable transaction.

The only thing that changes is transaction size and timing.

### Local continuation batching does not change effect identity or settlement rules

Receipts and stream frames are already keyed by `intent_hash`.
Including more of them in one transaction does not change:

- which open effect they belong to,
- duplicate handling,
- per-effect stream fencing,
- terminal settlement semantics.

It only changes how many already-staged continuation slices may share the fence.

## Relationship To `P2`

These knobs are not substitutes for `max_uncommitted_slices_per_world`.

### `P2` changes same-world speculative staging capacity

That is the hot-world ceiling knob.

### `P3` changes how the journal fence consumes staged work

That is the batching and fairness knob.

The intended interaction is:

1. `P2` allows the worker to have more useful staged work available,
2. `P3` lets the journal fence batch that staged work efficiently.

Without `P2`, `P3` mostly helps many-world throughput and continuation-heavy workloads.
Without `P3`, `P2` leaves throughput on the table because the fence still flushes too eagerly and
continuations still share the fence too poorly.

## Suggested Rollout

### Current rollout

Hosted now uses the simplified single flush rule plus configurable
`max_local_continuation_slices_per_flush`.

Further tuning should focus on:

- the default value for `max_local_continuation_slices_per_flush`,
- whether per-world staging-capacity pressure should become more nuanced,
- continuation-heavy benchmark coverage.

## What This Does Not Propose

This note does **not** propose:

- batching multiple mailbox items into one service slice,
- changing effect identity or receipt routing,
- relaxing ingress offset safety,
- making local continuations authoritative outside normal world admission.

It is only about how the worker batches already-staged slices.

## Recommendation

Land this as the third hosted throughput note:

1. make local continuation participation explicit with
   `max_local_continuation_slices_per_flush`,
2. replace modal flush policy with one capacity-and-limits batching rule,
3. stop treating local continuations as one-per-flush exceptional work.

This is the next policy layer after:

- `P1` removes projection publication from the hot path,
- `P2` defines how much same-world speculative staged work may exist.
