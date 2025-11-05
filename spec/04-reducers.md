# Reducers

Reducers are deterministic, WASM-compiled state machines that own application/domain state and business invariants. They consume events, update state, and may emit domain intents (events) and a constrained set of micro-effects. Complex, multi-step external orchestration is handled by AIR plans; reducers remain focused on domain evolution.

Note on scope (v1.0)
- This chapter describes reducers for v1.0: a reducer has a single state value. If you need many parallel instances of the same FSM, model them as a map<key, substate> inside this state (see “Coding Pattern” and “ReceiptEvent And Correlation”).
- In v1.1, “Cells” make per-key instances first-class (per-cell state and mailboxes) with the same reducer ABI. See: spec/05-cells.md.

## Role And Boundaries

- Single source of state changes: reducers advance only when they receive events. Plans do not call reducers directly; they raise events the kernel routes to reducers.
- Business logic lives here: validation, transitions, invariants, and shaping domain intents.
- Effects policy: reducers may emit only micro-effects (e.g., fs.blob.put, timer.set) under explicit capability slots. High-risk or multi-hop external effects (email/http to third parties, LLM, payments) must be orchestrated by plans.
- Orchestration handoff: reducers emit DomainIntent events (e.g., ChargeRequested, NotifyCustomer), which trigger plans via manifest triggers. Plans perform effects, await receipts, and raise result events back.

## Execution Environment (Deterministic WASM)

- Target: core wasm (wasm32-unknown-unknown). No WASI, no threads, no ambient clock or randomness.
- Deterministic numerics; prefer dec128 in values; normalize NaNs if floats used internally.
- All I/O via returned effects; reducers cannot perform syscalls.
- Replay: given the same input event stream and recorded receipts, reducers produce identical state bytes.

## ABI (Reducer)

- Export: `step(ptr, len) -> (ptr, len)`
- Input CBOR (canonical): `{ state: <bytes>, event: <bytes> }`
  - `state` is canonical CBOR matching the declared state schema.
  - `event` is canonical CBOR of a DomainEvent or a ReceiptEvent addressed to this reducer.
- Output CBOR (canonical): `{ state: <bytes>, domain_events?: [ { schema: <Name>, value: <Value> } ], effects?: [ ReducerEffect ], ann?: <annotations> }`
  - `state`: new canonical state bytes.
  - `domain_events` (optional): zero or more domain events (including DomainIntent). Kernel appends them to the journal and routes by manifest.
  - `effects` (optional): micro-effects only; see Effect Emission.
  - `ann` (optional): structured annotations for observability.

ReducerEffect shape (semantic contract from reducer to kernel):
- `{ kind: EffectKind, params: Value, cap_slot?: string }`
  - `kind`: must be in reducer’s declared `effects_emitted` allowlist.
  - `cap_slot` (optional): abstract slot to bind a concrete CapGrant via manifest.module_bindings.

## Events Seen By Reducers

- DomainEvent: business events and intents. Produced by other reducers or plans via raise_event. Versioned by schema Name.
- ReceiptEvent: adapter receipts converted to events by the kernel for micro-effects (e.g., TimerFired for timer.set). For complex external work, plans raise result DomainEvents instead of relying on raw receipts. Correlate using stable fields in your events/effect params (e.g., a key like order_id or an idempotency key). In v1.1 Cells, this key becomes a first-class route; see spec/05-cells.md.

Reducers should be written as explicit typestate machines: a `pc` (program counter) enum plus fences/idempotency to handle duplicates and retries. This makes continuations data-driven and deterministic.

## Effect Emission (Micro-Effects)

- Allowed from reducers: small, low-risk effects such as:
  - `fs.blob.put/get` (content-addressed)
  - `timer.set` (for backoff, deadlines)
- Disallowed from reducers (must go through plans):
  - `http.request` to third-party hosts (unless narrowly allowlisted by policy), `llm.generate`, `email.send`, payments, provider SDK calls, etc.

