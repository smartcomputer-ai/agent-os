# AIR Plan Extensions (v1.1+)

**Status**: Deferred. Not included in v1.0.

This document describes planned extensions to AIR plans for v1.1 and beyond. These additions are **not implemented in v1.0** and should be considered **forward-looking design notes** to guide future development. v1.0 ships with a minimal plan feature set (emit_effect, await_receipt, raise_event, await_event, assign, end) that covers essential orchestration needs through documented patterns.

**v1 Integration Note**: This spec has been updated to align with AIR v1 changes (see `spec/patch.md` and `spec/03-air.md`), particularly:
- The `ExprOrValue` union introduced in v1 is adopted here for `spawn_plan.input` and `spawn_for_each.inputs`
- References to authoring sugar vs. canonical JSON lenses follow v1 terminology
- JSON schema snippets use `ExprOrValue` where appropriate
- All canonicalization rules remain consistent with v1's CBOR encoding and schema-bound hashing

## Rationale for deferring to v1.1

Plans in v1.0 are intentionally narrow, focused on orchestrating external effects under governance. The extensions below—structured concurrency, sub-plans, fan-out/fan-in—are valuable but not required for a useful first release. Deferring allows:

- Validation of real-world pain points before building a mini workflow engine
- Smaller kernel surface area and faster time-to-market for v1.0
- Clear patterns using existing primitives (triggers, correlation keys, aggregator reducers)
- Forward compatibility: reserved fields and schema names allow adding these features without breaking existing worlds

When demand for composition and parallel orchestration is validated, these extensions can be introduced incrementally.

---

## Overview of v1.1 Extensions

The v1.1 extensions add **structured concurrency** to plans:

1. **PlanHandle**: First-class typed reference to running plan instances
2. **spawn_plan / await_plan**: Parent-child composition with typed results
3. **spawn_for_each / await_plans_all**: Fan-out over lists and barrier joins
4. **Invariant timing clarification**: When invariants run and how failures propagate
5. **Patterns** (no new ops): Deadlines, races, approvals using existing effects

These keep plans deterministic (all non-determinism still crosses the effect boundary via receipts), preserve replay, and enable modular, auditable orchestration without forcing reducers to carry control logic.

---

## 1. PlanHandle (typed references to plan instances)

### Concept

A **PlanHandle** is an opaque, typed reference to a running plan instance. Only the kernel creates handles; plans pass them as values in expressions.

### Built-in schema (reserved)

```
sys/PlanHandle@1 = record {
  instance: uuid,
  plan: Name
}
```

### Journal linkage

Extend **PlanStarted** entry to include:

```
PlanStarted {
  plan_name: Name,
  instance_id: uuid,
  input_hash: hash,
  parent_instance_id?: uuid  // NEW: links child to parent
}
```

### Manifest pinning

Child instances inherit the parent's pinned manifest hash at spawn time. This guarantees child semantics match the parent's view during shadow-run and replay.

### Validation

- Handles are opaque; the validator ensures `await_plan` and `await_plans_all` refer to handles produced by earlier steps in the same plan.
- Prevents dangling references and ensures type safety.

---

## 2. spawn_plan step (start a child plan)

### Purpose

Deterministically start a child plan instance from within a parent plan. Pure control-plane operation; no external effect.

### Shape

```json
{
  "id": "spawn_child",
  "op": "spawn_plan",
  "plan": "com.acme/charge_plan@1",
  "input": { "ref": "@plan.input.order" },
  "bind": { "handle_as": "child_handle" }
}
```

**Note**: The `input` field accepts `ExprOrValue` (as of v1), so you may provide a plain value in authoring sugar or canonical JSON form, or a full `Expr` tree (`ExprRef`, `ExprOp`, etc.) when dynamic computation is needed.

### Semantics

1. Validate that `plan` exists in manifest
2. Evaluate `input` expression (or literal value)
3. Type-check evaluated input against child plan's declared `input` schema
4. Create child instance with:
   - New UUID
   - Pinned to parent's manifest hash
   - Parent linkage via `parent_instance_id`
