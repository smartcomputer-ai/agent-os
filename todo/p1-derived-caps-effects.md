# P1: Make required_caps and allowed_effects Derived

**Priority**: P1 (strongly recommended for v1)
**Effort**: Medium
**Risk if deferred**: Medium - manual sync burden, drift bugs, agent authoring friction

## Summary (done)

`defplan` now derives `required_caps` and `allowed_effects` from `emit_effect` steps. Authors may omit the fields; if provided, they must match the derived sets. Normalization fills them and canonical CBOR always includes the sorted, deduped lists.

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

## Final Behavior

- Derivation: `required_caps` = union of `emit_effect.cap`; `allowed_effects` = union of `emit_effect.kind`.
- Authoring: fields optional; if supplied, must exactly match derived sets (sorted/deduped) or validation fails.
- Canonicalization: normalization fills missing fields and sorts/dedupes; canonical CBOR always includes the derived lists.

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

## Implementation & Status

- Code: `aos-air-types/src/validate.rs` derives and validates; `plan_literals.rs` normalization fills/sorts/dedupes.
- Spec: `spec/03-air.md` updated with derived semantics; `spec/schemas/defplan.schema.json` marks fields optional + uniqueItems + description.
- Example: `spec/05-workflows.md` fulfillment plan now omits the lists to showcase derivation.
- Tests: validation coverage for mismatch/omission/sorting; normalization test `normalize_fills_derived_caps_and_effects`; schema hash fixture updated.
- Hashes: `aos-cbor` canonical hash for defplan schema refreshed.

## Acceptance Criteria

- [x] Validator derives `required_caps` from `emit_effect.cap` fields
- [x] Validator derives `allowed_effects` from `emit_effect.kind` fields
- [x] Omitting the fields results in correct derivation
- [x] Supplying wrong values produces a clear validation error
- [x] Spec updated to document derived field behavior
- [x] Examples updated to use cleaner authoring style
- [x] All existing tests pass (including new coverage)