All reducer-sourced effects pass through capability and policy gates. Gates can reject disallowed kinds or require that an effect originate from a plan. Budgets settle on receipts.

## Relationship To Plans

- Triggering plans: reducers emit DomainIntent events that the manifest maps to plans via triggers. The kernel starts plan instances with the intent as input.
- Plans perform external work: plans emit effects, await receipts, and raise result events back to reducers (e.g., PaymentResult, NotificationSent).
- No direct invocation: plans do not invoke reducers; they communicate only by events (raise_event). Reducers do not call plans; they publish intents as events.

This boundary yields:
- One rail for state changes (events → reducers)
- One rail for external orchestration (plans → effects → receipts → events)
- Clear choke points for governance and approval (emit_effect in plans)

## Module Definition (defmodule: reducer)

- `module_kind`: `reducer`
- `abi.reducer`:
  - `state`: SchemaRef (canonical CBOR enforced at boundaries)
  - `event`: SchemaRef (domain/receipt event type family)
  - `annotations`?: SchemaRef
  - `effects_emitted`?: [EffectKind] — whitelist for static checks
  - `cap_slots`?: { slot_name → CapType } — abstract capability slots

Binding capability slots:
- In manifest.module_bindings: map `{ module_name → { slots: { slot_name → CapGrantName } } }`
- Policy can still deny at dispatch; bindings only select which grant to use.

## Coding Pattern (Rust)

A minimal reducer skeleton using the aos-wasm-sdk style ABI.

```rust
use serde::{Serialize, Deserialize};
use aos_wasm_sdk::{StepInput, StepOutput, EffectIntent};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct OrderState { pub pc: Pc, pub order_id: String, pub fences: Fences }

#[derive(Serialize, Deserialize, Clone)]
pub enum Pc { Idle, PendingPayment, Done, Failed }
impl Default for Pc { fn default() -> Self { Pc::Idle } }

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Fences { pub payment_done: bool }

#[derive(Serialize, Deserialize, Clone)]
pub enum Event {
    OrderCreated { order_id: String, amount_cents: u64 },
    PaymentResult { order_id: String, ok: bool, txn_id: Option<String> },
}

#[no_mangle]
pub extern "C" fn step(ptr: i32, len: i32) -> (i32, i32) { aos_wasm_sdk::entry(step_impl, ptr, len) }

fn step_impl(input: StepInput<OrderState, Event>) -> StepOutput<OrderState, serde_cbor::Value> {
    let mut s = input.state;
    let mut domain_events = Vec::new();
    let mut effects: Vec<EffectIntent<serde_cbor::Value>> = vec![];

    match input.event {
        Event::OrderCreated { order_id, amount_cents } if matches!(s.pc, Pc::Idle) => {
            s.order_id = order_id.clone();
            s.pc = Pc::PendingPayment;
            // Emit a DomainIntent to trigger a plan (no external effect here)
            domain_events.push(cbor!({
                "$schema": "com.acme/ChargeRequested@1",
                "order_id": order_id,
                "amount_cents": amount_cents
            }));
        }
        Event::PaymentResult { order_id: _, ok, txn_id: _ } if matches!(s.pc, Pc::PendingPayment) => {
            if ok { s.pc = Pc::Done; } else { s.pc = Pc::Failed; }
        }
        _ => {}
    }

    // Return domain_events as a first-class field per reducer ABI
    StepOutput { state: s, effects, domain_events, ann: None }
}
```

Notes
- DomainIntent is just a domain event with a schema that your manifest trigger references. The kernel appends it and starts the configured plan.
- Result events (e.g., PaymentResult) are raised by the plan and delivered to this reducer as `Event`.
- For micro-effects (e.g., timer): push a ReducerEffect with `kind: "timer.set"` and `cap_slot: Some("timer")` and ensure a binding exists.

## ReceiptEvent And Correlation

