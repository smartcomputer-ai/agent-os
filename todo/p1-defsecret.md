# P1: Introduce defsecret as a First-Class Defkind

**Priority**: P1 (strongly recommended for v1)
**Effort**: Medium
**Risk if deferred**: Medium - manifest model remains inconsistent

## Summary

Secrets are currently the only control-plane entity that doesn't follow the standard `[{name, hash}]` pattern in the manifest. They're inlined with a different structure. Introducing `defsecret` as a proper defkind makes the manifest model consistent and enables standard AIR operations (diffs, patches, shadow runs) on secrets.

## Rationale

### Current state: Secrets are special-cased

The manifest has five "normal" sections that all look alike:

```json
{
  "schemas": [{"name": "...", "hash": "sha256:..."}],
  "modules": [{"name": "...", "hash": "sha256:..."}],
  "plans": [{"name": "...", "hash": "sha256:..."}],
  "caps": [{"name": "...", "hash": "sha256:..."}],
  "policies": [{"name": "...", "hash": "sha256:..."}]
}
```

And then secrets, which are different:

```json
{
  "secrets": [
    {
      "alias": "payments/stripe",
      "version": 1,
      "binding_id": "env:STRIPE_KEY",
      "expected_digest": "sha256:...",
      "policy": {
        "allowed_caps": ["stripe_cap"],
        "allowed_plans": ["com.acme/charge@1"]
      }
    }
  ]
}
```

This breaks the "everything that defines the world is data in AIR" story.

### Problems with the current design

1. **Inconsistent model**: Why are secrets inline when everything else is referenced?
2. **Duplicate policy mechanism**: `secrets[].policy` duplicates what `defpolicy` does
3. **No content addressing**: Secret definitions aren't hashed and stored in CAS
4. **Diff/patch awkwardness**: Patching secrets requires special-case logic
5. **Shadow run gaps**: Harder to predict secret-related changes

### Benefits of `defsecret`

1. **Consistent manifest**: All definitions are `[{name, hash}]`
2. **Content addressed**: Secret metadata stored in CAS like everything else
3. **Standard operations**: Diffs, patches, and shadow runs work uniformly
4. **Clear audit trail**: Secret definition changes are versioned AIR nodes
5. **Simpler validation**: One pattern for all manifest entries

## Proposed Design

### New defkind: `defsecret`

```json
{
  "$kind": "defsecret",
  "name": "payments/stripe@1",
  "binding_id": "env:STRIPE_KEY",
  "expected_digest": "sha256:...",
  "allowed_caps": ["stripe_cap"],
  "allowed_plans": ["com.acme/charge@1"]
}
```

Fields:
- **name**: Standard versioned name (replaces alias+version)
- **binding_id**: Opaque ID resolved to a backend in node-local config
- **expected_digest**: Optional hash of plaintext for drift detection
- **allowed_caps**: Capability grants that may use this secret
- **allowed_plans**: Plans that may use this secret

### Updated manifest

```json
{
  "schemas": [{"name": "...", "hash": "sha256:..."}],
  "modules": [{"name": "...", "hash": "sha256:..."}],
  "plans": [{"name": "...", "hash": "sha256:..."}],
  "caps": [{"name": "...", "hash": "sha256:..."}],
  "policies": [{"name": "...", "hash": "sha256:..."}],
  "secrets": [{"name": "...", "hash": "sha256:..."}]  // Now consistent!
}
```

### Updated SecretRef

SecretRef can now use the standard Name format:

```json
{
  "$kind": "defschema",
  "name": "sys/SecretRef@1",
  "type": {
    "record": {
      "name": { "text": {} }  // e.g., "payments/stripe@1"
    }
  }
}
```

Or keep the alias+version form if you prefer human-readable references:

```json
{
  "record": {
    "alias": { "text": {} },
    "version": { "nat": {} }
  }
}
```

The Name format (`payments/stripe@1`) is cleaner and matches other references.

## Implementation Plan

### Step 1: Add JSON Schema for `defsecret`

Create `spec/schemas/defsecret.schema.json`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v1/defsecret.schema.json",
  "title": "AIR v1 defsecret",
  "type": "object",
  "properties": {
    "$kind": { "const": "defsecret" },
    "name": { "$ref": "common.schema.json#/$defs/Name" },
    "binding_id": {
      "type": "string",
      "description": "Opaque ID mapped to a backend in node-local resolver config"
    },
    "expected_digest": {
      "$ref": "common.schema.json#/$defs/Hash",
      "description": "Optional hash of plaintext for drift detection"
    },
    "allowed_caps": {
      "type": "array",
      "items": { "$ref": "common.schema.json#/$defs/CapGrantName" },
      "description": "Capability grants that may use this secret"
    },
    "allowed_plans": {
      "type": "array",
      "items": { "$ref": "common.schema.json#/$defs/Name" },
      "description": "Plans that may use this secret"
    }
  },
  "required": ["$kind", "name", "binding_id"],
  "additionalProperties": false
}
```

### Step 2: Update manifest schema

```diff
// manifest.schema.json
  "properties": {
    ...
-   "secrets": {
-     "type": "array",
-     "items": { /* inline secret object */ }
-   }
+   "secrets": {
+     "type": "array",
+     "items": { "$ref": "#/$defs/NamedRef" }
+   }
  }
```

### Step 3: Add Rust types

```rust
// aos-air-types/src/model.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefSecret {
    pub name: Name,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_caps: Vec<CapGrantName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_plans: Vec<Name>,
}

// Update AirNode enum
pub enum AirNode {
    Defschema(DefSchema),
    Defmodule(DefModule),
    Defplan(DefPlan),
    Defcap(DefCap),
    Defpolicy(DefPolicy),
    Defsecret(DefSecret),  // NEW
    Manifest(Manifest),
}
```

### Step 4: Update manifest loader

The loader needs to resolve `secrets` entries by hash and load the `defsecret` nodes.

### Step 5: Update secret resolver

The kernel's secret resolver needs to look up secrets by Name instead of by inline definition.

### Step 6: Update SecretRef handling

If changing SecretRef to use Name instead of alias+version:

```diff
// builtin-schemas.air.json
  {
    "$kind": "defschema",
    "name": "sys/SecretRef@1",
    "type": {
      "record": {
-       "alias": { "text": {} },
-       "version": { "nat": {} }
+       "name": { "text": {} }
      }
    }
  }
```

### Step 7: Update spec prose

- Add `defsecret` to the defkind list in `spec/03-air.md`
- Update `spec/17-secrets.md` to describe the new model
- Update examples that use secrets

### Step 8: Migration path

If any existing manifests have inline secrets, provide a migration tool:

```bash
aos migrate-secrets --manifest manifest.air.json
# Extracts inline secrets to defsecret nodes, updates manifest
```

## Acceptance Criteria

- [ ] `defsecret.schema.json` added with correct shape
- [ ] `manifest.schema.json` updated to use `[{name, hash}]` for secrets
- [ ] Rust `DefSecret` type implemented
- [ ] Manifest loader handles `defsecret` nodes
- [ ] Secret resolver updated for new model
- [ ] Spec prose updated
- [ ] Example 07-llm-summarizer updated for new secret format
- [ ] Migration tool provided (if needed)
- [ ] All tests pass
