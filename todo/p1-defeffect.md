# P1: Introduce defeffect as a First-Class Defkind

**Priority**: P1 (recommended for v1)
**Effort**: Medium-Large
**Risk if deferred**: Medium - effect catalog remains partially in code, partially in data

## Summary

Effect kinds (http.request, blob.put, llm.generate, etc.) are currently "known" through a combination of:
1. String constants in code
2. Param/receipt schemas in `builtin-schemas.air.json`
3. Prose documentation in the spec

There's no single AIR node that declares "this effect kind exists, here are its schemas, here's what capability type guards it."

Introducing `defeffect` makes the effect catalog fully data-driven and introspectable, enabling:
- Agents to discover what effects are available
- Tooling to validate effect params against the correct schema
- Future extensibility for custom effect kinds
- Clean separation between "what effects exist" and "how adapters implement them"

## Rationale

### Current state: Effects are half in code, half in data

Today, to understand the `http.request` effect, you need to look at:

1. **Code**: `EffectKind::HTTP_REQUEST` constant
2. **Schema**: `sys/HttpRequestParams@1`, `sys/HttpRequestReceipt@1` in `builtin-schemas.air.json`
3. **Capability**: `http.out` cap type (documented, not formally linked)
4. **Prose**: Spec §7 describes the effect

This scattering means:
- Adding a new effect kind requires changes in multiple places
- Agents can't introspect "what effects does this world support?"
- Validation logic must hard-code which schema to use for which effect kind

### Benefits of `defeffect`

1. **Single source of truth**: One AIR node defines the effect completely
2. **Introspectable**: Query the manifest to see all available effects
3. **Extensible**: Custom adapters can register custom effects via AIR
4. **Type-safe**: Validator knows exactly which schema to use for params/receipts
5. **Future-ready**: Enables effect composition, versioning, and discovery

### Future possibilities (v1.1+)

With `defeffect` as a foundation, future versions could add:
- **Effect versioning**: `http.request@2` with different param schema
- **Effect composition**: Macro effects that expand to multiple primitives
- **Effect discovery**: Agents query "what can this world do?"
- **Adapter registration**: Formal binding of effect kinds to adapter implementations
- **Effect policies**: Fine-grained policies per effect kind (rate limits, approvals)

## Proposed Design

### New defkind: `defeffect`

```json
{
  "$kind": "defeffect",
  "name": "sys/http.request@1",
  "kind": "http.request",
  "params_schema": "sys/HttpRequestParams@1",
  "receipt_schema": "sys/HttpRequestReceipt@1",
  "cap_type": "http.out",
  "description": "Performs an HTTP request to an external URL"
}
```

Fields:
- **name**: Standard versioned name for this effect definition
- **kind**: The `EffectKind` string used in plans (e.g., `"http.request"`)
- **params_schema**: Reference to the defschema for effect parameters
- **receipt_schema**: Reference to the defschema for effect receipts
- **cap_type**: The capability type that guards this effect
- **description**: Optional human-readable description

### Updated manifest

```json
{
  "schemas": [...],
  "modules": [...],
  "plans": [...],
  "caps": [...],
  "policies": [...],
  "secrets": [...],
  "effects": [
    {"name": "sys/http.request@1", "hash": "sha256:..."},
    {"name": "sys/blob.put@1", "hash": "sha256:..."},
    {"name": "sys/llm.generate@1", "hash": "sha256:..."}
  ]
}
```

### Built-in effects bundle

Ship a `spec/defs/builtin-effects.air.json` that defines all v1 effects:

```json
[
  {
    "$kind": "defeffect",
    "name": "sys/http.request@1",
    "kind": "http.request",
    "params_schema": "sys/HttpRequestParams@1",
    "receipt_schema": "sys/HttpRequestReceipt@1",
    "cap_type": "http.out"
  },
  {
    "$kind": "defeffect",
    "name": "sys/blob.put@1",
    "kind": "blob.put",
    "params_schema": "sys/BlobPutParams@1",
    "receipt_schema": "sys/BlobPutReceipt@1",
    "cap_type": "blob"
  },
  {
    "$kind": "defeffect",
    "name": "sys/blob.get@1",
    "kind": "blob.get",
    "params_schema": "sys/BlobGetParams@1",
    "receipt_schema": "sys/BlobGetReceipt@1",
    "cap_type": "blob"
  },
  {
    "$kind": "defeffect",
    "name": "sys/timer.set@1",
    "kind": "timer.set",
    "params_schema": "sys/TimerSetParams@1",
    "receipt_schema": "sys/TimerSetReceipt@1",
    "cap_type": "timer"
  },
  {
    "$kind": "defeffect",
    "name": "sys/llm.generate@1",
    "kind": "llm.generate",
    "params_schema": "sys/LlmGenerateParams@1",
    "receipt_schema": "sys/LlmGenerateReceipt@1",
    "cap_type": "llm.basic"
  },
  {
    "$kind": "defeffect",
    "name": "sys/vault.put@1",
    "kind": "vault.put",
    "params_schema": "sys/VaultPutParams@1",
    "receipt_schema": "sys/VaultPutReceipt@1",
    "cap_type": "secret"
  },
  {
    "$kind": "defeffect",
    "name": "sys/vault.rotate@1",
    "kind": "vault.rotate",
    "params_schema": "sys/VaultRotateParams@1",
    "receipt_schema": "sys/VaultRotateReceipt@1",
    "cap_type": "secret"
  }
]
```

