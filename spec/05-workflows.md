# Workflow Patterns and Coordination

This document describes how to coordinate complex workflows in AgentOS using the primitives from AIR v1. It bridges the gap between "here are the building blocks" (spec/03-air.md, spec/04-reducers.md) and "here's how to build real systems."

## Goals and Scope

- Show how reducers and plans coordinate for common workflow patterns
- Provide decision guidance for choosing patterns
- Document complex scenarios: compensations, fan-out/join, timeouts, retries, human approvals
- Establish best practices and anti-patterns

This is v1 guidance. Future versions may add higher-level orchestration primitives.

## Coordination Primitives

AIR v1 provides four coordination mechanisms:

### 1. Plan → Reducer (raise_event)

Plans can raise events that are delivered to reducers.

```json
{
  "op": "raise_event",
  "reducer": "com.acme/OrderSM@1",
  "event": {
    "record": {
      "order_id": {"ref": "@plan.input.order_id"},
      "success": {"ref": "@var:charge_rcpt.ok"},
      "txn_id": {"ref": "@var:charge_rcpt.txn_id"}
    }
  }
}
```

**Semantics**: Kernel serializes the event, appends to journal, delivers to reducer on next tick. The reducer declaration already pins the payload schema, so authors only provide the payload fields.

### 2. Reducer → Plan (manifest triggers)

Reducers emit DomainIntent events; manifest triggers start plans.

Reducer:
```rust
domain_events.push(DomainEvent {
    schema: "com.acme/ChargeRequested@1".into(),
    value: cbor!({"order_id": order_id, "amount_cents": amount})
});
```

Manifest:
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

**Semantics**: When a matching event is appended, kernel starts a new plan instance with the event as input.

### 3. Plan → Plan (await_event)

Plans can wait for domain events (from other plans or reducers).

```json
{
  "op": "await_event",
  "event": "com.acme/WorkCompleted@1",
  "where": {
    "op": "eq",
    "args": [
      {"ref": "@event.correlation_id"},
      {"ref": "@plan.input.request_id"}
    ]
  },
  "bind": {"as": "work_result"}
}
```

**Semantics**: Plan step blocks until a matching event appears in the journal; `where` predicate filters.

### 4. Conditional Flow (edges with guards)

Plan edges can have boolean predicates.

```json
{
  "edges": [
    {
      "from": "charge",
      "to": "reserve",
      "when": {
        "op": "eq",
        "args": [{"ref": "@var:charge_rcpt.status"}, {"text": "ok"}]
      }
    },
    {
      "from": "charge",
      "to": "compensate_refund",
      "when": {
        "op": "ne",
        "args": [{"ref": "@var:charge_rcpt.status"}, {"text": "ok"}]
      }
    }
  ]
}
```

**Semantics**: Step becomes ready only if all predecessors are done AND guard evaluates to true.

## Four Main Patterns

### Pattern 1: Single-Plan Orchestration (Plan-Driven)

**Description**: One plan orchestrates the entire workflow, coordinating multiple effects and raising result events to reducers for state tracking.

**When to use**:
- Flow is deterministic with clear steps
- No complex business decisions mid-flight
- Want full preflight visibility (shadow-run shows entire flow)
- Need centralized governance/approval

**Structure**:
```
User action → Reducer validates & emits intent
  ↓
Trigger starts Plan
  ↓
Plan internally:
  1. emit_effect A
  2. await_receipt A
  3. conditional branch on A result
  4. emit_effect B
  5. await_receipt B
  6. raise_event FinalResult → Reducer
  ↓
Reducer updates state to terminal
```

**Example**: Order fulfillment

Reducer emits `ProcessOrderIntent`:
```rust
(Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
    s.order_id = order_id.clone();
    s.pc = Pc::Processing;
    domain_events.push(DomainEvent {
        schema: "com.acme/ProcessOrderIntent@1".into(),
        value: cbor!({"order_id": order_id, "amount_cents": amount_cents})
    });
}
```