5. Append `PlanStarted { plan_name, instance_id, input_hash, parent_instance_id }`
6. Bind `handle_as` variable to `sys/PlanHandle@1 { instance, plan }`
7. Mark step as `Done`

### Determinism

- No side-effects beyond journal entries
- Replay reconstructs identical instance IDs and ordering from journal
- Shadow-run records predicted child spawns but does not execute child

---

## 3. await_plan step (wait for child completion)

### Purpose

Wait for a specific child instance to end and capture its typed result.

### Shape

```json
{
  "id": "wait_child",
  "op": "await_plan",
  "for": { "ref": "@step:spawn_child.handle" },
  "bind": { "as": "child_result" }
}
```

### Result type (inferred)

The bound variable is a **variant**:

```
Variant [
  Ok: ChildOutputType,           // if child plan succeeded
  Error: sys/PlanError@1,        // if child plan failed
  Canceled: unit                 // reserved for v1.2+ cancellation
]
```

Where:
- `ChildOutputType` is the child plan's declared `output` schema
- If child has no output schema, `Ok` carries `unit`
- `sys/PlanError@1 = record { code: text, message: text, details?: value }`

### Semantics

1. Step becomes **ready** when the referenced child's `PlanEnded` entry is appended to the journal
2. Bind the variant based on `PlanEnded.status`:
   - `status: "ok"` → `Variant::Ok(result)` where result is type-checked against child's output schema
   - `status: "error"` → `Variant::Error(...)` with structured error
   - `status: "canceled"` → `Variant::Canceled` (reserved)
3. Mark step as `Done`

### Validation

- `for` must reference a `spawn_plan` step in the same DAG
- Validator infers `ChildOutputType` from the spawned plan's output schema
- Downstream expressions can pattern-match on variant tags

---

## 4. spawn_for_each step (fan-out over list)

### Purpose

Spawn the same child plan for each element in a list. Deterministic fan-out.

### Shape

```json
{
  "id": "spawn_all",
  "op": "spawn_for_each",
  "plan": "com.acme/summarize_doc@1",
  "inputs": { "ref": "@plan.input.documents" },
  "max_fanout": 100,
  "bind": { "handles_as": "doc_handles" }
}
```

**Note**: The `inputs` field accepts `ExprOrValue`, so you may provide a plain list value or a dynamic expression.

### Semantics

1. Evaluate `inputs` expression (or literal list) → must be a `list<ChildInputType>`
2. Type-check each element against child plan's input schema
3. Check `max_fanout` constraint if present; fail if `len(inputs) > max_fanout`
4. For each input (in order):
   - Create child instance
   - Append `PlanStarted` with parent linkage
5. Bind `handles_as` to `list<sys/PlanHandle@1>` (same order as inputs)
6. Mark step as `Done`

### Constraints and guardrails

- **Optional `max_fanout` field**: Static ceiling to prevent runaway fan-out
  - Validator enforces if provided
  - Prevents accidental "fork bomb" scenarios
- **Atomicity**: Either all instances start (journal records all) or step fails before starting any
- **Order preservation**: Handle list matches input list order

### Performance note

Implementations may chunk internal creation, but journal ordering and determinism must be preserved.

---

## 5. await_plans_all step (barrier for all children)

### Purpose

Wait for all referenced child plans to complete. Collect their results in order.

### Shape

```json
{
  "id": "barrier",
  "op": "await_plans_all",
  "handles": { "ref": "@var:doc_handles" },
  "bind": { "results_as": "doc_results" }
}
```

### Result type (inferred)

```
list< Variant [
  Ok: ChildOutputType,
  Error: sys/PlanError@1,
  Canceled: unit
] >
```

Order matches the input handles list.

### Semantics

1. Step becomes **ready** when every handle in the list has a corresponding `PlanEnded` entry
2. Collect results in the same order as handles
3. Bind `results_as` to the result list
4. Mark step as `Done`

### Validation

