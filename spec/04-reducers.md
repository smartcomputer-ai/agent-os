# Reducers

Reducers are deterministic, WASM-compiled state machines that own application/domain state and business invariants. They consume events, update state, and may emit domain intents (events) and a constrained set of micro-effects (effects whose `origin_scope` allows reducers). Complex, multi-step external orchestration is handled by AIR plans; reducers remain focused on domain evolution.

**Note on examples**: Plan examples in this chapter use AIR v1's authoring sugar (plain JSON values) for readability. You may also use canonical JSON (tagged) or full `Expr` trees where `ExprOrValue` is accepted. See `spec/03-air.md` §3 for details on JSON lenses and canonicalization. Reducer WASM ABI boundaries always use canonical CBOR regardless of authoring format.

## Scope Note (v1.0 → v1.1)

v1 reducers treat the whole state as a single value. To model many FSM instances, author code as a `map<key, substate>`. Version 1.1 promotes this pattern to **Cells**: keyed reducers with per-cell state/mailboxes, still using the same `step` export. The kernel sets `ctx.cell_mode=true` and supplies `ctx.key`; returning `state=null` deletes the cell. See spec/06-cells.md for routing, storage, and migration details.

## Role and Boundaries

Reducers are the **single source of state changes**. They advance only when they receive events. Plans do not call reducers directly; they raise events the kernel routes to reducers.

**Business logic lives here**: validation, transitions, invariants, and shaping domain intents.

**Effects policy**: Reducers may emit only micro-effects (effects whose `origin_scope` includes reducers, e.g., `blob.put`, `timer.set` in v1) under explicit capability slots. High-risk or multi-hop external effects (email/HTTP to third parties, LLM, payments) must be orchestrated by plans.

**Orchestration handoff**: Reducers emit DomainIntent events (e.g., `ChargeRequested`, `NotifyCustomer`), which trigger plans via manifest triggers. Plans perform effects, await receipts, and raise result events back.

## Execution Environment (Deterministic WASM)

Reducers target core WASM (`wasm32-unknown-unknown`). No WASI, no threads, no ambient clock or randomness. Deterministic numerics are required; prefer `dec128` in values; normalize NaNs if floats are used internally.

All I/O happens via returned effects; reducers cannot perform syscalls. **Replay guarantee**: given the same input event stream and recorded receipts, reducers produce identical state bytes.

## ABI (Reducer)

### Export

`step(ptr, len) -> (ptr, len)`

### Input CBOR (canonical envelope)

```
{
  version: 1,
  state: <bytes|null>,
  event: <bytes>,
  ctx: { key?: <bytes>, cell_mode: <bool> }
}
```

- `state` is canonical CBOR matching the declared state schema; `null` is passed when creating a new cell.
- `event` is canonical CBOR of a DomainEvent or ReceiptEvent addressed to this reducer; the kernel validates and canonicalizes every event payload against its schema before routing/journaling, so reducers always see schema-shaped canonical CBOR.
- `ctx.cell_mode` is `true` when routed as a keyed reducer; `ctx.key` must be present in that mode and is advisory in v1 compatibility mode.

### Output CBOR (canonical)

```
{
  state: <bytes|null>,
  domain_events?: [{schema: <Name>, value: <Value>}],
  effects?: [<ReducerEffect>],
  ann?: <annotations>
}
```

- `state`: new canonical state bytes; `null` deletes a cell when `cell_mode=true`
- `domain_events` (optional): zero or more domain events (including DomainIntent); kernel appends them to the journal and routes by manifest
- `effects` (optional): micro-effects only (as defined by `origin_scope`); see Effect Emission
- `ann` (optional): structured annotations for observability

### ReducerEffect Shape

Semantic contract from reducer to kernel:

```
{
  kind: EffectKind,
  params: Value,
  cap_slot?: string
}
```

- `kind`: must be in reducer's declared `effects_emitted` allowlist
- `cap_slot` (optional): abstract slot to bind a concrete CapGrant via `manifest.module_bindings`

`EffectKind` is a namespaced string. The v1 kernel ships the built-in catalog listed in spec/03-air.md §7; adapter-defined kinds require runtime support to map to capabilities and receipts.

## Events Seen By Reducers

Reducers consume two kinds of events:

**DomainEvent**: Business events and intents. Produced by other reducers or plans via `raise_event`. Versioned by schema Name.