Plan `fulfillment_plan@1`:
```json
{
  "$kind": "defplan",
  "name": "com.acme/fulfillment_plan@1",
  "input": "com.acme/ProcessOrderIntent@1",
  "steps": [
    {"id": "charge", "op": "emit_effect", "kind": "payment.charge", "params": {...}, "cap": "payment_cap", "bind": {"effect_id_as": "charge_id"}},
    {"id": "wait_charge", "op": "await_receipt", "for": {"ref": "@var:charge_id"}, "bind": {"as": "charge_rcpt"}},
    {"id": "reserve", "op": "emit_effect", "kind": "inventory.reserve", "params": {...}, "cap": "inventory_cap", "bind": {"effect_id_as": "reserve_id"}},
    {"id": "wait_reserve", "op": "await_receipt", "for": {"ref": "@var:reserve_id"}, "bind": {"as": "reserve_rcpt"}},
    {"id": "notify", "op": "emit_effect", "kind": "email.send", "params": {...}, "cap": "mailer_cap", "bind": {"effect_id_as": "email_id"}},
    {"id": "wait_notify", "op": "await_receipt", "for": {"ref": "@var:email_id"}, "bind": {"as": "email_rcpt"}},
    {"id": "raise_result", "op": "raise_event", "reducer": "com.acme/OrderSM@1", "event": {"record": {"order_id": {"ref": "@plan.input.order_id"}}}},
    {"id": "done", "op": "end"}
  ],
  "edges": [
    {"from": "charge", "to": "wait_charge"},
    {"from": "wait_charge", "to": "reserve", "when": {"op": "eq", "args": [{"ref": "@var:charge_rcpt.status"}, {"text": "ok"}]}},
    {"from": "reserve", "to": "wait_reserve"},
    {"from": "wait_reserve", "to": "notify", "when": {"op": "eq", "args": [{"ref": "@var:reserve_rcpt.status"}, {"text": "ok"}]}},
    {"from": "notify", "to": "wait_notify"},
    {"from": "wait_notify", "to": "raise_result"},
    {"from": "raise_result", "to": "done"}
  ],
  "required_caps": ["payment_cap", "inventory_cap", "mailer_cap"],
  "allowed_effects": ["payment.charge", "inventory.reserve", "email.send"]
}
```

**Pros**:
- Shadow-run shows entire flow up front
- Governance reviews all effects together (budgets, approvals)
- Clear audit trail in single plan instance
- Easy to add compensations as conditional branches

**Cons**:
- Plan becomes complex for very long flows
- Business logic bleeds into plan structure
- Harder to reuse sub-flows across plans

---

### Pattern 2: Multi-Plan Choreography (Event-Driven)

**Description**: Multiple small plans coordinate via events. Each plan is triggered by a domain event, performs its work, and raises the next event.

**When to use**:
- Loosely coupled phases
- Different teams own different plans
- Want independent plan evolution/versioning
- Clear service boundaries

**Structure**:
```
Event A → Plan 1 → emits Event B
Event B → Plan 2 → emits Event C
Event C → Plan 3 → emits Event D (terminal)
```

**Example**: Multi-service order processing

```
OrderPlaced event → charge_plan
  charge_plan: payment.charge → raise PaymentCompleted

PaymentCompleted event → reserve_plan
  reserve_plan: inventory.reserve → raise ReservationCompleted

ReservationCompleted event → notify_plan
  notify_plan: email.send → raise OrderCompleted
```

Manifest:
```json
{
  "triggers": [
    {"event": "com.acme/OrderPlaced@1", "plan": "com.acme/charge_plan@1", "correlate_by": "order_id"},
    {"event": "com.acme/PaymentCompleted@1", "plan": "com.acme/reserve_plan@1", "correlate_by": "order_id"},
    {"event": "com.acme/ReservationCompleted@1", "plan": "com.acme/notify_plan@1", "correlate_by": "order_id"}
  ]
}
```

Each plan is small:
```json
{
  "$kind": "defplan",
  "name": "com.acme/charge_plan@1",
  "input": "com.acme/OrderPlaced@1",
  "steps": [
    {"id": "charge", "op": "emit_effect", "kind": "payment.charge", ...},
    {"id": "wait", "op": "await_receipt", ...},
    {"id": "notify", "op": "raise_event", "reducer": "EventBus@1", "event": {...}},
    {"id": "done", "op": "end"}
  ]
}
```

**Pros**:
- Plans stay small and focused
- Easy to swap/upgrade individual plans
- Natural service boundaries
- Each plan can have independent policies/caps

**Cons**:
- No single view of end-to-end flow
- Shadow-run only sees next plan, not full saga
- Compensation requires separate trigger chain
- Harder to reason about overall state
- Correlation keys critical (must thread through events)

