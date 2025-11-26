# P1: Defer Plan Invariants to v1.1

**Priority**: P1 (recommended for v1)
**Effort**: Small
**Risk if deferred**: Low (we're deferring, not implementing)

## Summary

`defplan` currently includes an `invariants?: [Expr]` field, but:
1. No examples use it
2. The evaluation semantics are underspecified
3. Failure behavior is unclear

Rather than ship half-baked semantics, we should defer invariants to v1.1 where they can be properly designed and tested.

## Rationale

### Current problems

The spec mentions invariants but leaves key questions unanswered:

1. **When are they checked?**
   - After each step?
   - Only at plan end?
   - Both?

2. **What happens on violation?**
   - Plan aborts immediately?
   - Plan continues but logs a warning?
   - Error status propagated to parent?

3. **What can they reference?**
   - Spec says "may only reference declared locals/steps/input (no `@event`)"
   - But the validator doesn't enforce this yet

### Why defer?

- **No examples use invariants** - feature is untested in practice
- **Complexity budget** - v1 should be minimal and solid
- **Forward compatibility** - easier to add well-designed invariants later than fix broken ones
- **spec/12-plans-v1.1.md already designs this** - we have a good plan for v1.1

### What v1.1 would specify (from spec/12):

> **When invariants run:**
> - After every step completes (at tick boundary)
> - At plan end, before emitting `PlanEnded`
>
> **Failure semantics:**
> - On the **first failing invariant**:
>   1. Kernel immediately ends plan instance
>   2. Appends `PlanEnded { status: "error", result_ref: <PlanError with code="invariant_violation"> }`
>   3. No further steps execute

This is the right design, but it needs implementation and testing.

## Proposed Change

### Option A: Remove from schema (recommended)

Remove `invariants` from the v1 schema entirely:

```diff
// defplan.schema.json
- "invariants": {
-   "type": "array",
-   "items": { "$ref": "common.schema.json#/$defs/Expr" }
- },
```

**Pros**: Clean v1 surface, no confusion
**Cons**: Breaking change if anyone somehow used it

### Option B: Keep in schema but ignore

Keep the field but document it as reserved:

```json
"invariants": {
  "type": "array",
  "items": { "$ref": "common.schema.json#/$defs/Expr" },
  "description": "RESERVED for v1.1. Ignored in v1.0."
}
```

**Pros**: Forward compatible, existing plans with invariants won't fail to parse
**Cons**: Confusing - field exists but does nothing

### Recommendation: Option A

Since no examples use invariants, there's no migration burden. A clean removal is better than a confusing "ignored" field.

## Implementation Plan

### Step 1: Remove from JSON Schema

```diff
// spec/schemas/defplan.schema.json
  "properties": {
    ...
-   "invariants": {
-     "type": "array",
-     "items": { "$ref": "common.schema.json#/$defs/Expr" }
-   }
  },
```

### Step 2: Remove from Rust types

```diff
// aos-air-types/src/model.rs
pub struct DefPlan {
    pub name: Name,
    pub input: SchemaRef,
    pub output: Option<SchemaRef>,
    pub locals: IndexMap<VarName, SchemaRef>,
    pub steps: Vec<PlanStep>,
    pub edges: Vec<PlanEdge>,
    pub required_caps: Vec<CapGrantName>,
    pub allowed_effects: Vec<EffectKind>,
-   pub invariants: Vec<Expr>,
}
```

### Step 3: Update spec prose

In `spec/03-air.md` §11, remove the `invariants` field from the shape and add a note:

> **Note**: Plan invariants are deferred to v1.1. See `spec/12-plans-v1.1.md` for the planned design.

### Step 4: Remove any validation code

If there's any code that attempts to validate invariants, remove it.

### Step 5: Update tests

Remove any tests that reference plan invariants.

## Acceptance Criteria

- [ ] `invariants` field removed from `defplan.schema.json`
- [ ] `invariants` field removed from Rust `DefPlan` struct
- [ ] Spec prose updated to note deferral to v1.1
- [ ] No runtime code references plan invariants
- [ ] All tests pass
- [ ] spec/12-plans-v1.1.md remains as the design doc for future work
