# Workflows

Workflows are the orchestration unit in AgentOS. A workflow module is a deterministic state machine that consumes canonical events, updates its state, emits domain events, and requests external effects through the kernel's capability and policy gates.

This document describes the active workflow runtime contract: what a workflow owns, how it is invoked, how effects and receipts move through the system, and which invariants make replay, audit, and governance reliable.

## 1) Scope

Workflow orchestration is code-defined and event-driven:
- `defmodule` with `module_kind: "workflow"` is the orchestration/state-machine unit.
- `pure` modules are deterministic compute helpers and do not emit effects.
- Manifest startup and domain ingress wiring use `routing.subscriptions`.

In practice, a workflow owns the end-to-end progression of a business process:
- it receives a domain event or receipt continuation
- it decides the next state transition
- it emits follow-up domain events for other workflows or observers
- it requests side effects when external work is required
- it resumes when receipts arrive for previously emitted intents

Workflow instances may be unkeyed or keyed. Keyed workflows partition state by instance key and are specified in detail in [spec/06-cells.md](06-cells.md).

## 2) Responsibility Split

Workflow modules own:
- domain state
- business invariants
- transition logic
- retry/compensation policy decisions

Kernel + execution runtime own:
- deterministic stepping
- capability checks
- policy checks
- effect emission and open-work tracking
- continuation admission and receipt ingestion

Executors/adapters own:
- side-effect execution
- non-authoritative progress reporting
- signed receipt production

This split keeps orchestration logic in workflow code while preserving a small deterministic runtime:
- workflow code decides what should happen
- the kernel decides whether and when it is allowed to happen
- executors perform the effect and return auditable continuations

The owner/executor seam for open external work is defined in
[spec/07-external-execution.md](07-external-execution.md).

## 3) Normative Runtime Contract

### 3.1 Authority and effect emission

1. Only workflow modules may originate module-emitted effects.
2. `pure` modules cannot emit effects.
3. Workflow modules must declare `abi.workflow.effects_emitted`.
4. Kernel rejects undeclared effect kinds before capability/policy evaluation.
5. Multiple effects per step are allowed; deterministic kernel output limits apply.

### 3.2 Deterministic canonicalization

1. Event payloads are schema-validated and canonicalized on ingress.
2. Effect params are schema-validated and canonicalized before intent hashing/enqueue.
3. Receipt payloads are schema-validated/canonicalized before continuation delivery.
4. Journal + snapshot persist canonical CBOR forms used for replay.
5. Runtime decode fallbacks for non-canonical event/receipt payload shapes are not part of the active contract.

### 3.3 Continuation routing contract

1. Receipt continuation routing is keyed by recorded origin identity:
- `origin_module_id`
- `origin_instance_key`
- `intent_id`/intent hash identity
2. Intent identity preimage includes origin instance identity to avoid ambiguous concurrent wakeups.
3. Continuation routing is manifest-independent.
4. `routing.subscriptions` is for domain-event ingress only.

### 3.4 Receipt envelope contract

Settled effects produce a generic workflow receipt envelope (schema family includes `sys/EffectReceiptEnvelope@1`) with at least:
- origin module identity
- origin instance key (if keyed)
- intent identity
- effect kind
- optional issuer reference echoed from the emitted effect
- receipt payload bytes
- receipt status
- emitted sequence metadata

### 3.5 Receipt fault handling

If receipt payload decoding/normalization fails:
1. The failing intent is settled (removed from pending).
2. If workflow event schema supports `sys/EffectReceiptRejected@1`, kernel emits it.
3. If not supported, kernel marks the workflow instance failed and drops remaining pending receipts for that instance (fault isolation, no global clogging).

### 3.6 Persisted workflow instance model

Kernel persists workflow instance runtime state (conceptually including):
- state bytes
- inflight intent set/map
- lifecycle status: `running|waiting|completed|failed`
- last processed sequence marker
- module version/hash metadata (for diagnostics)

Replay must restore this state deterministically.

### 3.7 Apply safety (strict quiescence)

Manifest apply is blocked when any of the following hold:
1. non-terminal workflow instances exist
2. any workflow has inflight intents
3. effect queue/scheduler still has pending work

No implicit abandonment/clearing of in-flight workflow state during apply.

### 3.8 Governance and shadow semantics

Shadow/governance reporting is bounded to observed execution horizon:
- observed effects so far
- pending workflow receipts/intents
- workflow instance statuses
- module effect allowlists
- relevant state/ledger deltas

No guarantee of complete static future-effect prediction for unexecuted branches.

## 4) Runtime Flow

1. Domain event is appended and canonicalized.
2. Router evaluates `routing.subscriptions` and delivers to matching workflow modules.
3. Workflow `step` runs deterministically with current state + event.
4. Workflow returns new state, domain events, and effect intents.
5. Kernel enforces `effects_emitted` allowlist, then caps and policy, and records open work.
6. Executors may run allowed external work independently and emit stream frames and a terminal
   receipt.
7. Kernel canonicalizes admitted continuations and routes them to the recorded origin instance.

## 5) Workflow Module Contract

Workflow modules declare reducer ABI fields:
- `state`: state schema
- `event`: event schema
- `context` (optional)
- `effects_emitted` (required for effecting modules)
- `cap_slots` (optional slot -> cap type)

Manifest binds slots via `module_bindings`.

## 6) Routing Contract

`routing.subscriptions` maps event schema -> module:
- required fields are `event`, `module`; `key_field` is used for keyed module delivery
- deterministic evaluation order is manifest order
- matching subscriptions fan out in order

Continuation delivery from receipts does not use this routing table.

## 7) Conceptual Patterns

### Pattern A: Single workflow state machine

Best when business transitions, retries, and compensations are tightly coupled.

### Pattern B: Multi-module choreography

Best when contexts/teams are split; modules communicate through domain events.

### Pattern C: Timer + receipt driven progression

Best for deadlines, backoff, and long-running lifecycle checkpoints.

## 8) Minimal Examples

### 8.1 Workflow transition sketch (Rust, conceptual)

```rust
enum Pc { Idle, AwaitingCharge, Done, Failed }

match (state.pc, event) {
    (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
        state.order_id = order_id;
        state.pc = Pc::AwaitingCharge;
        effects.push(emit("payment.charge", params, Some("payments")));
    }
    (Pc::AwaitingCharge, Event::EffectReceiptEnvelope { status, .. }) => {
        state.pc = if status == "ok" { Pc::Done } else { Pc::Failed };
    }
    _ => {}
}
```

### 8.2 Manifest routing + binding sketch

```json
{
  "routing": {
    "subscriptions": [
      {
        "event": "com.acme/OrderEvent@1",
        "module": "com.acme/order_workflow@1",
        "key_field": "order_id"
      }
    ]
  },
  "module_bindings": {
    "com.acme/order_workflow@1": {
      "slots": { "payments": "cap_payments" }
    }
  }
}
```

## 9) Reliability Checklist

1. Include stable correlation fields in events/effect params.
2. Use explicit idempotency keys for externally visible effects.
3. Treat all continuation payloads as schema-bound inputs.
4. Keep terminal states and duplicate fences in module state.
5. Model retries with explicit attempt/backoff state.

## 10) Testing Checklist

1. Transition tests: `(state,event)->(state,events,effects)`.
2. Receipt progression tests for `ok/error/timeout/fault` paths.
3. Replay-or-die snapshot equivalence tests.
4. Concurrency tests: no cross-delivery between keyed instances.
5. Apply-safety tests: strict-quiescence block/unblock behavior.
