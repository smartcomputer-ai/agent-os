# P1: Hosted Projection Publication Modes

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: Medium-High (hosted throughput stays capped by read-side work in the durable
flush path, and the hosted center remains architecturally muddled about what is authoritative)  
**Status**: Proposed hosted follow-on for v0.18  
**Depends on**:
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`
- `roadmap/v0.18-execution/hosted-throughput-regression.md`

## Goal

Define the hosted projection publication contract and the runtime knob that controls it:

1. what projection publication is allowed to do in the worker commit path,
2. what should be treated as authoritative versus read-side only,
3. how hosted should recover projection publication after crash or failover,
4. why the default hosted mode should not synchronously publish projections inline with the journal
   fence.

This note is about hosted worker and materializer behavior only.
It does not change the authoritative journal model.

## Why This Exists

The current active hosted worker still publishes projection records synchronously during
`finalize_flush_success()`.

That has two bad consequences:

1. projection publication latency is paid directly on the world service critical path,
2. the hosted center still behaves as if projection publication were part of authoritative commit.

That is the wrong center.

The hosted architecture note explicitly says:

- `Control` and `Materializer` stay outside the authoritative execution center.

So projection publication should be treated as read-side work with repairable lag, not as a
condition of authoritative world progress.

## Current Problem

The throughput regression profiler already shows that even after bypassing the durable Kafka flush
for benchmarking, the worker still spends substantial time in the flush/finalize stage.

That is consistent with the current implementation shape:

1. commit slice(s),
2. classify and submit post-commit work,
3. build projection records,
4. publish projection records synchronously,
5. only then continue.

This is especially expensive in broker mode because projection records are currently published
record-by-record and awaited record-by-record.

Even if this is not the full hot-world throughput answer, it is still the wrong architecture and a
real throughput tax.

## Core Design Stance

### 1) Projections are read-side state only

Projection topics, materialized cell tables, workspace views, and command/projection observers are
not authoritative world state.

Authoritative hosted state remains:

- checkpoint/snapshot baseline,
- `aos-journal`,
- kernel-reconstructible open-work and continuation state.

So projection publication must not be required for authoritative commit success.

### 2) The durable journal fence should complete without waiting on projection publication by default

The core hosted fence is:

- append frame or durable disposition,
- advance Kafka source offsets for the contiguous committed prefix,
- publish effects only after that durable append.

Projection publication is not part of that fence.

### 3) Background mode must be restart-repairable

`background` must not mean:

- publish opportunistically from an in-memory queue and lose track on crash.

It must mean:

- authoritative commit succeeds first,
- projection lag is allowed,
- crash/restart or failover causes the assigned worker to republish a fresh full projection image.

The current hosted runtime already has the right basic shape for that:

- projection continuity is process-local worker state, not durable metadata,
- reopening a world without matching continuity assigns a new `projection_token`,
- activating that world republishes `WorldMeta` plus the full cell/workspace snapshot,
- the materializer treats the new token as a projection epoch boundary.

### 4) Read-your-writes is a policy choice, not an authority rule

Some hosted reads may want immediate read-side freshness for debugging or tests.
That is a policy knob.
It is not a reason to keep projections on the authoritative commit path by default.

## Recommended Surface

Introduce one hosted runtime/worker knob:

```rust
enum ProjectionCommitMode {
    Inline,
    Background,
}
```

Recommended default:

- `Background`

Recommended semantics:

- `Inline`
  - projection publication remains synchronous after durable flush success,
  - intended for debugging, benchmarking comparison, and compatibility paths,
  - not recommended as the default hosted production mode.
- `Background`
  - durable journal commit completes first,
  - the worker marks affected worlds as projection-dirty,
  - background projection publication catches up asynchronously,
  - crash/restart or failover repairs projection lag by republishing a fresh snapshot under a new
    projection token.

## What `Background` Should Mean

`Background` should still preserve the post-commit ordering that matters:

1. durable slice commit succeeds,
2. opened effects may be started,
3. projection publication may lag.

That means background projection lag must never delay:

- source offset advancement for already-durable ingress,
- same-world service after commit,
- async effect start for the committed slice.

### Worker responsibilities in `Background`

After durable commit, the worker should:

1. clear world commit barriers,
2. apply post-commit effect classification,
3. record projection-dirty state for affected worlds,
4. return to scheduling.

It should not synchronously publish projection records before returning to the scheduler.

### Repair responsibilities in `Background`

The first-cut repair path does not need a separate durable projection-restore mechanism.

Instead, on worker restart, reopen, or failover:

1. worlds in assigned partitions are reopened,
2. projection continuity is absent or mismatched,
3. the worker assigns a new `projection_token`,
4. activating the world republishes `WorldMeta` plus a full projection snapshot,
5. the materializer treats that token change as a reset for that world's read-side rows.

That is enough to recover from a crash in which authoritative journal commit succeeded but the
background projection publication path had not caught up yet.

The trade is startup or failover republish cost, not correctness risk.

## Why This Preserves Correctness

### Authoritative guarantees stay unchanged

This note does not relax:

- world journal ordering,
- replay correctness,
- effect open/settle ordering,
- Kafka ingress offset safety.

Those still depend only on the authoritative slice commit fence.

### Crash safety stays intact if restart republish is authoritative-state-driven

The only thing that may be lost transiently in `Background` mode is read-side freshness.

After crash or failover:

1. reopen world from checkpoint plus journal,
2. detect missing process-local continuity and mint a new projection token,
3. republish the full projection image for the reopened world.

If that republish path works, background mode does not weaken crash safety for authoritative state.

### The tradeoff is eventual consistency, not correctness loss

The actual trade in `Background` mode is:

- projection readers may lag behind the most recent committed world state.

That is acceptable because projections are read-side only.

## What This Does Not Yet Propose

This note does **not** require a full `materializer-only` cut where the worker stops publishing any
projection topic records and a separate materializer derives projections exclusively from
`aos-journal`.

That may still be the cleanest future end state.
But it is a larger refactor than needed for the immediate hosted throughput and architecture fix.

This note intentionally keeps the near-term knob narrow:

- `inline | background`

with `background` defined as the default hosted behavior.

## Recommended Implementation Shape

### 1) Add the mode and default it to `Background`

The hosted runtime config should carry `projection_commit_mode`.

Hosted binaries/tests may still override to `Inline` where needed.

### 2) Keep projection continuity process-local and use republish as the repair path

The current projection token / projected head continuity machinery is already enough for the
initial hosted cut, but it should be understood as an in-memory bootstrap hint, not a durable
restore contract.

Hosted should rely on it to decide:

- projection state is current for this worker process,
- projection state must be republished on reopen,
- projection continuity must be invalidated by minting a new token.

That keeps the recovery model simple:

- if the process survives, incremental background publication may continue,
- if the process dies or ownership changes, republish the world under a new token.

### 3) Move synchronous projection publication behind the mode gate

- `Inline`: keep current behavior.
- `Background`: replace synchronous publish with projection-dirty enqueue/marking.

### 4) Add explicit repair coverage

The hosted test surface should cover:

1. commit succeeds, background projection publish is delayed, world continues running,
2. crash after durable commit but before projection catch-up, restart repairs read-side state,
3. failover worker repairs stale projection continuity before serving reads that depend on it.

## Expected Impact

This should not be sold as the full hot-world throughput answer.

It is:

- a real throughput improvement,
- a removal of read-side work from the authoritative critical path,
- an architectural correction that should happen regardless of the exact same-world pipelining
  decision.

The likely effect is:

- meaningful reduction in post-commit overhead,
- better many-world throughput,
- better headroom for later same-world pipelining work,
- but not by itself enough to guarantee `>=1000 events/s` for one hot world.

## Recommendation

Land this as the no-regret hosted fix:

1. add `projection_commit_mode = inline | background`,
2. make `background` the default,
3. keep `inline` for debugging/comparison,
4. treat journal-driven repairability as the non-negotiable correctness rule.