---

### Pattern 3: Reducer-Driven Steps (Reducer as State Machine)

**Description**: Reducer owns the workflow state machine. Each transition emits an intent; triggered plans are thin effect wrappers.

**When to use**:
- Complex business rules determine next steps
- Need shared saga patterns across reducers
- Want testable, versioned business logic
- Compensations involve business decisions

**Structure**:
```
Event X → Reducer (validates, decides next step)
  → emits Intent A
  → pc = AwaitingA

Trigger: simple_plan_A (1 effect → 1 result)

Event: Result A → Reducer (pc = AwaitingA)
  → validates result
  → emits Intent B (or Compensation Intent if A failed)
  → pc = AwaitingB or Compensating
```

**Example**: Order saga with reducer-driven compensation

Reducer `OrderSM`:
```rust
#[derive(Serialize, Deserialize)]
pub enum Pc {
    Idle,
    AwaitingPayment,
    AwaitingReservation,
    Compensating,
    Done,
    Failed
}

fn step_impl(input: StepInput<OrderState, Event>) -> StepOutput<OrderState, Value> {
    let mut s = input.state;
    let mut domain_events = Vec::new();

    match (&mut s.pc, input.event) {
        (Pc::Idle, Event::OrderCreated { order_id, amount_cents, items }) => {
            s.order_id = order_id.clone();
            s.amount_cents = amount_cents;
            s.items = items;
            s.pc = Pc::AwaitingPayment;
            domain_events.push(DomainEvent {
                schema: "com.acme/ChargeRequested@1".into(),
                value: cbor!({"order_id": order_id, "amount_cents": amount_cents})
            });
        }

        (Pc::AwaitingPayment, Event::ChargeResult { success, txn_id }) => {
            if success {
                s.payment_txn = Some(txn_id);
                s.pc = Pc::AwaitingReservation;
                domain_events.push(DomainEvent {
                    schema: "com.acme/ReserveRequested@1".into(),
                    value: cbor!({"order_id": s.order_id, "items": s.items})
                });
            } else {
                s.pc = Pc::Failed;
                s.last_error = Some("Payment failed".into());
            }
        }

        (Pc::AwaitingReservation, Event::ReserveResult { success, hold_id }) => {
            if success {
                s.reservation_hold = Some(hold_id);
                s.pc = Pc::Done;
            } else {
                // Business rule: refund payment if reservation fails
                s.pc = Pc::Compensating;
                domain_events.push(DomainEvent {
                    schema: "com.acme/RefundRequested@1".into(),
                    value: cbor!({"order_id": s.order_id, "txn_id": s.payment_txn.unwrap()})
                });
            }
        }

        (Pc::Compensating, Event::RefundResult { success }) => {
            if success {
                s.pc = Pc::Failed;
                s.last_error = Some("Reservation failed, payment refunded".into());
            } else {
                s.pc = Pc::Failed;
                s.last_error = Some("Reservation failed, refund also failed".into());
            }
        }

        _ => {} // ignore unmatched events (idempotency)
    }

    StepOutput { state: s, effects: vec![], domain_events, ann: None }
}
```

Plans are thin wrappers (one per intent type):
```json
{
  "$kind": "defplan",
  "name": "com.acme/charge_wrapper@1",
  "input": "com.acme/ChargeRequested@1",
  "steps": [
    {"id": "charge", "op": "emit_effect", "kind": "payment.charge", ...},
    {"id": "wait", "op": "await_receipt", ...},
    {"id": "result", "op": "raise_event", "reducer": "com.acme/OrderSM@1", "event": {"record": {"success": {"ref": "@var:rcpt.ok"}, ...}}},
    {"id": "done", "op": "end"}
  ]
}
```

**Pros**:
- Business logic in reducers (testable, versioned, replay)
- Plans are thin, reusable effect wrappers
- Reducer implements complex typestate machines
- Easy to share saga patterns via `aos-saga` helpers
- Compensations use same reducer logic

**Cons**:
- Shadow-run can't predict full flow (only sees next intent)
- Governance sees effects one at a time
- More reducer complexity (must manage continuations)
- Correlation keys critical

---

### Pattern 4: Hybrid (Plan Orchestrates, Reducer Tracks)

**Description**: Plan orchestrates effects, but raises intermediate events to reducer for canonical state tracking and business logic hooks.