- `handles` must be a list produced by `spawn_for_each` (or homogeneous `spawn_plan` outputs)
- **Homogeneity constraint** (v1.1): All handles must reference the same child plan
  - Simplifies typing: single `ChildOutputType` inferred
  - Mixed-plan barriers deferred to v1.2+

### Downstream processing

Use expressions to filter/map results (note: `assign.expr` accepts `ExprOrValue`, so you can provide a full expression tree as shown, or a plain value when static):

```json
{
  "id": "extract_successes",
  "op": "assign",
  "expr": {
    "op": "filter",
    "args": [
      { "ref": "@var:doc_results" },
      { "op": "eq", "args": [
        { "op": "get", "args": [{ "ref": "@current" }, { "text": "$tag" }] },
        { "text": "Ok" }
      ]}
    ]
  },
  "bind": { "as": "successes" }
}
```

---

## 6. Invariants: timing and failure semantics

### Current gap (v1.0)

Invariant evaluation timing is underspecified. When do they run? How do violations terminate plans?

### v1.1 clarification

**When invariants run:**
- After every step completes (at tick boundary)
- At plan end, before emitting `PlanEnded`

**Failure semantics:**
- Invariants are boolean `Expr` evaluated against current plan environment
- On the **first failing invariant**:
  1. Kernel immediately ends plan instance
  2. Appends `PlanEnded { status: "error", result_ref: <PlanError with code="invariant_violation"> }`
  3. No further steps execute
  4. Any pending awaits ignored during replay (PlanEnded fences the instance)

**Validation:**
- Invariants must be total (all refs must resolve) and side-effect-free
- Validator checks that invariant expressions are boolean-typed

### Example

```json
{
  "$kind": "defplan",
  "name": "com.acme/payment_plan@1",
  "invariants": [
    {
      "op": "le",
      "args": [
        { "op": "get", "args": [{ "ref": "@var:total_spent" }, { "text": "cents" }] },
        { "nat": 100000 }
      ]
    }
  ]
}
```

If `total_spent.cents > 100000` after any step, plan terminates with invariant violation.

---

## 7. Updated defplan schema (delta for v1.1)

### New step types

Add to `Step` union in `spec/schemas/defplan.schema.json`:

```json
{
  "$defs": {
    "StepSpawnPlan": {
      "allOf": [
        { "$ref": "#/$defs/StepBase" },
        {
          "properties": {
            "op": { "const": "spawn_plan" },
            "plan": { "$ref": "common.schema.json#/$defs/Name" },
            "input": { "$ref": "common.schema.json#/$defs/ExprOrValue" },
            "bind": {
              "type": "object",
              "properties": {
                "handle_as": { "$ref": "common.schema.json#/$defs/VarName" }
              },
              "required": ["handle_as"]
            }
          },
          "required": ["op", "plan", "input", "bind"]
        }
      ]
    },
    "StepAwaitPlan": {
      "allOf": [
        { "$ref": "#/$defs/StepBase" },
        {
          "properties": {
            "op": { "const": "await_plan" },
            "for": { "$ref": "common.schema.json#/$defs/Expr" },
            "bind": { "$ref": "#/$defs/BindAs" }
          },
          "required": ["op", "for", "bind"]
        }
      ]
    },
    "StepSpawnForEach": {
      "allOf": [
        { "$ref": "#/$defs/StepBase" },
        {
          "properties": {
            "op": { "const": "spawn_for_each" },
            "plan": { "$ref": "common.schema.json#/$defs/Name" },
            "inputs": { "$ref": "common.schema.json#/$defs/ExprOrValue" },
            "max_fanout": { "type": "integer", "minimum": 1 },
            "bind": {
              "type": "object",
              "properties": {
                "handles_as": { "$ref": "common.schema.json#/$defs/VarName" }
              },
              "required": ["handles_as"]
            }
          },
          "required": ["op", "plan", "inputs", "bind"]
        }
      ]
    },
    "StepAwaitPlansAll": {
      "allOf": [
        { "$ref": "#/$defs/StepBase" },
        {
          "properties": {
            "op": { "const": "await_plans_all" },
            "handles": { "$ref": "common.schema.json#/$defs/Expr" },
            "bind": { "$ref": "#/$defs/BindAs" }
          },
          "required": ["op", "handles", "bind"]
        }
      ]
    }
  }
}
```

