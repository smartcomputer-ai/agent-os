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

## Chosen Design (clean refactor)

- Make secrets first-class nodes: add `defsecret` + `AirNode::Defsecret`; manifest `secrets` becomes `[{name, hash}]`.
- `DefSecret` stores only `name` (alias@version) plus binding/digest/ACL; loader parses `name` into `(alias, version)` and enforces invariants.
- Keep `SecretRef` canonical as `{alias, version}` for hashing/replay; optionally allow a sugar that parses a `name` string during normalization.
- Loader resolves manifest refs into `DefSecret` nodes, derives `(alias, version)` keys, and builds a runtime `SecretCatalog`; no inline manifest secrets remain.
- Validation enforces parsed alias/version uniqueness, version bounds, binding presence, digest format, and ACL targets (caps/plans) at load time.
- Migration tool lifts existing inline secrets into defsecret nodes and rewrites manifest `secrets` to named refs.

## Proposed Design (details)

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
- **name**: Standard versioned name (`alias@version`)
- **alias/version (derived)**: Parsed from `name` during validation; enforced to be well-formed and `version >= 1`
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
  "secrets": [{"name": "...", "hash": "sha256:..."}]  // now consistent
}
```

### Updated SecretRef

Keep canonical `{alias, version}`:

```json
{
  "record": {
    "alias": { "text": {} },
    "version": { "nat": {} }
  }
}
```

Optional sugar (if desired later): accept `{ "name": "payments/stripe@1" }` during normalization, parsing to the canonical form so hashing stays stable.

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

- Resolve `manifest.secrets: Vec<NamedRef>` → load `DefSecret` nodes by hash → parse `name` into `(alias, version)` (enforce `version >= 1`) → build:
  - `HashMap<Name, DefSecret>` (by def name) for governance/diff/shadow tools
  - `SecretCatalog` keyed by `(alias, version)` for runtime enforcement/injection
- Remove reliance on inline `Manifest.secrets`; loader is the single place that materializes secrets into runtime shape.

### Step 5: Update secret resolver

- Kernel receives the `SecretCatalog` derived above; no code should read manifest inline secrets.
- Keep resolver contract: resolve by `binding_id`, optional `expected_digest`.
- Normalize secret variants and inject using the new catalog; no behavior change, just new source of truth.
- Catalog lookup is still by `(alias, version)`; these are derived from `DefSecret.name` so there is no drift between serialized data and runtime keys.

### Step 6: Update SecretRef handling

- Keep canonical `{alias, version}` for hashing/replay.
- Optional sugar (later): accept `"name": "alias@version"` during normalization, parsing to canonical form so hashes stay stable.
- If we adopt name-based schema, the diff would be the same shape as above but should only be applied alongside normalization logic.

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

## Suggested execution order (granular)

1. Schemas: add `defsecret.schema.json`; update `manifest.schema.json` to `NamedRef` for secrets.
2. Types: add `DefSecret` + `AirNode::Defsecret`; update serde defaults and trait derives.
3. Store loader: load defsecret nodes, parse `name` → `(alias, version)`, build `SecretsIndex/SecretCatalog`, drop use of inline `Manifest.secrets`.
4. Validation: extend `aos-air-types::validate` and `aos-store` validation to cover parsed alias/version uniqueness, binding presence, version >= 1, digest format, and ACL target existence.
5. Runtime: rewire `aos-kernel` to consume the new catalog; ensure effect injection/policy paths use the loader-derived catalog only.
6. Builtins: keep `sys/SecretRef@1` canonical; optionally add name-based sugar plus normalization if desired.
7. Specs/docs: update `spec/03-air.md`, `spec/17-secrets.md`, and any governance prose that lists defkinds.
8. Examples/tests: rewrite example manifests (07) and update tests/goldens that construct inline secrets; add new tests for loader + validation + injection using defsecret nodes.
9. Migration tool: CLI to lift inline secrets → defsecret nodes and rewrite manifests (helps existing fixtures).

## Acceptance Criteria

- [ ] `defsecret.schema.json` added with correct shape
- [ ] `manifest.schema.json` updated to use `[{name, hash}]` for secrets
- [ ] Rust `DefSecret` type + `AirNode::Defsecret` implemented (serialized fields match schema)
- [ ] Manifest loader resolves secret refs into `DefSecret` nodes, parses name → alias/version, and builds catalog/index
- [ ] Secret validation covers parsed alias/version uniqueness, binding, version, digest format, ACL target existence
- [ ] Runtime secret injection/policy uses loader-derived catalog (no inline manifest dependency)
- [ ] Spec prose updated
- [ ] Example 07-llm-summarizer updated for new secret format
- [ ] Migration tool provided (if needed)
- [ ] All tests pass