**When to use**:
- Want both auditability (plan) and business logic (reducer)
- High-value workflows needing governance AND flexibility
- Need to inspect/query state mid-workflow

**Structure**:
```
Event: Intent → Reducer validates → emits ProcessIntent
  ↓
Trigger: orchestration_plan
  Plan:
    1. emit effect A
    2. await_receipt A
    3. raise_event A_Completed → Reducer (tracking)
    4. emit effect B
    5. await_receipt B
    6. raise_event B_Completed → Reducer
    7. raise_event FinalCompleted → Reducer
  ↓
Reducer tracks: Pending → A_Done → B_Done → Done
```

**Example**: Payment with tracking

Reducer:
```rust
(Pc::Pending, Event::ProcessPaymentIntent { order_id, amount }) => {
    s.order_id = order_id;
    s.amount = amount;
    s.pc = Pc::Processing;
    domain_events.push(DomainEvent {
        schema: "com.acme/PaymentOrchestrationRequested@1".into(),
        value: cbor!({"order_id": order_id, "amount": amount})
    });
}

(Pc::Processing, Event::ChargeCompleted { success, txn_id }) => {
    if success {
        s.txn_id = Some(txn_id);
        s.pc = Pc::AwaitingConfirmation;
    } else {
        s.pc = Pc::Failed;
    }
}

(Pc::AwaitingConfirmation, Event::ConfirmationCompleted { confirmed }) => {
    s.pc = if confirmed { Pc::Done } else { Pc::Compensating };
}
```

Plan:
```json
{
  "steps": [
    {"id": "charge", "op": "emit_effect", "kind": "payment.charge", ...},
    {"id": "wait_charge", "op": "await_receipt", ...},
    {"id": "notify_charge", "op": "raise_event", "reducer": "PaymentSM@1", "event": {...}},
    {"id": "confirm", "op": "emit_effect", "kind": "payment.confirm", ...},
    {"id": "wait_confirm", "op": "await_receipt", ...},
    {"id": "notify_confirm", "op": "raise_event", "reducer": "PaymentSM@1", "event": {...}},
    {"id": "done", "op": "end"}
  ]
}
```

**Pros**:
- Plan shows full orchestration (governance/shadow/audit)
- Reducer maintains canonical state (queryable)
- Clear separation: plan = how, reducer = what happened
- Can add business logic hooks without changing plan

**Cons**:
- More events to define/maintain
- Potential for plan/reducer state drift
- Extra coordination overhead
- More complex to reason about

## Complex Scenarios

### Runtime Enforcement & Visibility

The runtime now enforces the schema boundaries described in spec/03-air.md at execution time:

- `raise_event` payloads/keys are canonicalized against the reducer's declared schemas, and invalid payloads are rejected before journaling.
- `await_receipt` and `await_event` references are validated when the manifest is loaded, so orchestration bugs (missing handles, typos in predicates) fail fast.
- `end` step results are canonicalized against `plan.output`. When a plan returns a value, the kernel appends a `PlanResult` journal record capturing `{plan_name, plan_id, output_schema, value_cbor}` and caches recent results for operators/CLI tooling.

**Operational impact**: governance reviewers and on-call engineers can now rely on the journal alone to answer “what did this plan produce?” without replaying expressions. Shadow runs also surface the same canonical outputs, making approval diffs clearer. If your workflow depends on downstream automation, use the recorded `PlanResult` entries instead of parsing reducer events.

### Compensations (Saga Pattern)

Three approaches:

#### A. Plan-Based Compensation (Conditional Branches)

Use edge guards to route failures to compensation steps.

```json
{
  "steps": [
    {"id": "charge", "op": "emit_effect", "kind": "payment.charge", ...},
    {"id": "wait_charge", "op": "await_receipt", ...},
    {"id": "reserve", "op": "emit_effect", "kind": "inventory.reserve", ...},
    {"id": "wait_reserve", "op": "await_receipt", ...},
    {"id": "refund", "op": "emit_effect", "kind": "payment.refund", "params": {"txn_id": {"ref": "@var:charge_rcpt.txn_id"}}, ...},
    {"id": "done_ok", "op": "end", "result": {"text": "success"}},
    {"id": "done_fail", "op": "end", "result": {"text": "compensated"}}
  ],
  "edges": [
    {"from": "charge", "to": "wait_charge"},
    {"from": "wait_charge", "to": "reserve", "when": {"ref": "@var:charge_rcpt.ok"}},
    {"from": "wait_charge", "to": "done_fail", "when": {"op": "not", "args": [{"ref": "@var:charge_rcpt.ok"}]}},
    {"from": "reserve", "to": "wait_reserve"},
    {"from": "wait_reserve", "to": "done_ok", "when": {"ref": "@var:reserve_rcpt.ok"}},
    {"from": "wait_reserve", "to": "refund", "when": {"op": "not", "args": [{"ref": "@var:reserve_rcpt.ok"}]}},
    {"from": "refund", "to": "done_fail"}
  ]
}
```