- ReceiptEvent envelope (conceptual): `{ intent_hash, effect_kind, adapter_id, status, receipt: <EffectReceiptValue> }`.
  - For micro-effects (timer, blob), the kernel may convert receipts into standard domain events (e.g., `sys/TimerFired@1`).
  - For complex effects (payments/email/http/llm), prefer plans to raise explicit result DomainEvents instead of relying on raw receipts.
- Correlation: include stable domain ids in effect params (e.g., `order_id` or idempotency keys). Reducers match on those fields in events.

## Micro-Effects Allowlist

- Allowed from reducers in v1: `fs.blob.put`, `fs.blob.get`, `timer.set`.
- All other effect kinds should be denied by policy when originating from reducers and executed via plans instead.

## Capability Slot Resolution

- Reducers declare `cap_slots` and refer to slots in emitted effects.
- Resolution order (reducers): `manifest.module_bindings` provides concrete CapGrants for slots; if missing or incompatible, the kernel rejects the effect.
- Plans do not override reducer cap slots in v1 (since plans don’t invoke reducers).

## ABI Pragmatics

- Reducers run in a single linear memory; the host copies CBOR input into memory and reads CBOR output back.
- Keep payloads modest; large data should be passed by content address (blob refs) via the blob store.
- Ensure canonical CBOR at boundaries; normalize floats or avoid them in values.

## Error Taxonomy And Retries

- Classify error outcomes in result events (or receipt-derived events) as retryable vs terminal.
- Use `timer.set` for backoff and retry scheduling; store attempt counters and deadlines in state; fence duplicates with idempotency keys and done flags.
- For compensations, emit explicit DomainIntents (e.g., `RefundRequested`) to trigger plans that perform compensating effects.

## Event Schemas And Versioning

- All DomainEvents and ReceiptEvents should be declared as defschema entries (versioned by Name, e.g., `com.acme/PaymentResult@1`).
- Reducers should validate or pattern-match on schema versions and handle upgrades by introducing new event variants and states.

## Typestate And Continuations

For multi-step local workflows inside a reducer (micro-sagas):
- Encode continuation as a `pc` enum plus durable locals and fences.
- On each receipt event, match the current `pc` and advance.
- Idempotency: derive idempotency keys from stable inputs and store “done” flags; ignore duplicates.
- Retries/backoff: schedule `timer.set` and move to a waiting state; on timer receipt, retry emission (respecting max attempts in state).

Provide a small helper (aos-saga) to reduce boilerplate:
- Effect builders that attach idempotency keys
- Fences utilities (mark_done/check_duplicate)
- Timer/backoff helpers
- Trace annotations to improve observability

## Testing And Determinism

- Unit-test reducer transitions: (state,event) -> (state, domain_events/effects)
- Golden tests with canned receipts: feed a sequence of events/receipts and assert final state bytes.
- Replay tests: serialize the journal, replay, and assert identical snapshot bytes.
- Schema discipline: ensure all state/event values are canonical CBOR of declared schemas.

## Anti-Patterns

- Orchestrating multi-effect external workflows inside reducers (mixing payment/email/http) — lift to plans.
- Emitting unbounded numbers of effects per step — keep to zero or one micro-effect.
- Using wall-clock or randomness — derive from event/log context instead; schedule timers for delays.
- Cross-calling other reducers — communicate via events only.

## Operational Guidance

- Capabilities: bind reducer cap slots in manifest; keep scopes tight; budgets enforced on receipts.
- Policy: deny high-risk effect kinds from reducers; require plans or approvals.
- Tracing: include correlation ids (e.g., order_id) in events and annotate outputs for observability.
- Upgrades: pin schema versions; treat changes as new module versions; let in-flight instances finish under old semantics.

## Summary

Reducers are deterministic, WASM-compiled domain state machines. They own state and business logic, emit domain events (including intents), and may perform only micro-effects under strict policy. Plans are the orchestration layer for external work: they turn intents into effects under capabilities and policy, await receipts, and raise results back as events. This split keeps state mutation on a single rail, concentrates governance at explicit choke points, and preserves determinism and auditability.
