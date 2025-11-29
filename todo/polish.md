# Polish: Small Improvements Before v1 Ship

**Priority**: P2 (nice to have)
**Effort**: Small (each item)
**Risk if deferred**: Low

This document collects small polish items that improve spec clarity, tooling, and developer experience. Each can be done independently.

---

## 1. Use `Name` type consistently in schemas

**Problem**: Some schemas use `{ "text": {} }` where the value is conceptually a Name.

**Example**: `sys/TimerFired@1` has `reducer: { "text": {} }` but the value is always a Name-formatted string like `com.acme/OrderSM@1`.

**Fix**: Either:
- Add a comment noting "text value in Name format"
- Or create a semantic validation rule

**Files**: `spec/defs/builtin-schemas.air.json`

---

## 2. Add explicit "no $schema in payloads" rule

**Problem**: The spec implies that reducers/plans shouldn't embed `$schema` fields in event payloads, but it's not a bold rule.

**Current text** (spec/03-air.md §11):
> "The kernel infers the payload schema from the reducer's manifest entry and validates/canonicalizes the event (and optional key) before emitting, so authors should not embed `$schema` fields inside the payload."

**Fix**: Make it a bold callout:

> **Important**: Never embed `$schema` fields inside event or effect payloads. The kernel determines schemas from manifest routing and capability bindings. Self-describing payloads are not supported and will be rejected.

**Files**: `spec/03-air.md`, `spec/04-reducers.md`

---

## 3. Document plan edge uniqueness constraint

**Problem**: The `defplan.edges` array has no uniqueness constraint. Two identical edges `{from: A, to: B}` could appear.

**Fix**: Add to spec and validator:
> Edges must be unique by `(from, to)` pair. Duplicate edges are a validation error.

**Files**: `spec/03-air.md`, `spec/schemas/defplan.schema.json` (add note), validation code

---

## 4. Clarify routing.inboxes purpose

**Problem**: `manifest.routing.inboxes` is underspecified. What is `source`?

**Current schema**:
```json
"inboxes": [{
  "source": { "type": "string" },
  "reducer": { "$ref": "common.schema.json#/$defs/Name" }
}]
```

**Fix**: Add documentation explaining:
- What `source` represents (external inbox name? reducer name?)
- When inboxes are used vs events
- Example use case

**Files**: `spec/03-air.md`, `spec/schemas/manifest.schema.json`

---

## 5. Add air_version to manifest

**Problem**: No way to detect manifest version without parsing the whole thing.

**Fix**: Add optional `air_version` field:

```json
{
  "$kind": "manifest",
  "air_version": "1.0",
  ...
}
```

Validation: If present, must be "1.0" (or whatever we ship). Future versions can require it.

**Files**: `spec/schemas/manifest.schema.json`, `spec/03-air.md`

---

## 6. Enforce "one effect per reducer step" in validator

**Problem**: The spec says reducers may emit at most one effect per step, but this might not be validated.

**Fix**: Add explicit validation:

```rust
if output.effects.len() > 1 {
    return Err(ReducerError::TooManyEffects {
        count: output.effects.len(),
        message: "Reducers may emit at most one effect per step. Lift complex orchestration to a plan."
    });
}
```

**Files**: `crates/aos-kernel/src/reducer.rs`

---

## 7. Document pure modules as reserved

**Problem**: `defmodule.module_kind` is an enum with only `"reducer"`. The spec mentions pure modules are deferred to v1.1, but doesn't say the enum is extensible.

**Fix**: Add note to spec:
> `module_kind` is currently limited to `"reducer"`. Future versions will add `"pure"` for stateless computation modules. Existing manifests will remain valid.

**Files**: `spec/03-air.md` §6

---

## 8. Add test vectors for third-party implementations

**Problem**: No public test vectors for verifying CBOR canonicalization and hashing.

**Fix**: Create `spec/test-vectors/` with:
- `canonical-cbor.json`: Input JSON → expected CBOR hex → expected hash
- `schemas.json`: Sample defschema values and their hashes
- `plans.json`: Sample defplan values and their hashes

**Files**: New directory `spec/test-vectors/`

---

## 9. Add "retry with backoff" example

**Problem**: The reducer-driven retry pattern (using `timer.set` and fences) is documented in spec/04-reducers.md but not demonstrated in examples.

**Fix**: Add example `08-retry-backoff/` showing:
- Reducer emits intent
- Plan returns error
- Reducer schedules retry with exponential backoff
- Reducer tracks attempt count and gives up after max

**Files**: New example `examples/08-retry-backoff/`

---

## 10. Clarify Value vs Expr in ExprOrValue disambiguation

**Problem**: The spec explains the dual JSON lenses but doesn't clearly state the parsing order for `ExprOrValue`.

**Fix**: Add explicit paragraph:

> In `ExprOrValue` positions, the loader first attempts to parse the JSON as an `Expr` (checking for `op`, `ref`, `record`, etc. keys). If that fails, it treats the JSON as a plain `Value` and interprets it using the surrounding schema context.

**Files**: `spec/03-air.md` §3

---

## Checklist

- [ ] 1. Add Name format comments to builtin schemas
- [ ] 2. Add bold "no $schema in payloads" rule
- [x] 3. Document edge uniqueness constraint
- [x] 4. Clarify routing.inboxes purpose
- [x] 5. Add air_version to manifest
- [x] 6. Enforce one effect per reducer step
- [ ] 7. Document pure modules as reserved
- [ ] 8. Add test vectors
- [ ] 9. Add retry-backoff example
- [ ] 10. Clarify ExprOrValue parsing order