**Use when**: Compensation logic is simple (no business rules)

#### B. Reducer-Based Compensation (Intent Emission)

Reducer detects failure and emits compensation intent.

```rust
(Pc::AwaitingReservation, Event::ReserveResult { success: false, .. }) => {
    s.pc = Pc::Compensating;
    domain_events.push(DomainEvent {
        schema: "RefundRequested@1".into(),
        value: cbor!({"txn_id": s.payment_txn.unwrap()})
    });
}
```

**Use when**: Compensation requires business logic (e.g., partial refunds, customer tier logic)

#### C. Hybrid (Plan Guards + Reducer Tracking)

Plan handles compensation flow, reducer tracks compensating state.

```json
// In plan
{"from": "wait_reserve", "to": "notify_compensation", "when": "failure"},
{"id": "notify_compensation", "op": "raise_event", "event": {}},
{"from": "notify_compensation", "to": "refund"}
```

```rust
// In reducer
(Pc::AwaitingReservation, Event::CompensationStarted {}) => {
    s.pc = Pc::Compensating;
    // Could emit compensating notifications, update metrics, etc.
}
```

### Parallel Effects (Fan-Out / Join)

Plans support this naturally via DAG (no edges = parallel).

```json
{
  "steps": [
    {"id": "fetch_feed1", "op": "emit_effect", "kind": "http.request", "params": {"url": "feed1"}, ...},
    {"id": "fetch_feed2", "op": "emit_effect", "kind": "http.request", "params": {"url": "feed2"}, ...},
    {"id": "fetch_feed3", "op": "emit_effect", "kind": "http.request", "params": {"url": "feed3"}, ...},
    {"id": "wait1", "op": "await_receipt", "for": {"ref": "@var:feed1_id"}, ...},
    {"id": "wait2", "op": "await_receipt", "for": {"ref": "@var:feed2_id"}, ...},
    {"id": "wait3", "op": "await_receipt", "for": {"ref": "@var:feed3_id"}, ...},
    {"id": "merge", "op": "assign", "expr": {"op": "concat", "args": [...]}, ...},
    {"id": "done", "op": "end"}
  ],
  "edges": [
    // No edges between fetch_feed1/2/3 → parallel execution
    {"from": "fetch_feed1", "to": "wait1"},
    {"from": "fetch_feed2", "to": "wait2"},
    {"from": "fetch_feed3", "to": "wait3"},
    // Join: merge depends on all waits
    {"from": "wait1", "to": "merge"},
    {"from": "wait2", "to": "merge"},
    {"from": "wait3", "to": "merge"},
    {"from": "merge", "to": "done"}
  ]
}
```

Deterministic scheduler executes all ready steps (fetch_feed1/2/3 have no predecessors, so all fire in first tick).

### Timeouts and Deadlines

Use `timer.set` to implement timeouts.

#### From Reducer (Micro-Effect)

```rust
(Pc::AwaitingApproval, Event::ApprovalRequested { deadline_ns, request_id }) => {
    s.approval_deadline = deadline_ns;
    effects.push(EffectIntent {
        kind: "timer.set".into(),
        params: cbor!({"deliver_at_ns": deadline_ns, "key": request_id}),
        cap_slot: Some("timer")
    });
    s.pc = Pc::AwaitingApprovalOrTimeout;
}

(Pc::AwaitingApprovalOrTimeout, Event::TimerFired { key }) => {
    // Timeout! Cancel or compensate
    s.pc = Pc::TimedOut;
    domain_events.push(DomainEvent {
        schema: "ApprovalTimedOut@1".into(),
        value: cbor!({"request_id": key})
    });
}

(Pc::AwaitingApprovalOrTimeout, Event::ApprovalGranted { .. }) => {
    s.pc = Pc::Approved;
    // Could emit "cancel timer" intent, or just ignore duplicate timer receipt
}
```

