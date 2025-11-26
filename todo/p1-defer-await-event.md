# P1: Defer await_event to v1.1

**Priority**: P1 (recommended for v1)
**Effort**: Small-Medium
**Risk if deferred**: Low (we're deferring, not implementing)

## Summary

The `await_event` plan step allows plans to wait for domain events. However:
1. No examples use it
2. Key semantics are underspecified
3. The core pattern (reducer→trigger→plan→raise_event→reducer) is sufficient for v1

We should defer `await_event` to v1.1 where it can be properly specified and tested.

## Rationale

### Underspecified semantics

The current spec leaves critical questions unanswered:

1. **Historical vs future events?**
   - Does it only see events appended *after* the step becomes active?
   - Or can it match events already in the journal?

2. **Multiple matches?**
   - If multiple events match the `where` predicate, which one wins?
   - First by journal height? Last? Any?

3. **Racing waiters?**
   - Can multiple `await_event` steps in different plans match the same event?
   - Is it "first come first served" or broadcast?

4. **Correlation and keyed reducers?**
   - How does this interact with `correlate_by` in triggers?
   - Does the plan instance have a "scope" that filters events?

### No examples use it

```bash
grep -r "await_event" examples/
# No results
```

The feature is completely untested in practice.

### The core pattern is sufficient

For v1, the canonical coordination pattern is:

```
Reducer emits DomainIntent
    ↓
Trigger starts Plan
    ↓
Plan orchestrates effects (emit_effect/await_receipt)
    ↓
Plan raises result event (raise_event)
    ↓
Reducer advances state
```

This covers:
- Effect orchestration
- Saga patterns
- Compensation flows
- Long-running workflows (via timer.set)

`await_event` adds plan-to-plan coordination, which is a more advanced pattern that can wait for v1.1.

### What v1.1 should specify

When we add `await_event` in v1.1, we should specify:

1. **Only future events**: Events must be appended after step activation
2. **First match wins**: Earliest matching event by journal height
3. **Broadcast semantics**: Multiple waiters can match the same event
4. **Explicit correlation**: Use `where` predicate; no implicit scoping

## Proposed Change

### Option A: Remove from schema (recommended)

Remove `await_event` from the v1 plan step types:

```diff
// defplan.schema.json
"Step": {
  "type": "object",
  "oneOf": [
    { "$ref": "#/$defs/StepRaiseEvent" },
    { "$ref": "#/$defs/StepEmitEffect" },
    { "$ref": "#/$defs/StepAwaitReceipt" },
-   { "$ref": "#/$defs/StepAwaitEvent" },
    { "$ref": "#/$defs/StepAssign" },
    { "$ref": "#/$defs/StepEnd" }
  ]
}
```

### Option B: Keep but mark as reserved

Keep the step type but reject it during validation:

```rust
PlanStepKind::AwaitEvent(_) => {
    return Err(ValidationError::ReservedFeature {
        feature: "await_event",
        message: "await_event is reserved for v1.1"
    });
}
```

### Recommendation: Option A

Clean removal is better. If no examples use it, there's no migration burden.

## Implementation Plan

### Step 1: Remove from JSON Schema

```diff
// spec/schemas/defplan.schema.json

// Remove from Step oneOf
- { "$ref": "#/$defs/StepAwaitEvent" },

// Remove the definition
- "StepAwaitEvent": {
-   "allOf": [
-     { "$ref": "#/$defs/StepBase" },
-     {
-       "type": "object",
-       "properties": {
-         "id": { "$ref": "common.schema.json#/$defs/StepId" },
-         "op": { "const": "await_event" },
-         "event": { "$ref": "common.schema.json#/$defs/SchemaRef" },
-         "where": { "$ref": "common.schema.json#/$defs/Expr" },
-         "bind": { "$ref": "#/$defs/BindAs" }
-       },
-       "required": ["op","event","bind"],
-       "additionalProperties": false
-     }
-   ]
- },
```

### Step 2: Remove from Rust types

```diff
// aos-air-types/src/model.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanStepKind {
    RaiseEvent(PlanStepRaiseEvent),
    EmitEffect(PlanStepEmitEffect),
    AwaitReceipt(PlanStepAwaitReceipt),
-   AwaitEvent(PlanStepAwaitEvent),
    Assign(PlanStepAssign),
    End(PlanStepEnd),
}

- #[derive(Debug, Clone, Serialize, Deserialize)]
- pub struct PlanStepAwaitEvent {
-     pub event: SchemaRef,
-     #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
-     pub where_clause: Option<Expr>,
-     pub bind: PlanBind,
- }
```

### Step 3: Remove from plan executor

Remove any handling of `await_event` in the kernel's plan executor.

### Step 4: Update spec prose

In `spec/03-air.md` §11, remove `await_event` from the Steps section and add a note:

> **Note**: `await_event` (plan waiting for domain events) is deferred to v1.1. The v1 coordination pattern uses triggers to start plans and `raise_event` to notify reducers.

In `spec/05-workflows.md`, update any examples that mention `await_event` to use the reducer-driven pattern instead.

### Step 5: Update tests

Remove any tests that use `await_event`.

## Acceptance Criteria

- [ ] `await_event` step type removed from JSON schema
- [ ] `PlanStepAwaitEvent` removed from Rust types
- [ ] Plan executor doesn't handle `await_event`
- [ ] Spec prose updated to document deferral
- [ ] Workflow patterns doc uses only v1 primitives
- [ ] All tests pass
- [ ] spec/12-plans-v1.1.md documents future `await_event` semantics
