# P1: Make required_caps and allowed_effects Derived

**Priority**: P1 (strongly recommended for v1)
**Effort**: Medium
**Risk if deferred**: Medium - manual sync burden, drift bugs, agent authoring friction

## Summary

`defplan` currently requires authors to manually maintain two lists:
- `required_caps: [CapGrantName]` - capability grants the plan needs
- `allowed_effects: [EffectKind]` - effect kinds the plan may emit

These are fully derivable from the plan's `emit_effect` steps. Making them derived eliminates a class of synchronization bugs and simplifies plan authoring.

## Rationale

### Current problem

Every `emit_effect` step declares:
```json
{
  "op": "emit_effect",
  "kind": "http.request",    // <- EffectKind
  "cap": "http_out_google",  // <- CapGrantName
  ...
}
```

Authors must then duplicate this information:
```json
{
  "required_caps": ["http_out_google", "llm_basic"],
  "allowed_effects": ["http.request", "llm.generate"]
}
```

If someone adds a step but forgets to update the lists, they get a runtime error instead of a clear validation message. If someone removes a step but leaves the lists, the plan claims capabilities it doesn't use.

### Benefits of derivation

1. **Eliminates sync bugs**: Impossible for lists to drift from actual steps
2. **Simpler authoring**: Agents and humans don't maintain redundant data
3. **Better tooling**: `air fmt` can always produce correct, canonical plans
4. **Clearer validation**: Error messages point to the step, not the list

## Proposed Design

### Semantics

The validator computes:
- `computed_caps = union of (emit_effect.cap for all emit_effect steps)`
- `computed_effects = union of (emit_effect.kind for all emit_effect steps)`

### Authoring behavior

1. **If author omits the fields**: Validator/loader fills them with computed values before canonicalization
2. **If author supplies the fields**: They must exactly match computed values, or validation fails with a clear error

### Canonical form

The canonical CBOR always includes the computed, sorted lists. This ensures:
- Hashes are stable regardless of authoring style
- Introspection tools see the full picture
- Shadow runs can enumerate required capabilities

### Example

Author writes (sugar):
```json
{
  "$kind": "defplan",
  "name": "com.acme/example@1",
  "input": "com.acme/Input@1",
  "steps": [
    {"id": "fetch", "op": "emit_effect", "kind": "http.request", "cap": "http_cap", ...},
    {"id": "summarize", "op": "emit_effect", "kind": "llm.generate", "cap": "llm_cap", ...}
  ],
  "edges": [...]
  // Note: required_caps and allowed_effects omitted
}
```

Canonical form (after validation):
```json
{
  "$kind": "defplan",
  "name": "com.acme/example@1",
  "input": "com.acme/Input@1",
  "steps": [...],
  "edges": [...],
  "required_caps": ["http_cap", "llm_cap"],
  "allowed_effects": ["http.request", "llm.generate"]
}
```

## Implementation Plan

### Step 1: Update validation in `aos-air-types`

In `validate.rs` (or equivalent), add a function:

```rust
fn derive_plan_caps_and_effects(plan: &DefPlan) -> (Vec<CapGrantName>, Vec<EffectKind>) {
    let mut caps = BTreeSet::new();
    let mut effects = BTreeSet::new();

    for step in &plan.steps {
        if let PlanStepKind::EmitEffect(emit) = &step.kind {
            caps.insert(emit.cap.clone());
            effects.insert(emit.kind.clone());
        }
    }

    (caps.into_iter().collect(), effects.into_iter().collect())
}
```

### Step 2: Update plan validation

```rust
fn validate_plan(plan: &mut DefPlan) -> Result<(), ValidationError> {
    let (derived_caps, derived_effects) = derive_plan_caps_and_effects(plan);

    if plan.required_caps.is_empty() {
        plan.required_caps = derived_caps;
    } else if plan.required_caps != derived_caps {
        return Err(ValidationError::CapsEffectsMismatch {
            field: "required_caps",
            declared: plan.required_caps.clone(),
            derived: derived_caps,
        });
    }

    // Same for allowed_effects
    ...
}
```

### Step 3: Update spec prose

In `spec/03-air.md` §11 (defplan), update:

> **`required_caps`** and **`allowed_effects`** are **derived fields**. The validator computes them from `emit_effect` steps:
> - `required_caps` = sorted set of all `emit_effect.cap` values
> - `allowed_effects` = sorted set of all `emit_effect.kind` values
>
> Authors may omit these fields (recommended) or supply them explicitly. If supplied, they must exactly match the derived values.

### Step 4: Update JSON Schema

Make both fields optional:

```json
"required_caps": {
  "type": "array",
  "items": { "$ref": "common.schema.json#/$defs/CapGrantName" },
  "description": "Derived from emit_effect steps. May be omitted in authoring."
},
"allowed_effects": {
  "type": "array",
  "items": { "$ref": "common.schema.json#/$defs/EffectKind" },
  "description": "Derived from emit_effect steps. May be omitted in authoring."
}
```

### Step 5: Update examples

Remove explicit `required_caps`/`allowed_effects` from example plans to demonstrate the cleaner authoring style.

### Step 6: Add tests

- Test that omitted fields are filled correctly
- Test that mismatched fields produce clear validation errors
- Test that canonical hashes are stable regardless of authoring style

## Acceptance Criteria

- [ ] Validator derives `required_caps` from `emit_effect.cap` fields
- [ ] Validator derives `allowed_effects` from `emit_effect.kind` fields
- [ ] Omitting the fields results in correct derivation
- [ ] Supplying wrong values produces a clear validation error
- [ ] Spec updated to document derived field behavior
- [ ] Examples updated to use cleaner authoring style
- [ ] All existing tests pass