#### From Plan (Parallel Timer Branch)

```json
{
  "steps": [
    {"id": "request", "op": "emit_effect", "kind": "approval.request", ...},
    {"id": "start_timer", "op": "emit_effect", "kind": "timer.set", "params": {"deliver_at_ns": "..."}, ...},
    {"id": "wait_approval", "op": "await_event", "event": "ApprovalGranted@1", ...},
    {"id": "wait_timer", "op": "await_receipt", "for": {"ref": "@var:timer_id"}, ...},
    {"id": "success", "op": "end", "result": {"text": "approved"}},
    {"id": "timeout", "op": "end", "result": {"text": "timed_out"}}
  ],
  "edges": [
    {"from": "request", "to": "start_timer"},
    {"from": "start_timer", "to": "wait_approval"},
    {"from": "start_timer", "to": "wait_timer"},
    // Whichever completes first "wins"
    {"from": "wait_approval", "to": "success"},
    {"from": "wait_timer", "to": "timeout"}
  ]
}
```

Note: v1 plans don't have explicit "await_first" or cancellation, so both branches remain pending. Implement cancellation in v1.1 or handle idempotently (ignore timer receipt after approval).

### Retries and Backoff

Three approaches:

#### A. Adapter-Level Retries (Transparent)

Adapters handle retries transparently using idempotency keys. Not visible to reducers/plans.

**Use when**: Network transients, no business logic needed

#### B. Reducer-Driven Retries

Reducer tracks retry count and re-emits intent with backoff.

```rust
#[derive(Serialize, Deserialize)]
pub struct State {
    pub pc: Pc,
    pub retry_count: u32,
    pub max_retries: u32,
    pub backoff_ns: u64,
}

(Pc::AwaitingCharge, Event::ChargeResult { success: false, retriable: true }) => {
    if s.retry_count < s.max_retries {
        s.retry_count += 1;
        let delay = s.backoff_ns * (2_u64.pow(s.retry_count)); // exponential backoff
        effects.push(EffectIntent {
            kind: "timer.set".into(),
            params: cbor!({"deliver_at_ns": now() + delay, "key": s.order_id}),
            cap_slot: Some("timer")
        });
        s.pc = Pc::BackingOff;
    } else {
        s.pc = Pc::Failed;
        s.last_error = Some("Max retries exceeded".into());
    }
}

(Pc::BackingOff, Event::TimerFired { .. }) => {
    s.pc = Pc::AwaitingCharge;
    domain_events.push(DomainEvent {
        schema: "ChargeRequested@1".into(),
        value: cbor!({"order_id": s.order_id, "amount": s.amount, "attempt": s.retry_count})
    });
}
```

**Use when**: Need business logic for retry decisions, exponential backoff, max attempts

#### C. Plan-Based Retries (v1 Limited)

Plans can't loop, so retries require explicit unrolled steps or external retry orchestrator.

Unrolled example (ugly, not recommended):
```json
{
  "steps": [
    {"id": "try1", "op": "emit_effect", ...},
    {"id": "wait1", "op": "await_receipt", ...},
    {"id": "try2", "op": "emit_effect", ...},
    {"id": "wait2", "op": "await_receipt", ...},
    {"id": "try3", "op": "emit_effect", ...},
    {"id": "wait3", "op": "await_receipt", ...},
    {"id": "success", "op": "end"},
    {"id": "fail", "op": "end"}
  ],
  "edges": [
    {"from": "try1", "to": "wait1"},
    {"from": "wait1", "to": "success", "when": "ok"},
    {"from": "wait1", "to": "try2", "when": "error"},
    {"from": "try2", "to": "wait2"},
    {"from": "wait2", "to": "success", "when": "ok"},
    {"from": "wait2", "to": "try3", "when": "error"},
    {"from": "try3", "to": "wait3"},
    {"from": "wait3", "to": "success", "when": "ok"},
    {"from": "wait3", "to": "fail", "when": "error"}
  ]
}
```

**Recommendation**: Use reducer-driven retries for v1; consider adding plan-level retry primitives in v1.1.

### Human Approvals

v1 policy gate can return `RequireApproval`, but execution path is incomplete. For full implementation:

```
Plan: emit_effect llm.generate (expensive)
  ↓
Policy: decide() → RequireApproval
  ↓
Kernel: writes ApprovalRequired to journal, suspends plan
  ↓
External system: human/AI reviews, grants approval (or denies)
  ↓
Kernel: writes ApprovalGranted, resumes plan
  ↓
Plan continues with effect
```

**v1 workaround**: Model approval as an effect + await:

```json
{
  "steps": [
    {"id": "request_approval", "op": "emit_effect", "kind": "approval.request", "params": {"reason": "expensive LLM call"}, ...},
    {"id": "wait_approval", "op": "await_receipt", ...},
    {"id": "proceed", "op": "emit_effect", "kind": "llm.generate", ...}
  ],
  "edges": [
    {"from": "request_approval", "to": "wait_approval"},
    {"from": "wait_approval", "to": "proceed", "when": {"ref": "@var:approval_rcpt.approved"}}
  ]
}
```

Approval adapter queues task for humans; receipt arrives when approved/denied.

### Long-Running Workflows (Days/Weeks)

Use reducers + timer.set for long-running state.

```rust
(Pc::Idle, Event::CampaignScheduled { start_ns, end_ns, actions }) => {
    s.campaign_start = start_ns;
    s.campaign_end = end_ns;
    s.actions = actions;
    effects.push(EffectIntent {
        kind: "timer.set".into(),
        params: cbor!({"deliver_at_ns": start_ns}),
        cap_slot: Some("timer")
    });
    s.pc = Pc::AwaitingStart;
}

(Pc::AwaitingStart, Event::TimerFired { .. }) => {
    s.pc = Pc::Running;
    // Execute first action, schedule next timer, etc.
}
```

Reducers persist state; kernel ensures deterministic replay.

## Adding New Effect Kinds (when extending the catalog)

Effect kinds and capability types are open strings. To introduce a new kind:
- Create or register an adapter that knows how to execute it and return a signed receipt, and map the kind to a capability type.
- Add canonical param/receipt schemas (for first-class/built-in kinds, place them in `spec/defs/builtin-schemas.air.json` and refresh `spec/schemas/builtin.catalog.schema.json` for strict tooling). Adapter-scoped kinds can ship their own schemas alongside the adapter.
- Define a `defcap` for the capability type that enforces host/model/etc. constraints; bind grants in manifests and list the new kind in `allowed_effects` where used.
- Update policy to allow/deny the new kind explicitly; default-deny will block unknown names until policy and capability wiring exist.
- Add tests or shadow scenarios that exercise the new kind to ensure params/receipts canonicalize and replay correctly.

## Decision Matrix

| Scenario | Recommended Pattern | Why |
|----------|---------------------|-----|
| Simple deterministic flow (payment → email) | Single-plan | Preflight visibility, centralized governance |
| Service boundaries (different teams) | Multi-plan choreography | Independent evolution, loose coupling |
| Complex business rules mid-flow | Reducer-driven | Business logic in testable reducers |
| Need both audit trail and flexibility | Hybrid | Best of both: plan audit + reducer logic |
| Compensations with business rules | Reducer-driven | Reducer decides compensation strategy |
| Compensations without business rules | Plan conditional branches | Simpler, all in plan DAG |
| Parallel independent effects | Single-plan with DAG | Natural parallelism in plan scheduler |
| Human-in-the-loop | Single-plan + approval effect (v1 workaround) | Choke point at emit_effect |
| Long-running (days/weeks) | Reducer-driven + timer.set | Reducer persists state across long pauses |
| Retries with backoff | Reducer-driven | Exponential backoff logic in reducer |
| High-volume, low-value workflows | Multi-plan choreography | Decentralized, scalable |
| High-value, governance-heavy workflows | Single-plan or Hybrid | Centralized approval/audit |

## Anti-Patterns

### 1. Orchestrating Network Effects in Reducers

**Bad**:
```rust
// DON'T DO THIS
effects.push(EffectIntent { kind: "http.request".into(), ... });
effects.push(EffectIntent { kind: "llm.generate".into(), ... });
```

**Why**: Violates architectural boundary; bypasses governance; can't shadow-run; policy can't gate properly.

**Fix**: Emit DomainIntent, let plan handle effects.

### 2. Business Logic in Plans