### Validation rules (semantic)

- **await_plan**: `for` must reference a prior `spawn_plan` step; infer `ChildOutputType` from spawned plan's output schema
- **spawn_for_each**: `inputs` must type-check to `list<ChildInputType>` (accepts `ExprOrValue`, so may be a plain list or expression); enforce `max_fanout` if present
- **await_plans_all**: `handles` must be a list from `spawn_for_each` or homogeneous `spawn_plan`; enforce homogeneity for typing
- **Invariants**: Must be boolean expressions; total (no missing refs); side-effect-free

**Note on `ExprOrValue`**: As of v1, `spawn_plan.input` and `spawn_for_each.inputs` accept `ExprOrValue`, allowing authors to provide plain values (in authoring sugar or canonical JSON) when the schema is statically known, or full expressions when dynamic computation is needed. Guards (`for`, `handles`, `edges[].when`) remain full `Expr` because they are predicates or references requiring expression semantics.

---

## 8. Journal entries (delta for v1.1)

### PlanStarted

Add optional field:

```
PlanStarted {
  plan_name: Name,
  instance_id: uuid,
  input_hash: hash,
  parent_instance_id?: uuid    // NEW
}
```

### No new entry types required

- `await_plan` and `await_plans_all` consume existing `PlanEnded` entries
- Step completions recorded in existing journal flow

---

## 9. Determinism and replay

### Key properties

- All new steps are **control-plane only**; no non-deterministic inputs
- Replay uses recorded `PlanStarted` / `PlanEnded` to drive `await_*` readiness
- Shadow-run records predicted spawns but does not execute children
- Invariant evaluations are deterministic (same env → same result)

### Replay behavior

1. Replay reconstructs parent-child graph from `PlanStarted` entries
2. `spawn_*` steps deterministically recreate instance IDs and handles
3. `await_*` steps become ready when corresponding `PlanEnded` appears in journal
4. Invariant violations terminate at the same step during replay

---

## 10. Patterns (no new ops required)

These patterns work with v1.0 primitives + v1.1 structured concurrency.

### Pattern A: Deadlines (race child vs. timer)

**Goal**: Timeout a child plan if it takes too long.

**Steps**:

1. `spawn_child`: spawn_plan(...)
2. `set_timer`: emit_effect(kind: "timer.set", params: {after_ns: 30000000000}, ...)
3. `await_child`: await_plan(for: child_handle, bind: "child_res")
4. `await_timer`: await_receipt(for: timer_effect_id, bind: "timer_rcpt")
5. `decide`: assign expression that sets `decision` var to "child" or "timeout" based on which completed first
6. Branch on `decision`:
   - Edge to `handle_success` when `decision == "child"` and `child_res` is Ok
   - Edge to `handle_timeout` when `decision == "timeout"`

**Determinism**: Receipt ordering in the journal determines which completes first. The scheduler's step-id ordering only applies when multiple steps are ready within the same tick.

### Pattern B: Any-of over heterogeneous waits

**Goal**: Wait for the first of multiple conditions (e.g., approval OR timeout OR cancellation event).

**Steps**:

1. Start multiple parallel awaits (each in separate step):
   - `await_approval`: await_receipt(...)
   - `await_timeout`: await_receipt(...)
   - `await_cancel`: await_event(...)
2. Each await flows to an `assign` step that sets a write-once decision flag
3. Downstream steps guard on the decision flag
4. Later-completing awaits flow to no-op sink or compensation paths

**Note**: Noisier than a hypothetical `await_any`, but works deterministically without kernel changes.

### Pattern C: Human approvals

**Goal**: Require human sign-off before proceeding.

**Effect type**: Standardize an `approval.request` effect kind:

```
approval.request {
  params: {
    subject: text,
    requester: text,
    context_ref?: hash
  }
}

receipt: {
  decision: "approved" | "denied",
  approver: text,
  rationale?: text
}
```

**Steps**:

1. `request_approval`: emit_effect(kind: "approval.request", ...)
2. `await_approval`: await_receipt(for: approval_effect_id, bind: "approval_rcpt")
3. Guard downstream on `approval_rcpt.decision == "approved"`

**Note**: This is explicit and auditable. Policy can still gate risky effects upstream with `require_approval` in v1.2+, but the explicit request provides better UX and audit trails.

---

## 11. Examples

### Example A: Single child composition

**Scenario**: Parent spawns a charge plan, waits for result, raises typed event to reducer.

**Note**: The examples below use authoring sugar for readability (plain JSON values). You may also use canonical JSON (tagged) or full `Expr` trees where `ExprOrValue` is accepted.

```json
{
  "$kind": "defplan",
  "name": "com.acme/order_flow@1",
  "input": "com.acme/OrderCreated@1",
  "steps": [
    {
      "id": "spawn_charge",
      "op": "spawn_plan",
      "plan": "com.acme/charge_plan@1",
      "input": { "ref": "@plan.input" },
      "bind": { "handle_as": "charge_handle" }
    },
    {
      "id": "await_charge",
      "op": "await_plan",
      "for": { "ref": "@step:spawn_charge.charge_handle" },
      "bind": { "as": "charge_result" }
    },
    {
      "id": "on_success",
      "op": "raise_event",
      "reducer": "com.acme/OrderSM@1",
      "event": {
        "record": {
          "$schema": { "text": "com.acme/PaymentSuccess@1" },
          "order_id": { "ref": "@plan.input.order_id" },
          "txn_id": { "ref": "@var:charge_result.Ok.txn_id" }
        }
      }
    },
    {
      "id": "on_error",
      "op": "raise_event",
      "reducer": "com.acme/OrderSM@1",
      "event": {
        "record": {
          "$schema": { "text": "com.acme/PaymentFailed@1" },
          "order_id": { "ref": "@plan.input.order_id" },
          "error": { "ref": "@var:charge_result.Error.message" }
        }
      }
    },
    { "id": "done", "op": "end" }
  ],
  "edges": [
    { "from": "spawn_charge", "to": "await_charge" },
    {
      "from": "await_charge",
      "to": "on_success",
      "when": {
        "op": "eq",
        "args": [
          { "op": "get", "args": [{ "ref": "@var:charge_result" }, { "text": "$tag" }] },
          { "text": "Ok" }
        ]
      }
    },
    {
      "from": "await_charge",
      "to": "on_error",
      "when": {
        "op": "eq",
        "args": [
          { "op": "get", "args": [{ "ref": "@var:charge_result" }, { "text": "$tag" }] },
          { "text": "Error" }
        ]
      }
    },
    { "from": "on_success", "to": "done" },
    { "from": "on_error", "to": "done" }
  ]
}
```

### Example B: Fan-out and barrier

**Scenario**: Receive list of documents, spawn summarize plan for each, wait for all, publish aggregate result.

