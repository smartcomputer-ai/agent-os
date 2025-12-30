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

Status: DONE.

### 3) Inline defcap definitions in grants

Skipped for now. The combination of default enforcer + built-in `sys/*` caps +
default slot binding should remove most of the boilerplate without introducing
asymmetry or governance complexity.

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

Status: DONE.

### 5) Example simplification sweep

After steps (2) + (4), the following examples can be simplified:

- `examples/01-hello-timer`
  - Remove `air/capabilities.air.json` (sys/timer now built-in).
  - Drop `sys/timer@1` from `manifest.caps`.
  - If reducer uses the default slot, remove `module_bindings` (or keep until slot rename).

- `examples/02-blob-echo`
  - Remove `air/capabilities.air.json` (sys/blob now built-in).
  - Drop `sys/blob@1` from `manifest.caps`.
  - If reducer uses the default slot, remove `module_bindings`.

- `examples/08-retry-backoff`
  - Remove `air/capabilities.air.json` (sys/timer now built-in).
  - Drop `sys/timer@1` from `manifest.caps`.
  - If reducer uses the default slot, remove `module_bindings`.

- `examples/09-worldfs-lab`
  - Remove `sys/blob@1` and `sys/query@1` from `air/capabilities.air.json`
    (built-in); keep only the custom `notes/blob_cap@1`.
  - Drop `sys/blob@1` + `sys/query@1` from `manifest.caps`.
  - If `cap_sys_blob` / `cap_query` grants are unused, drop them too.
  - Remove empty `module_bindings` from the manifest.

- `examples/03-fetch-notify`
  - Keep custom `demo/http_fetch_cap@1`; still needs the HTTP enforcer.
  - Remove empty `module_bindings` from the manifest.

- `examples/04-aggregator`
  - Keep custom `demo/http_aggregate_cap@1`.
  - Remove `enforcer` field (defaults to allow-all).
  - Remove empty `module_bindings` from the manifest.

- `examples/05-chain-comp`
  - Keep custom `demo/http_chain_cap@1`.
  - Remove `enforcer` field (defaults to allow-all).
  - Remove empty `module_bindings` from the manifest.

- `examples/06-safe-upgrade` (air.v1 + air.v2)
  - Keep custom HTTP caps.
  - Remove `enforcer` fields where allow-all is intended.
  - Remove empty `module_bindings` from the manifest.

- `examples/07-llm-summarizer`
  - Keep HTTP and LLM caps; LLM still uses `sys/CapEnforceLlmBasic@1`.
  - Remove `enforcer` from the HTTP defcap if allow-all is intended.
  - Remove empty `module_bindings` from the manifest.

Status: DONE (examples updated and `cargo run -p aos-examples -- all` passes).

## Additional Hardening

### 6) Lock down `sys/*` definitions at load time

Reject any external manifest entries named `sys/*` for defcap, defmodule, defschema,
defplan, defpolicy, defeffect, and defsecret. Only built-in catalogs may define `sys/*`.

Implications:
- Examples no longer carry `sys/*` definitions in their own assets.
- Loader/asset ingestion performs a strict namespace check before merge.

Status: DONE.

## Notes / Open Questions

- Auto-include should be always-on for built-in catalogs.
- Default bindings must not mask mistakes; restrict to single-grant cases only.

## Other Possible Improvements Noticed

- `examples/04-aggregator` still uses `verbs` in the HTTP cap schema; align to
  `methods` to match the current spec.