### Bootstrap consideration

**Question**: How do you call effects before the first manifest with `defeffect` is applied?

**Answer**: The kernel ships with built-in effect definitions compiled in. These are automatically available even with an empty manifest. The `defeffect` nodes in `builtin-effects.air.json` are the canonical representation, but the kernel "knows" them at startup.

This is similar to how programming languages have built-in types that are also representable in the type system.

### Validation changes

When validating a plan's `emit_effect` step:

```rust
fn validate_emit_effect(step: &PlanStepEmitEffect, manifest: &Manifest) -> Result<()> {
    // Look up the defeffect by kind
    let effect_def = manifest.lookup_effect_by_kind(&step.kind)
        .ok_or(ValidationError::UnknownEffectKind(step.kind.clone()))?;

    // Validate params against the effect's param schema
    validate_value_against_schema(&step.params, &effect_def.params_schema)?;

    // Verify the capability grant's type matches
    let cap_grant = manifest.lookup_cap_grant(&step.cap)?;
    let cap_def = manifest.lookup_cap(&cap_grant.cap)?;
    if cap_def.cap_type != effect_def.cap_type {
        return Err(ValidationError::CapTypeMismatch {
            effect_kind: step.kind.clone(),
            expected: effect_def.cap_type.clone(),
            got: cap_def.cap_type.clone(),
        });
    }

    Ok(())
}
```

## Implementation Plan

### Step 1: Add JSON Schema for `defeffect`

Create `spec/schemas/defeffect.schema.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v1/defeffect.schema.json",
  "title": "AIR v1 defeffect",
  "description": "Effect kind definition. Declares an effect's param/receipt schemas and required capability type.",
  "type": "object",
  "properties": {
    "$kind": { "const": "defeffect" },
    "name": { "$ref": "common.schema.json#/$defs/Name" },
    "kind": {
      "$ref": "common.schema.json#/$defs/EffectKind",
      "description": "The effect kind string used in emit_effect steps"
    },
    "params_schema": {
      "$ref": "common.schema.json#/$defs/SchemaRef",
      "description": "Schema for effect parameters"
    },
    "receipt_schema": {
      "$ref": "common.schema.json#/$defs/SchemaRef",
      "description": "Schema for effect receipts"
    },
    "cap_type": {
      "$ref": "common.schema.json#/$defs/CapType",
      "description": "Capability type that guards this effect"
    },
    "description": {
      "type": "string",
      "description": "Optional human-readable description"
    }
  },
  "required": ["$kind", "name", "kind", "params_schema", "receipt_schema", "cap_type"],
  "additionalProperties": false
}
```

### Step 2: Update manifest schema

```diff
// manifest.schema.json
  "properties": {
    ...
+   "effects": {
+     "type": "array",
+     "items": { "$ref": "#/$defs/NamedRef" },
+     "description": "Effect kind definitions"
+   }
  },
- "required": ["$kind","schemas","modules","plans","caps","policies"],
+ "required": ["$kind","schemas","modules","plans","caps","policies","effects"],
```

### Step 3: Create built-in effects bundle

Create `spec/defs/builtin-effects.air.json` with all v1 effect definitions.

### Step 4: Add Rust types

```rust
// aos-air-types/src/model.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefEffect {
    pub name: Name,
    pub kind: EffectKind,
    pub params_schema: SchemaRef,
    pub receipt_schema: SchemaRef,
    pub cap_type: CapType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// Update AirNode enum
pub enum AirNode {
    Defschema(DefSchema),
    Defmodule(DefModule),
    Defplan(DefPlan),
    Defcap(DefCap),
    Defpolicy(DefPolicy),
    Defsecret(DefSecret),
    Defeffect(DefEffect),  // NEW
    Manifest(Manifest),
}
```

### Step 5: Update manifest loader

- Load `effects` entries by hash
- Build an effect catalog indexed by `kind`
- Merge built-in effects with manifest-declared effects

### Step 6: Update validation

- Plan validator looks up effect definitions by kind
- Validates params against `params_schema`
- Verifies capability type matches

### Step 7: Update effect manager

- Receipt validation uses `receipt_schema` from defeffect
- Error messages reference the effect definition

### Step 8: Update spec prose

- Add `defeffect` to the defkind list in `spec/03-air.md`
- Move effect catalog documentation to reference `defeffect` nodes
- Update `spec/05-workflows.md` "adding new effect kinds" section

### Step 9: Update examples

Ensure all examples include the built-in effects in their manifests (or document that built-ins are implicit).

## Acceptance Criteria

- [ ] `defeffect.schema.json` added with correct shape
- [ ] `manifest.schema.json` includes `effects` array
- [ ] `builtin-effects.air.json` defines all v1 effects
- [ ] Rust `DefEffect` type implemented
- [ ] `AirNode` enum includes `Defeffect` variant
- [ ] Manifest loader handles effect definitions
- [ ] Plan validator uses defeffect for param schema lookup
- [ ] Effect manager uses defeffect for receipt schema lookup
- [ ] Spec prose updated
- [ ] All examples work with new model
- [ ] All tests pass
- [ ] Hash tests updated for new schemas
