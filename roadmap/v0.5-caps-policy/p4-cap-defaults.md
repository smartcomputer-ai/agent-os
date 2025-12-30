---
title: "p4: Cap Defaults / DX"
status: draft
---

# p4-cap-defaults

Goal: reduce capability boilerplate while preserving explicit security boundaries.

## Problems Observed

- Every example must define `defcap` objects (often identical `sys/*` caps).
- Every `defcap` must spell out an enforcer even when it should be allow-all.
- Every reducer must bind a cap slot even when only one matching grant exists.

## Already Done

### 1) Default enforcer

If a `defcap` omits `enforcer`, the runtime defaults to `sys/CapAllowAll@1`.

Changes:
- `defcap.enforcer` is optional in schema.
- `DefCap` fills a default enforcer in the model.
- Spec notes the default behavior.

Status: DONE.

## Planned DX Improvements (Do All)

### 2) Auto-include built-in `sys/*` defcaps

Make `sys/*` cap definitions (timer/blob/http/llm/query/secret) implicit, similar to
the built-in schema/effect catalogs. This lets examples skip `capabilities.air.json`
entirely and only define grants.

Structure change:
- Loader merges `spec/defs/builtin-caps.air.json` into the manifest cap catalog.
- External manifests may still define non-`sys/*` defcaps; `sys/*` names remain reserved.

Impact:
- `manifest.caps` can be empty for common effects.
- Grants continue to reference named defcaps (`cap: "sys/timer@1"`).

### 3) Inline defcap definitions in grants

Allow a `cap_grant` to carry the defcap definition inline instead of referencing a
separate defcap object.

Structure change (one option):
```
CapGrant:
  name: text
  cap: Name(defcap)
  cap_def?: defcap            // if present, registers a defcap with the same name
  params: Value
  expiry_ns?: nat
```

Rules:
- `cap_def.name` must match `cap`.
- If both inline and top-level defcap exist, they must be identical.

Impact:
- Single-file manifests can define a grant + schema + enforcer in one place.
- Helps avoid separate `capabilities.air.json` in examples.

### 4) Default reducer cap-slot binding

Auto-bind `cap_slot: "default"` for reducers when there is exactly one matching grant
for the effect kind, and no explicit module binding exists.

Structure change:
- None in AIR. Binding resolution becomes:
  - If module binding exists for the slot, use it.
  - Else, if slot is "default" and there is exactly one grant for the effect cap_type,
    use it.
  - Else, error as before.

Impact:
- Reduces boilerplate for simple reducers that only emit one effect type.
- Still deterministic; ambiguous cases remain errors.

## Additional Hardening

### 5) Lock down `sys/*` definitions at load time

Reject any external manifest entries named `sys/*` for defcap, defmodule, defschema,
defplan, defpolicy, defeffect, and defsecret. Only built-in catalogs may define `sys/*`.

Implications:
- Examples no longer carry `sys/*` definitions in their own assets.
- Loader/asset ingestion performs a strict namespace check before merge.

## Notes / Open Questions

- Auto-include should be always-on for built-in catalogs.
- Inline defcaps in grants should be normalized by lifting into the manifest catalog
  during load (identity-checked against any existing defcap).
- Default bindings must not mask mistakes; restrict to single-grant cases only.