**Bad**:
```json
{
  "id": "decide",
  "op": "assign",
  "expr": {
    "op": "if",
    "args": [
      {"op": "and", "args": [
        {"op": "gt", "args": [{"ref": "@var:amount"}, 1000]},
        {"op": "eq", "args": [{"ref": "@var:user_tier"}, "premium"]}
      ]},
      {"text": "discount_10"},
      {"text": "no_discount"}
    ]
  }
}
```

**Why**: Business rules belong in reducers (versioned, testable, replay). Plans should orchestrate, not decide.

**Fix**: Let reducer emit intent with business decision already made.

### 3. Missing Correlation Keys

**Bad**:
```rust
// Reducer emits intent without correlation id
domain_events.push(DomainEvent {
    schema: "ChargeRequested@1".into(),
    value: cbor!({"amount": 100})  // Missing order_id!
});
```

**Why**: Plan raises result event, but which reducer instance should receive it? Kernel can't route without key.

**Fix**: Always include correlation key (order_id, user_id, request_id) in intents and configure `correlate_by` in triggers.

### 4. Unbounded Retries in Reducers

**Bad**:
```rust
(Pc::Failed, Event::ChargeResult { success: false, .. }) => {
    // Retry forever!
    domain_events.push(DomainEvent { schema: "ChargeRequested@1".into(), ... });
}
```

**Why**: Infinite loops burn resources, prevent terminal states.

**Fix**: Track retry count, set max attempts, use exponential backoff, move to terminal failure state.

### 5. Cross-Reducer Calls

**Bad**:
```rust
// Trying to "call" another reducer
effects.push(EffectIntent { kind: "reducer.invoke".into(), ... }); // No such effect!
```

**Why**: Reducers don't invoke each other; events are the only communication mechanism.

**Fix**: Emit domain event; kernel routes to other reducer via manifest.routing.

### 6. Ignoring Idempotency

**Bad**:
```rust
(Pc::AwaitingPayment, Event::ChargeResult { success, txn_id }) => {
    // No fence! If this event is replayed, we'll emit duplicate ReserveRequested
    s.pc = Pc::AwaitingReservation;
    domain_events.push(DomainEvent { schema: "ReserveRequested@1".into(), ... });
}
```

**Why**: Events may be replayed (e.g., during recovery). Duplicate intents = duplicate charges.

**Fix**: Use fences:
```rust
(Pc::AwaitingPayment, Event::ChargeResult { success, txn_id }) => {
    if !s.fences.payment_done {
        s.fences.payment_done = true;
        s.payment_txn = Some(txn_id);
        s.pc = Pc::AwaitingReservation;
        domain_events.push(...);
    }
}
```

### 7. Plan Timeouts Without Cancellation

**Bad**:
```json
// Start timer and wait for approval in parallel, but both branches complete
{"from": "start_timer", "to": "wait_approval"},
{"from": "start_timer", "to": "wait_timer"},
{"from": "wait_approval", "to": "success"},
{"from": "wait_timer", "to": "timeout"}
```

**Why**: If approval arrives first, plan goes to "success", but timer still fires later. No cancellation in v1.

**Fix**: Handle idempotently (ignore timer receipt if already approved), or wait for v1.1 cancellation primitive.

## Future Enhancements (v1.1+)

- **Plan-level retries**: `emit_effect_with_retry(max_attempts, backoff_policy)`
- **Cancellation**: Explicit `cancel_effect` step to stop outstanding effects/timers
- **Approval gates**: First-class `require_approval` step that suspends and journals approval requests
- **Sub-plans**: `invoke_plan` step to call reusable sub-graphs
- **Dynamic parallelism**: `for_each` step to fan-out over lists
- **Await-first**: `await_any` to proceed when any of N effects completes
- **Versioned transitions**: Explicit handling of in-flight workflows during plan upgrades

## Conclusion

AIR v1 provides four coordination primitives that support a wide range of workflow patterns. Choose patterns based on:
- **Governance needs**: Single-plan for tight control
- **Business logic complexity**: Reducer-driven for complex rules
- **Service boundaries**: Multi-plan for loose coupling
- **Auditability + flexibility**: Hybrid for both

Start simple (single-plan), migrate to reducer-driven as business logic grows. Avoid anti-patterns (network effects in reducers, business logic in plans, missing correlation keys). Use fences and idempotency keys everywhere.

The architecture deliberately keeps plans non-Turing complete to maintain preflight analyzability and governance. For complex orchestration, use reducers; for simple effect choreography, use plans.
