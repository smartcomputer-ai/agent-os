# External Execution

This document defines the runtime seam for workflow-origin external work.
It refines the workflow runtime contract in [spec/05-workflows.md](05-workflows.md) by separating:

- authoritative owner-side state progression,
- external execution,
- continuation admission.

## 1) Scope

This document covers:

- durable open external work,
- owner/executor responsibilities,
- continuation identity and admission,
- out-of-order progression across multiple open effects,
- restart and quiescence principles.

It does **not** define:

- fabric/session/artifact/log products,
- concrete queue or worker topology,
- executor-specific timeout policy.

## 2) Core Split

### Owner

The owner owns:

- deterministic workflow stepping,
- capability and policy enforcement,
- journaling open work,
- continuation routing and admission,
- quiescence/governance visibility.

### Executor

The executor owns:

- reconciling open work against an execution substrate,
- optionally claiming or starting work,
- observing progress,
- producing terminal outcomes.

Executors are non-authoritative.
They never mutate world state directly.

### Owner-local timer executor

Timers remain owner-local internal execution, but still participate in the same open-work and
continuation model:

1. timer work is opened durably,
2. owner-local scheduling tracks due time,
3. due delivery re-enters through ordinary continuation admission,
4. the timer remains open work until terminal settlement.

## 3) Identity And Lifecycle

### 3.1 Durable open work

Workflow-origin external work becomes authoritative in this order:

1. workflow emits effect intent,
2. owner validates/canonicalizes and records open work,
3. executor may then reconcile or start external execution.

### 3.2 Open-work identity

`intent_hash` is the authoritative runtime identity for one open effect.

For workflow-origin effects, this is not merely a hash of effect params in isolation.
The runtime mints uniqueness before hashing by deriving the effective workflow idempotency input
from origin identity and emission position, so `intent_hash` already behaves as the per-emission
open-work id used for:

- pending/open owner state,
- continuation routing,
- stream fencing,
- replay,
- quiescence.

Executors may still maintain separate attempt/provider identities such as:

- `attempt_id`,
- `operation_id`,
- provider-native handles.

Those are executor-operational identities, not second owner-side effect ids.

### 3.3 Lifecycle

The authoritative coarse lifecycle is:

`open -> terminal`

Where:

- `open` means the owner has recorded durable open work,
- `terminal` means the owner has admitted terminal settlement.

Optional non-terminal continuations remain observations on an open effect.

The recommended continuation shape remains:

1. zero or more stream frames,
2. exactly one terminal receipt.

Stream frames may update per-effect observation metadata, but they do not create a new coarse
owner lifecycle phase and do not settle the effect by themselves.

## 4) Admission And Ordering

### 4.1 Owner admission only

Only the owner may:

- admit stream frames,
- admit terminal receipts,
- settle open work,
- advance authoritative world state.

### 4.2 Out-of-order progression

Distinct open effects may progress and settle out of original emission order.

This is valid as long as:

- each effect instance preserves its own sequencing/fencing rules,
- owner admission remains canonical and replayable.

### 4.3 Runtime caches are not authoritative

Local queues, task handles, inflight maps, and channels are runtime cache only.
This includes transport-local notions such as "submitted to handler" or "published to queue."

Durable authority is:

1. journaled open work,
2. pending continuation routing context,
3. admitted continuations,
4. admitted terminal receipts.

## 5) Recovery And Quiescence

### 5.1 Recovery

On restart, recovery must proceed from durable state rather than live task memory.

Owner restart rebuilds routing and runtime cache from replayed open work.
Executor restart reconciles replayed open work against its execution substrate.
No second persisted effect-state source is required for restart.

### 5.2 Started evidence

Once admitted non-terminal evidence shows that external work exists, recovery is no longer deciding
whether stale work may begin from scratch.
It is deciding how to reattach to, observe, or settle already-existing work.

That evidence remains observation on an open effect until terminal settlement.

### 5.3 Quiescence

Open work remains visible for:

- diagnostics,
- governance,
- strict quiescence,
- manifest apply safety.

No implicit abandonment of open work occurs during apply.

## 6) Relationship To Workflows

Workflow code continues to own:

- business state,
- retries and compensation decisions,
- business deadlines and timer-driven escalation.

The owner/executor seam only governs:

- when external work becomes durable open work,
- how external progress is admitted,
- how terminal settlement re-enters workflow progression.
