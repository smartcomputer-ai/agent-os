# Workflows

This document is the single workflow guide for AgentOS. It merges the old reducer-focused and workflow-pattern docs into one conceptual model with a small set of examples.

## 1) What A Workflow Is

A workflow is a deterministic state machine implemented as a WASM module with `module_kind: "workflow"` (reducer ABI).

Core properties:
- Owns domain state and business invariants.
- Advances only through events.
- Emits explicit effects (capability + policy gated).
- Continues from normalized receipt events.
- Replays byte-identically from journal + receipts.

There is no separate plan/trigger authoring model in active workflow runtime semantics.

## 2) Runtime Mental Model

One event enters a workflow module at a time:

1. Kernel routes a normalized event (`routing.subscriptions`).
2. Workflow `step` runs deterministically against current state.
3. Workflow returns:
- next state
- zero or more domain events
- zero or more effect intents
4. Effects are authorized (capability constraints + policy).
5. Adapter receipts are normalized and re-enter as events (typically `sys/EffectReceiptEnvelope@1`; optional `sys/EffectReceiptRejected@1` for malformed payload handling).

State changes only through event handling.

## 3) Contract Surface

Workflow modules declare:
- `abi.reducer.state`: state schema
- `abi.reducer.event`: event schema
- `abi.reducer.effects_emitted`: effect kind allowlist
- `abi.reducer.cap_slots`: abstract slots -> cap types

Manifest provides:
- `routing.subscriptions`: event schema -> workflow module
- `module_bindings`: module slots -> concrete cap grants

Policy evaluates each effect intent with origin metadata (`workflow|system|governance`) and defaults to deny when no rule matches.

## 4) Design Boundaries

Keep inside workflow module:
- business rules
- state transitions
- retry/compensation policy decisions
- idempotency fences

Keep outside workflow module:
- non-deterministic execution (adapters)
- authorization decisions (cap enforcer + policy)
- transport/integration concerns (HTTP providers, LLM vendors, etc.)

## 5) Core Patterns

### Pattern A: Single-Module Stateful Workflow

Use one workflow module to manage full domain lifecycle with explicit states.

Good for:
- strong invariants
- retry/compensation logic tied to domain state
- long-running flows

### Pattern B: Event Choreography Across Modules

Split by bounded context. One module emits domain events, others subscribe.

Good for:
- team/service boundaries
- independent versioning
- simpler module responsibilities

### Pattern C: Timer + Receipt Driven Continuations

Use `timer.set` plus receipt events for timeouts/backoff and async checkpoints.

Good for:
- retries with delay
- human-in-loop deadlines
- long-running orchestration

## 6) Minimal Example

### Workflow state machine (conceptual Rust)

```rust
enum Pc { Idle, AwaitingCharge, Done, Failed }

match (state.pc, event) {
    (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
        state.order_id = order_id;
        state.pc = Pc::AwaitingCharge;
        effects.push(emit("payment.charge", params, Some("payments")));
    }
    (Pc::AwaitingCharge, Event::EffectReceiptEnvelope { status, receipt_payload, .. }) => {
        if status == "ok" { state.pc = Pc::Done; } else { state.pc = Pc::Failed; }
    }
    _ => {}
}
```

### Routing + bindings (manifest excerpt)

```json
{
  "routing": {
    "subscriptions": [
      { "event": "com.acme/OrderEvent@1", "module": "com.acme/order_workflow@1" }
    ]
  },
  "module_bindings": {
    "com.acme/order_workflow@1": {
      "slots": { "payments": "cap_payments" }
    }
  }
}
```

## 7) Reliability Rules

- Always include stable correlation fields in domain events and effect params.
- Attach explicit idempotency keys for externally visible effects.
- Treat receipt payloads as schema-validated inputs, never ad-hoc blobs.
- Use terminal states + fences to prevent duplicate downstream intents.
- Model retries in state (attempt counters, backoff schedule, max attempts).

## 8) Anti-Patterns

- Business decisions in adapters or policy rules.
- Hidden side effects outside `effects` output.
- Coupling modules through direct calls instead of events.
- Unbounded retries without terminal failure state.
- Skipping replay tests for multi-step workflows.

## 9) Testing Guidance

Minimum coverage:
- transition unit tests: `(state, event) -> (state, events, effects)`
- receipt progression tests (ok/error/timeout)
- replay-or-die tests from genesis journal to byte-identical snapshot
- schema compatibility tests for state/event upgrades

## 10) Migration Notes (Plan-Era -> Workflow Runtime)

- `defplan` and manifest `triggers` are legacy compatibility concepts.
- Active runtime entry is event routing via `routing.subscriptions`.
- Move orchestration into workflow module typestate and receipt handling.
- Keep policy/capability governance unchanged: explicit grants, explicit decisions, explicit receipts.
