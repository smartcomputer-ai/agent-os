# P20: Open-Work Backpressure and Overload Control

**Priority**: P20  
**Effort**: Medium-High  
**Risk if deferred**: Medium (the core async execution seam can still land, but detached execution
can expose unbounded open-work growth and unclear overload behavior)  
**Status**: Tentative / Optional for v0.18  
**Depends on**:
- `roadmap/v0.18-execution/p1-open-effect-lifecycle-and-owner-executor-seam.md`
- `roadmap/v0.18-execution/p2-start-eligibility-expiry-and-staleness.md`

## Goal

Describe a possible owner-side overload-control model for open external work:

1. why async execution can require explicit backpressure,
2. what should be bounded authoritatively versus operationally,
3. how the world should behave when open-work budgets are exhausted,
4. why receipts and stream frames must still be admitted under pressure,
5. where upstream ingress throttling may be desirable.

This document is intentionally tentative.
It is orthogonal to the core owner/executor seam and can land later if needed.

## Why This Exists

The core v0.18 problem is async external execution.
That seam can be designed without first deciding the system's overload policy.

But once execution is truly decoupled, the system loses an accidental property of the current host
loop:

1. drain effects,
2. dispatch them immediately,
3. await external receipts in the same host cycle,
4. only then continue.

That shape is not the architecture we want, but it does create incidental backpressure in some local
modes because the owner loop and adapter execution are still too coupled.

If external execution becomes an actually independent daemon or worker plane, then the question
changes:

- what stops open effects from growing without bound,
- what happens when receipts trigger more effects faster than executors can drain them,
- what is the overload behavior for domain ingress versus receipt ingress,
- where do we preserve replay-identical semantics while still protecting the system.

This is especially relevant because current kernel/runtime behavior already supports:

- per-effect async settlement,
- out-of-order receipts,
- receipt-to-workflow continuations,
- workflow steps that may emit new effects while handling prior receipts.

So a bad workflow can create positive feedback:

1. open effect,
2. receive receipt,
3. handle receipt in workflow,
4. emit more effects than were just settled,
5. repeat.

Per-tick workflow output limits are not enough to prevent that.
They bound a single step.
They do not bound total outstanding open work across the world.

## Problem Statement

Today there is no real kernel-side semaphore or authoritative open-work budget.

What exists today is mainly:

- deterministic per-tick output bounds,
- a dispatch outbox,
- open receipt context,
- per-workflow in-flight intent indexes,
- quiescence reporting.

Those pieces are useful for correctness and recovery, but they do not by themselves define overload
behavior.

Once owner progress and executor progress are split, the system needs an answer for:

1. bounded authoritative open work,
2. bounded executor concurrency,
3. bounded upstream submission pressure,
4. deterministic overload semantics.

These are related but not the same problem.

## Core Design Stance

### 1) Do not model this as a kernel semaphore

A live semaphore is an executor/runtime construct.
It reflects currently available worker capacity, sockets, provider slots, or host resources.

That is not the right owner-side primitive because:

- it is not inherently replayable,
- it depends on runtime placement and operator topology,
- it would couple deterministic admission to non-deterministic executor state.

The owner-side primitive should be a deterministic budget over authoritative open work.

### 2) Bound unique open effects, not incidental queue structures

One open effect currently appears in multiple structures for different reasons:

- dispatch/outbox state,
- pending continuation context,
- per-workflow in-flight indexing.

Those are not separate effects.

So the budget basis should be the unique open-effect set keyed by `intent_hash`, not:

- `queued_effects + pending_workflow_receipts + inflight_intents`,
- executor queue depth,
- live worker permits.

### 3) Receipts and stream frames must remain admissible under pressure

If overload handling blocks settlement ingress, the system can deadlock its own pressure relief.

Terminal receipts free open-work credit.
Non-terminal observations may be needed for reconcile/reattach logic.

So overload policy should prefer:

1. admit terminal receipts,
2. admit relevant stream frames,
3. admit control/governance traffic,
4. throttle or reject ordinary domain ingress first.

### 4) Overload should be individual-effect aware

The desired async model is per-effect and out-of-order.

So overload handling should first try to reject or fail individual newly-emitted effects rather than
freezing unrelated open effects or halting the whole world.

### 5) Executor concurrency limits still matter, but they are not authoritative

Effect daemons may still use:

- semaphores,
- worker pools,
- per-adapter concurrency caps,
- provider quotas,
- rate limiters.

Those are valid and likely necessary.
They are execution-plane controls, not owner truth.

## Recommended Model

### 1) Introduce authoritative open-work credits

Define deterministic budgets such as:

- `max_open_effects_world`
- `max_open_effects_per_workflow_instance`
- optional `max_open_effects_per_effect_kind`
- optional `max_open_effects_per_route`

These should count open effects by `intent_hash`.

In practice the natural owner-side source of truth is the open continuation context set that already
exists for effects awaiting terminal settlement.

### 2) Enforce the budget at effect-open admission

The budget check should occur when a workflow tries to turn output effects into durable open work.

That is the right point because it is:

- deterministic,
- journal-adjacent,
- independent of executor placement,
- equally applicable whether the triggering input was a domain event, receipt, or stream frame.

This is the key rule:

- overload control should apply where new open work is created,
- not only at domain ingress.

### 3) Prefer per-effect deterministic rejection on overflow

If a workflow step proposes more new effects than the open-work budget allows, preferred behavior is:

1. admit the triggering input,
2. open effects that fit,
3. reject effects that exceed capacity with a deterministic owner-side reason such as
   `effect.over_capacity`,
4. route those rejections back through the normal per-effect continuation path.

This preserves the async and individual-effect model better than globally stalling the world.

### 4) Allow workflow failure as the strict fallback

If rejected-effect delivery cannot be represented for a workflow, or if policy prefers a harder
safety response, the workflow instance may be failed.

That is stricter than per-effect rejection and should not be the first choice, but it is preferable
to silent growth or non-deterministic blocking.

### 5) Keep executor-local concurrency controls separate

Even with authoritative open-work credits, executors should still independently bound:

- concurrent starts,
- in-flight substrate operations,
- streaming fan-out,
- adapter-specific provider concurrency.

Those controls only shape execution throughput.
They do not replace owner-side open-work bounds.

## Overload Semantics

### Case A: Domain ingress arrives while world is saturated

Recommended default:

- allow the owner to continue admitting settlement traffic,
- reject or defer new ordinary domain ingress at the host/service boundary,
- if domain ingress is admitted and leads to new effects, those effects still face owner-side
  open-work admission limits.

This gives two pressure valves:

1. front-door submission throttling,
2. authoritative open-effect budgeting.

### Case B: A receipt continuation tries to fan out more work than budget allows

Recommended default:

1. admit the receipt,
2. settle the old open effect,
3. run the workflow continuation,
4. open only the subset of newly-emitted effects that fit,
5. reject the rest deterministically.

This is important.
Blocking receipt admission is the wrong move because receipts are how capacity is returned.

### Case C: Streaming observations arrive while saturated

Recommended default:

- admit observations for already-open effects,
- continue to gate any newly-opened work that downstream workflow steps try to emit.

### Case D: Executor plane is clogged but owner is not

Recommended default:

- executor-local semaphores and rate controls reduce start pressure,
- owner open-effect credits prevent total outstanding work from growing without bound,
- front-door ingress can apply additional throttling if a world stays above high watermark.

## Why Domain Ingress Alone Is Insufficient

A design that only throttles domain events misses the most important feedback path:

1. receipt admitted,
2. workflow continuation runs,
3. workflow emits more effects,
4. open work grows even with domain ingress paused.

So the authoritative bound must sit on effect opening itself.

Domain ingress throttling is still valuable, but it is not sufficient.

## Why Not Freeze The Whole World

A whole-world pause sounds simple, but it has bad properties:

- it penalizes unrelated workflows and unrelated already-open effects,
- it can obstruct terminal settlements,
- it treats overload as a coarse scheduler halt instead of a per-effect admission problem,
- it works against the goal of independent out-of-order effect progress.

The system should stay alive under pressure and continue draining existing obligations.

## Scheduler And Priority Guidance

If the owner loop later grows explicit priority handling, the preferred order under pressure is:

1. terminal receipts,
2. non-terminal observations for already-open effects,
3. governance/control traffic,
4. ordinary domain ingress.

This does not by itself solve overload, but it aligns runtime behavior with credit release and
system recovery.

## Possible Surface Shape

If this lands, the implementation may want:

- world-level config for open-work credits,
- workflow-level optional overrides or caps,
- trace/quiescence surfaces that show:
  - current open-effect count,
  - configured limits,
  - rejected-over-capacity count,
  - ingress throttling state.

Operator-facing high/low watermarks may also be useful at the host/service layer even if the kernel
only knows the hard deterministic bound.

## Non-Goals

This document does **not** define:

- provider billing budgets,
- capability spend budgets,
- retry policy,
- start-by or expiry policy,
- claim/lease protocol,
- the exact execution feed topology,
- the exact surface for hosted submission throttling.

Those are separate concerns.

## Open Questions

1. Should over-capacity be modeled as a synthetic rejected receipt event, a workflow output error, or
   a dedicated admitted continuation kind?
2. Should partial acceptance of a workflow's emitted effects be allowed, or should one over-budget
   effect cause the whole workflow step to fail?
3. Should the first version expose only a world-level cap, or also per-workflow-instance caps?
4. Should front-door domain-ingress throttling live in hosted/local services only, or also in the
   common host abstraction?
5. Do we want a soft watermark for operator controls in addition to a hard deterministic kernel
   bound?

## Recommendation

Keep this work out of the critical path for the main async execution seam.

But design `p1` and `p2` so this can be added cleanly later:

1. owner-side open work remains explicit and uniquely keyed by `intent_hash`,
2. executor concurrency controls stay operational and non-authoritative,
3. continuation admission stays centralized,
4. receipt handling is never forced to wait on effect-start capacity.

If detached execution starts exposing unbounded-open-work failure modes in practice, this document
should become the basis for the next focused cut.