**ReceiptEvent**: Adapter receipts converted to events by the kernel for micro-effects (e.g., `TimerFired` for `timer.set`). These are **wrapped into the reducer’s ABI event variant** before routing, so reducers should include the built-in receipt schemas as variant arms. If a reducer emits a micro-effect, its ABI event schema must either be that receipt schema itself or a variant with a `ref` to it. For complex external work, plans raise result DomainEvents instead of relying on raw receipts. Correlate using stable fields in your events/effect params (e.g., a key like `order_id` or an idempotency key). In v1.1 Cells, this key becomes a first-class route; see spec/06-cells.md.

**Important**: Do not embed `$schema` fields inside reducer event payloads. The kernel determines schemas from manifest routing and capability bindings; self-describing payloads are rejected.

**Normalization**: All events—emitted by reducers, raised by plans, synthesized from receipts, or injected externally—are schema-validated and canonicalized on ingress. The journal stores only canonical CBOR, and keyed routing/correlation uses the decoded typed value. Reducers can deserialize directly into their event enum/struct without handling ExprValue tagging.

Reducers should be written as explicit **typestate machines**: a `pc` (program counter) enum plus fences/idempotency to handle duplicates and retries. This makes continuations data-driven and deterministic.

## Effect Emission (Micro-Effects)

- Allowed from reducers: small, low-risk effects such as:
- `blob.put/get` (content-addressed)
  - `timer.set` (for backoff, deadlines)
- Disallowed from reducers (must go through plans):
  - `http.request` to third-party hosts, `llm.generate`, `email.send`, payments, provider SDK calls, etc.

All reducer-sourced effects pass through capability and policy gates. The v1 policy system enforces origin-aware rules:
- Policy Match rules can specify `origin_kind: "reducer"` to deny heavy effects from reducers
- Policy Match rules can specify `origin_kind: "plan"` to allow those same effects from plans
- The kernel populates origin metadata (origin_kind and origin_name) on every EffectIntent for policy evaluation
- Budgets settle on receipts

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
            domain_events.push(DomainEvent {
                schema: "com.acme/ChargeRequested@1".into(),
                value: cbor!({
                    "order_id": order_id,
                    "amount_cents": amount_cents
                })
            });
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

## Complete Example: Reducer → Plan → Reducer Flow

A typical pattern: reducer emits an intent, a plan performs the work, results return as events.

### 1. Reducer emits a DomainIntent

```rust
// In OrderSM reducer
match (&mut s.pc, input.event) {
    (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
        s.order_id = order_id.clone();
        s.pc = Pc::AwaitingPayment;
        // Emit DomainIntent to request external payment
        domain_events.push(DomainEvent {
            schema: "com.acme/ChargeRequested@1".into(),
            value: cbor!({
                "order_id": order_id,
                "amount_cents": amount_cents
            })
        });
    }
    // ... other transitions
}
```

### 2. Manifest trigger starts the plan

```json
{
  "triggers": [
    {
      "event": "com.acme/ChargeRequested@1",
      "plan": "com.acme/charge_plan@1",
      "correlate_by": "order_id"
    }
  ]
}
```

### 3. Plan performs the effect

Note: This example uses v1 authoring sugar. The `params` and `event` fields accept `ExprOrValue`, so you can provide plain record/variant values (as shown) or full `Expr` trees when dynamic computation is needed.

```json
{
  "$kind": "defplan",
  "name": "com.acme/charge_plan@1",
  "input": "com.acme/ChargeRequested@1",
  "steps": [
    {
      "id": "charge",
      "op": "emit_effect",
      "kind": "payment.charge",
      "params": {
        "order_id": {"ref": "@plan.input.order_id"},
        "amount_cents": {"ref": "@plan.input.amount_cents"}
      },
      "cap": "payment_cap",
      "bind": {"effect_id_as": "charge_id"}
    },
    {
      "id": "wait",
      "op": "await_receipt",
      "for": {"ref": "@var:charge_id"},
      "bind": {"as": "receipt"}
    },
    {
      "id": "notify_reducer",
      "op": "raise_event",
      "reducer": "com.acme/OrderSM@1",
      "event": {
        "record": {
          "order_id": {"ref": "@plan.input.order_id"},
          "success": {
            "op": "eq",
            "args": [
              {"ref": "@var:receipt.status"},
              "ok"
            ]
          },
          "txn_id": {"ref": "@var:receipt.txn_id"}
        }
      }
    },
    {"id": "done", "op": "end"}
  ],
  "edges": [
    {"from": "charge", "to": "wait"},
    {"from": "wait", "to": "notify_reducer"},
    {"from": "notify_reducer", "to": "done"}
  ],
  "required_caps": ["payment_cap"],
  "allowed_effects": ["payment.charge"]
}
```