```json
{
  "$kind": "defplan",
  "name": "com.acme/batch_summarize@1",
  "input": "com.acme/SummarizeRequest@1",
  "steps": [
    {
      "id": "spawn_all",
      "op": "spawn_for_each",
      "plan": "com.acme/summarize_doc@1",
      "inputs": { "ref": "@plan.input.documents" },
      "max_fanout": 50,
      "bind": { "handles_as": "doc_handles" }
    },
    {
      "id": "barrier",
      "op": "await_plans_all",
      "handles": { "ref": "@var:doc_handles" },
      "bind": { "results_as": "doc_results" }
    },
    {
      "id": "filter_successes",
      "op": "assign",
      "expr": {
        "comment": "Filter to only Ok results",
        "op": "filter",
        "args": [
          { "ref": "@var:doc_results" },
          { "op": "eq", "args": [
            { "op": "get", "args": [{ "ref": "@current" }, { "text": "$tag" }] },
            { "text": "Ok" }
          ]}
        ]
      },
      "bind": { "as": "successes" }
    },
    {
      "id": "build_report",
      "op": "assign",
      "expr": {
        "record": {
          "total": { "op": "len", "args": [{ "ref": "@var:doc_results" }] },
          "successful": { "op": "len", "args": [{ "ref": "@var:successes" }] },
          "summaries": { "ref": "@var:successes" }
        }
      },
      "bind": { "as": "report" }
    },
    {
      "id": "publish",
      "op": "raise_event",
      "reducer": "com.acme/ReportSM@1",
      "event": {
        "record": {
          "$schema": { "text": "com.acme/BatchSummaryComplete@1" },
          "report": { "ref": "@var:report" }
        }
      }
    },
    { "id": "done", "op": "end" }
  ],
  "edges": [
    { "from": "spawn_all", "to": "barrier" },
    { "from": "barrier", "to": "filter_successes" },
    { "from": "filter_successes", "to": "build_report" },
    { "from": "build_report", "to": "publish" },
    { "from": "publish", "to": "done" }
  ]
}
```

### Example C: Deadline pattern

**Scenario**: Spawn child, race against timer, first to complete decides.

```json
{
  "steps": [
    {
      "id": "spawn_child",
      "op": "spawn_plan",
      "plan": "com.acme/slow_work@1",
      "input": { "ref": "@plan.input" },
      "bind": { "handle_as": "child_h" }
    },
    {
      "id": "set_timer",
      "op": "emit_effect",
      "kind": "timer.set",
      "params": {
        "record": {
          "after_ns": { "nat": 30000000000 }
        }
      },
      "cap": "sys_timer",
      "bind": { "effect_id_as": "timer_id" }
    },
    {
      "id": "await_child",
      "op": "await_plan",
      "for": { "ref": "@var:child_h" },
      "bind": { "as": "child_res" }
    },
    {
      "id": "await_timer",
      "op": "await_receipt",
      "for": { "ref": "@var:timer_id" },
      "bind": { "as": "timer_rcpt" }
    },
    {
      "id": "decide_child",
      "op": "assign",
      "expr": { "text": "child" },
      "bind": { "as": "decision" }
    },
    {
      "id": "decide_timeout",
      "op": "assign",
      "expr": { "text": "timeout" },
      "bind": { "as": "decision" }
    },
    {
      "id": "handle_success",
      "op": "raise_event",
      "reducer": "com.acme/WorkSM@1",
      "event": { "comment": "child succeeded before timeout" }
    },
    {
      "id": "handle_timeout",
      "op": "raise_event",
      "reducer": "com.acme/WorkSM@1",
      "event": { "comment": "work timed out" }
    },
    { "id": "done", "op": "end" }
  ],
  "edges": [
    { "from": "spawn_child", "to": "await_child" },
    { "from": "set_timer", "to": "await_timer" },
    { "from": "await_child", "to": "decide_child", "when": { "op": "not", "args": [{ "op": "has", "args": [{ "ref": "@env" }, { "text": "decision" }] }] } },
    { "from": "await_timer", "to": "decide_timeout", "when": { "op": "not", "args": [{ "op": "has", "args": [{ "ref": "@env" }, { "text": "decision" }] }] } },
    { "from": "decide_child", "to": "handle_success" },
    { "from": "decide_timeout", "to": "handle_timeout" },
    { "from": "handle_success", "to": "done" },
    { "from": "handle_timeout", "to": "done" }
  ]
}
```

**Note**: Only one decision path executes; the other await completes later and flows to a guarded edge that is never taken.

---

## 12. Operational and governance considerations

### Capabilities

- Children do not inherit ad-hoc privileges
- Each child's `emit_effect` evaluated against current cap ledger
- Keep `required_caps` explicit on child plans

### Budgets and rate limits

