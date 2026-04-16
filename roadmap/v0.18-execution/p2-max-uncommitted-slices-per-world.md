# P2: Max Uncommitted Slices Per World

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (hosted can batch across many worlds, but single hot-world throughput
remains fundamentally capped by one-slice-at-a-time commit serialization)  
**Status**: Proposed hosted follow-on for v0.18  
**Depends on**:
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`
- `roadmap/v0.18-execution/hosted-throughput-regression.md`
- `roadmap/v0.18-execution/p21-hosted-projection-publication-modes.md`

## Goal

Define the hosted throughput knob that controls same-world speculative staging without changing the
deterministic kernel admission model:

1. keep one mailbox item per service slice,
2. allow more than one same-world slice to remain staged before commit when safe,
3. preserve authoritative commit ordering, restart repair, and effect-start rules,
4. make the throughput/correctness contract explicit instead of hiding it inside
   `commit_blocked = true`.

## Why This Exists

The current active hosted design allows at most one uncommitted slice per world.

That was a good first-cut simplification.
It is now the main hot-world throughput limiter.

For one busy world, the current behavior is effectively:

1. service one mailbox item,
2. stage one slice,
3. block the world behind `commit_blocked`,
4. commit that slice,
5. only then service the next mailbox item.

That guarantees simple ordering, but it turns the hosted worker into a per-event fence machine for
single-world workloads.

If hosted must support worlds that absorb many concurrent tool calls, receipts, and model updates,
that ceiling is too low.

## Keep The Terminology Straight

### Slice

One hosted service slice means:

- dequeue one mailbox item,
- admit that one item,
- drain deterministic workflow progress until idle,
- stage one `CompletedSlice`.

This note does **not** change that unit.

### Effect

An external effect is open work keyed by `intent_hash`.
It may remain open across many later slices.
Distinct open effects may progress and settle out of original emission order as long as each effect
preserves its own sequencing/fencing rules.

So this note is **not** about whether multiple effects may overlap.
That already exists today.

This note is about whether multiple same-world **uncommitted slices** may overlap.

## Recommended Surface

Introduce one hosted scheduler/runtime knob:

```rust
max_uncommitted_slices_per_world: usize
```

Recommended meaning:

- `1`
  - current behavior,
  - at most one staged but not-yet-committed slice per world.
- `>1`
  - allow up to `N` staged same-world slices before commit catches up.

Recommended default:

- `1` initially,
- then increase conservatively once the implementation and tests are in place.

## What This Knob Changes

It changes:

- how far a single world may get ahead of the durable commit fence.

It does **not** change:

- the kernel API,
- the rule that one mailbox item maps to one service slice,
- the rule that effects start only after the opening slice commits,
- the rule that ingress offsets advance only with a durable contiguous prefix,
- the rule that authoritative world state advances only through committed journal frames.

## Core Design Stance

### 1) Do not batch multiple mailbox items into one kernel admission step first

The hosted architecture already chose the better first optimization boundary:

- batch at the journal fence,
- not at the kernel admission boundary.

That should remain true here.

So the first same-world throughput lever should be:

- allow more than one uncommitted same-world slice,

not:

- combine many ingress items into one kernel admission call.

### 2) Keep commit order strict per world

If slices `S1`, `S2`, and `S3` are staged for one world, the durable order must remain:

- `S1` before `S2` before `S3`.

There is no license here to reorder committed world frames within a world.

### 3) Keep effect start tied to the opening slice commit

If `S2` opens effect `E2`, `E2` must not start just because `S2` has been staged.

It may start only after `S2` itself commits durably.

This is critical.
The knob permits speculative staging.
It does not permit speculative effect execution.

### 4) Treat inline deterministic followups as ordering barriers

The current one-slice rule incidentally preserves ordering between:

- ingress items,
- inline post-commit internal effects,
- local continuation followups,
- later same-world ingress.

Once staged depth is greater than `1`, that ordering must be preserved intentionally.

So conservative same-world pipelining should stop at slices that can enqueue inline followups ahead
of later mailbox work.

## Recommended Conservative Semantics

The first safe version should be conservative rather than fully general.

### A world may continue staging later slices only while all staged predecessors are non-barrier slices

Examples of barrier slices:

- slices that open `InlineInternal` effects,
- slices that produce inline post-commit receipts,
- checkpoint slices,
- create-world/bootstrap special cases,
- world-control operations that rely on strict quiescence.

Examples of non-barrier slices:

- plain domain-event slices that only mutate workflow state and open no effects,
- slices that open only `ExternalAsync` effects,
- slices that open only owner-local timer effects,
- local receipt/frame continuations that produce no inline internal followups.

### Same-world staging depth applies only to staged-not-yet-committed slices

The count basis should be:

- slices successfully staged for a world,
- not yet durably committed or rolled back.

Once a slice commits or is recovered away on failure, it no longer consumes depth.

## Why This Preserves Correctness

### Authoritative state still advances only through the durable frame log

Even with staged depth greater than `1`, authoritative progress still depends on committed frames.

Speculative staged slices do not become authoritative until their commit succeeds.

### Crash safety can remain intact

Crash safety does not require `max_uncommitted_slices_per_world = 1`.
It requires:

1. strict per-world durable ordering,
2. restart from checkpoint plus journal replay,
3. requeue/repair of failed staged slices from durable state plus retained original items,
4. effect start only after the owning slice commits.

If those hold, a crash may lose speculative progress, but not authoritative correctness.

### Open effects already support overlap and out-of-order settlement

This is important to state explicitly.

The system already allows:

- effect `E1` opened in slice `S1`,
- effect `E2` opened in later slice `S2`,
- `E2` settling before `E1`,

as long as each effect is routed and fenced by its own `intent_hash`.

That is not a new semantic cost of same-world pipelining.
That is already part of the external execution model.

## Required State Changes

### 1) Replace single pending-slice tracking with ordered pending-slice tracking

Today each world effectively has:

- `commit_blocked: bool`
- `pending_slice: Option<SliceId>`

That will need to become an ordered per-world pending-slice structure such as:

- `pending_slices: VecDeque<SliceId>`

or equivalent.

### 2) Add speculative world-sequence advancement

If multiple same-world frames are staged before commit, later slices must not all compute frame
sequence numbers from the current durable head only.

Hosted needs a speculative per-world sequence cursor for staged slices so:

- `S1` gets `seq 100..109`,
- `S2` gets `seq 110..113`,
- `S3` gets `seq 114..120`,

before commit has necessarily caught up.

### 3) Rework flush failure recovery for multi-slice worlds

On transaction failure, hosted can no longer assume only one failed slice per world.

Recovery must:

1. reopen the world once from durable state,
2. clear all failed speculative slice tracking for that world,
3. requeue original items for all failed slices in the original mailbox order,
4. preserve deterministic replay of what was not durably committed.

### 4) Keep collector ordering explicit

The collector may include multiple same-world slices in one transaction.
But it must still preserve:

- per-world slice order,
- per-partition contiguous offset rules,
- no offset advancement past a gap.

## Suggested Barrier Rule For The First Cut

The first implementation should keep the eligibility rule simple:

1. a world may stage up to `max_uncommitted_slices_per_world`,
2. but only while every older staged slice for that world is non-barrier,
3. and only while no inline post-commit followup is pending ahead of ordinary mailbox work.

If a slice is a barrier slice:

- stage it,
- stop servicing later same-world items until it commits and its post-commit followups are enqueued.

This keeps the first cut understandable and testable.

## Why `max_uncommitted_slices_per_world` Is The Right Knob

This surface says exactly what the system is controlling.

It is better than a boolean such as:

- `same_world_pipelining = true`

because:

- `1` is the safe baseline,
- `2`, `4`, `8` are meaningful conservative tuning levels,
- the operator can reason directly about memory, speculative depth, and recovery complexity.

## What To Expect From This Knob

This is the real hot-world throughput lever.

If hosted needs to support:

- one busy world,
- many parallel tool calls,
- many receipts/frames arriving quickly,

then `max_uncommitted_slices_per_world > 1` is the knob that can change that ceiling materially.

By contrast:

- `flush_max_delay`,
- `max_slices`,
- `max_bytes`,

mainly help batching across many worlds once the current one-slice same-world barrier remains in
place.

## Recommended Rollout

### Phase 1

- land `projection_commit_mode = background` first,
- keep `max_uncommitted_slices_per_world = 1`,
- remove read-side publication from the authoritative critical path.

### Phase 2

- introduce ordered multi-slice per-world tracking,
- add speculative per-world sequence assignment,
- keep conservative barrier rules,
- set `max_uncommitted_slices_per_world = 2` in targeted hosted benchmarks/tests.

### Phase 3

- expand coverage to `4` or `8` if the conservative model holds,
- only then consider whether kernel-admission micro-batching is needed at all.

## Recommendation

Land this as the explicit hosted throughput knob:

1. add `max_uncommitted_slices_per_world`,
2. keep default `= 1` until the recovery/barrier model lands,
3. preserve one-mailbox-item-per-slice semantics,
4. preserve per-world commit order and post-commit effect-start rules,
5. treat barrier slices as the mechanism that keeps same-world pipelining semantically safe.