### 4. Reducer consumes the result

```rust
match (&mut s.pc, input.event) {
    (Pc::AwaitingPayment, Event::PaymentResult { success, txn_id }) => {
        if success {
            s.pc = Pc::Paid;
            s.txn_id = Some(txn_id);
            // Could emit next intent (e.g., ReserveInventory)
        } else {
            s.pc = Pc::Failed;
            s.last_error = Some("Payment failed".into());
        }
    }
    // ... other transitions
}
```

This cycle keeps domain logic in reducers and external effects in plans. The plan acts as a deterministic, auditable orchestrator under capabilities and policy.

## ReceiptEvent And Correlation

- ReceiptEvent envelope (conceptual): `{ intent_hash, effect_kind, adapter_id, status, receipt: <EffectReceiptValue> }`.
  - For micro-effects (timer, blob), the kernel converts receipts into standard payloads (`sys/TimerFired@1`, `sys/BlobPutResult@1`, …) and **wraps them into the reducer’s ABI event variant** before routing.
  - For complex effects (payments/email/http/llm), prefer plans to raise explicit result DomainEvents instead of relying on raw receipts.
- Correlation: include stable domain ids in effect params (e.g., `order_id` or idempotency keys). Reducers match on those fields in events.

## Micro-Effects Allowlist

- Allowed from reducers in v1: `blob.put`, `blob.get`, `timer.set`.
- All other effect kinds should be denied by policy when originating from reducers and executed via plans instead.

## Effect Boundaries and Guardrails

To maintain clear separation between reducers and plans, the kernel enforces:

### Reducer Effect Limits (v1)
- **At most ONE effect per step**: reducers emit zero or one effect per invocation
- **Only "micro-effects"**: effects whose `origin_scope` allows reducers (currently `blob.put`, `blob.get`, `timer.set`)
- **NO network effects**: `http.request`, `llm.generate`, `email.send`, `payment.charge` must go through plans

### Policy Enforcement (v1)
- EffectKind not in declared `effects_emitted` → rejected by validator at load time
- Disallowed kinds from reducers → policy denial at runtime via origin-aware rules:
  - Policy rules with `{ origin_kind: "reducer", effect_kind: "llm.generate" }` → decision: "deny"
  - Policy rules with `{ origin_kind: "plan", effect_kind: "llm.generate" }` → decision: "allow"
- Multiple effects in single step → rejected with diagnostic: "Reducers may emit at most one effect per step; lift complex orchestration to a plan"
- The kernel attaches `origin_kind="reducer"` and `origin_name=<module_name>` to all reducer-emitted effect intents for policy matching

### Intent-Based Pattern
- **Complex workflows**: emit DomainIntent event → manifest trigger starts plan → plan performs effects → plan raises result event → reducer consumes result
- **Micro-effects**: emit directly from reducer if effect kind is in allowlist and bound to a valid `cap_slot`
- **Governance and approvals**: deferred to v1.1+; v1 policy supports only allow/deny decisions

### Enforcement Points
- **Static validation**: defmodule `effects_emitted` checked against micro-effects allowlist
- **Runtime policy gate**: checks effect source (reducer vs plan), denies prohibited combinations
- **Linter warnings**: flag reducers emitting DomainIntents with no matching trigger in manifest

This boundary ensures that high-risk external effects flow through the auditable, governable plan layer while keeping simple, low-risk effects available to reducers for tight state transitions.

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

- Capabilities: bind reducer cap slots in manifest; keep scopes tight; budgets enforced on receipts with conservative pre-checks for variable-cost effects (LLM tokens, blob sizes).
- Policy (v1): use origin-aware rules to deny high-risk effect kinds from reducers (origin_kind="reducer") and allow them from plans (origin_kind="plan"). Default-deny posture recommended.
- Tracing: include correlation ids (e.g., order_id) in events and annotate outputs for observability.
- Upgrades: pin schema versions; treat changes as new module versions; let in-flight instances finish under old semantics.

## Summary

Reducers are deterministic, WASM-compiled domain state machines. They own state and business logic, emit domain events (including intents), and may perform only micro-effects under strict policy. Plans are the orchestration layer for external work: they turn intents into effects under capabilities and policy, await receipts, and raise results back as events. This split keeps state mutation on a single rail, concentrates governance at explicit choke points, and preserves determinism and auditability.