- Fan-out creates bursty effect emission
- Rely on policy rpm/daily budgets to throttle
- Use `max_fanout` on `spawn_for_each` as a static brake

### Observability

- Parent-child links enrich the why-graph
- Show spawned counts, completion rates, and result distributions in plan inspectors
- Trace costs and timings hierarchically

---

## 13. Backward compatibility and migration

### v1.0 → v1.1 upgrade path

- Existing v1.0 plans are unchanged and continue to work
- New ops (`spawn_plan`, `await_plan`, `spawn_for_each`, `await_plans_all`) live alongside existing steps
- Validator and journal changes are additive
- Forward-compat fields (`parent_instance_id`, `sys/PlanHandle`) reserved in v1.0 to ease transition

### Migration strategy

1. **Phase 1**: Deploy v1.1 kernel
2. **Phase 2**: Author new plans using structured concurrency
3. **Phase 3**: Optionally refactor existing reducer-based fan-out patterns to use plan barriers

---

## 14. Open questions and deferred features (v1.2+)

These are explicitly **not included in v1.1** but documented for future consideration:

### await_any / k-of-n

- **Use case**: Wait for first N of M children to succeed; cancel the rest
- **Challenge**: Heterogeneous typing (different child plan types), partial result handling
- **Defer rationale**: Adds significant complexity; patterns with separate `await_plan` steps cover most needs

### cancel_plan step

- **Use case**: Terminate a running child plan (timeout, user cancellation)
- **Challenge**: Cancellation semantics, compensating transactions, receipt handling for in-flight effects
- **Defer rationale**: Requires new error surfaces and coordinator state; deadline patterns suffice for v1.1

### call_pure in plans

- **Use case**: Lightweight transforms (parse, filter, map) without spinning up a full reducer
- **Challenge**: Introduces compute in plans (currently orchestration-only); ABI design, determinism guarantees
- **Defer rationale**: Keep plans narrow; push transforms to reducers or effect adapters

### Native deadline fields on awaits

- **Use case**: Built-in timeout on `await_plan` without explicit timer steps
- **Challenge**: Requires implicit timer plumbing or ambient time abstraction
- **Defer rationale**: Patterns with explicit timers are more auditable and transparent

### Per-plan concurrency limits

- **Use case**: Throttle how many instances of a plan can run simultaneously (avoid thundering herds)
- **Challenge**: Global coordinator state, admission control, queuing semantics
- **Defer rationale**: `max_fanout` and policy rate limits cover most cases; defer until demand validated

---

## 15. Implementation effort estimate

Rough sizing for a solo developer:

| Component | Effort | Notes |
|-----------|--------|-------|
| PlanHandle type + journal linkage | S | New schema, journal field, validation |
| spawn_plan + await_plan | M | Kernel plan engine changes, step types, typing inference |
| spawn_for_each + await_plans_all | M | List handling, barrier readiness, homogeneity checks |
| Invariant timing/failure | S | Engine hook, error struct, journal entry |
| Documentation + examples | M | Comprehensive guide, migration notes, patterns |
| Testing (replay, barriers, races) | L | Golden tests, fuzz variants, edge cases |

**Total: ~3-4 weeks** for a careful implementation with strong tests.

---

## 16. Final conclusions

### Ship in v1.1 when demand validated

- **PlanHandles** with parent-child journal linkage
- **spawn_plan** and **await_plan** for single-child composition
- **spawn_for_each** and **await_plans_all** for fan-out/fan-in
- **Invariant timing clarification** and structured failure semantics

### Document now, defer building

- Deadline/race patterns using `timer.set` + guards
- Approval patterns using `approval.request` effect
- This spec as forward-looking design guidance

### Defer to v1.2+

- `await_any` / k-of-n with heterogeneous types
- `cancel_plan` with compensation semantics
- `call_pure` for lightweight in-plan transforms
- Native deadline fields on awaits
- Per-plan concurrency limits

This roadmap keeps v1.0 lean and shippable, validates real-world needs with patterns, and provides a clear, safe path to structured concurrency when orchestration complexity demands it.
