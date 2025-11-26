# P0: Clarify Null/Option in Expr vs Value Contexts

**Priority**: P0 (must fix before v1 ships)
**Effort**: Small
**Risk if deferred**: High - agents produce invalid AIR, tooling confusion

## Summary

The spec prose mentions `{ "const": { "null": {} } }` as the way to produce a null value in expressions, but the JSON Schema for `ExprConst` defines `{"null": {}}` directly (no `const` wrapper). This inconsistency must be resolved.

## Rationale

When authoring AIR (especially by agents), the exact shape of null/none values is critical:
- Wrong shape → validation failure
- Ambiguous spec → agents guess differently
- Runtime surprises when "null" means different things

## Current State

### Spec prose (`spec/03-air.md` §3.2):

> **Nulls in Expr vs Value**: When an `ExprOrValue` slot is authored as a literal Value, `null` still denotes `none` for `option<T>`. When authored as an expression, use `{ "const": { "null": {} } }` to produce a `null`/`none` value; raw JSON `null` is only valid on the literal path and is not parsed as an `ExprConst`.

### JSON Schema (`common.schema.json`):

```json
"ExprConst": {
  "type": "object",
  "oneOf": [
    { "properties": { "null": { "type": "object", "additionalProperties": false } }, "required": ["null"], "additionalProperties": false },
    { "properties": { "bool": { "type": "boolean" } }, "required": ["bool"], "additionalProperties": false },
    // ... etc
  ]
}
```

The schema defines `{"null": {}}` directly—there is no `"const"` wrapper.

### The confusion

| Context | Spec says | Schema says |
|---------|-----------|-------------|
| Expr null | `{"const": {"null": {}}}` | `{"null": {}}` |

The `"const"` wrapper in the prose doesn't exist in the schema.

## Recommendation

**Keep the schema as-is. Fix the prose.**

The schema is correct: `ExprConst` variants are `{"null": {}}`, `{"nat": 42}`, `{"text": "foo"}`, etc. There is no `const` wrapper.

### Updated spec language

Replace the confusing paragraph with:

> **Nulls in Expr vs Value contexts:**
>
> - In **Value** positions (interpreted via a schema), raw JSON `null` represents `none` for `option<T>` types.
> - In **Expr** positions, use `{"null": {}}` to construct a null/none value. Raw JSON `null` is **not** valid in expression contexts.
>
> Example:
> ```json
> // In a Value position (schema-directed):
> { "field": null }  // field is option<T>, this is none
>
> // In an Expr position (explicit AST):
> { "op": "eq", "args": [{"ref": "@var:x"}, {"null": {}}] }
> ```

## Implementation Plan

### Step 1: Update `spec/03-air.md` §3.2

Remove all references to `{"const": {...}}` wrapper. The tagged form is just `{"null": {}}`, `{"nat": 42}`, etc.

### Step 2: Search for `"const"` in examples

```bash
grep -r '"const"' spec/ examples/
```

If any examples use the wrong form, fix them.

### Step 3: Verify Rust deserialization

The `ExprConst` enum in `aos-air-types/src/model.rs` should match:

```rust
pub enum ExprConst {
    Null { null: EmptyObject },
    Bool { bool: bool },
    // ...
}
```

This is already correct. No code changes needed.

### Step 4: Add a test case

Add a test that deserializes `{"null": {}}` as `ExprConst::Null` and rejects `{"const": {"null": {}}}`.

## Acceptance Criteria

- [ ] Spec prose updated to show `{"null": {}}` (not `{"const": {"null": {}}}`)
- [ ] No examples use the wrong form
- [ ] Test confirms correct parsing of null expressions
- [ ] CLAUDE.md or a FAQ note explains the Value vs Expr distinction if needed
