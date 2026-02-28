# P2: Manifest Effect Bindings and Host Profiles

**Priority**: P2  
**Status**: Complete  
**Date**: 2026-02-28

## Goal

Introduce the smallest governed routing contract needed after P1:

1. world-visible `kind -> adapter_id` routing intent,
2. fail-fast host compatibility checks at world-open,
3. compatibility with current kind-keyed host registry during rollout.

## Design Principles

1. `defeffect` stays type/cap/origin only.
2. Manifest declares routing intent; host declares route availability.
3. Kernel remains deterministic boundary (emit/cap/policy/journal).
4. Internal effects remain kernel-handled and never use external routing.
5. Remote providers and strict mode are P3 concerns.

## Proposed AIR Additions

Add optional manifest field:

`effect_bindings: [EffectBinding ...]`

Proposed minimal shape:

```json
{
  "kind": "llm.generate",
  "adapter_id": "llm.default"
}
```

Fields:

1. `kind` (EffectKind): must correspond to a loaded effect definition.
2. `adapter_id` (text): logical route id (not implementation binary/class name).

## Why `adapter_id` is logical

Do not encode "native Rust class path", "WASI module path", or queue topic
details in manifest. Use stable ids (`llm.default`, `http.default`).

This keeps manifests portable while allowing per-host implementation differences.

## Host Profile Model (P2 minimal)

Introduce only this host config concept in P2:

`adapter_routes: { adapter_id -> AdapterProviderSpec }`

In P2, provider spec can stay implementation-defined. P2 only requires:

1. host can answer "is `adapter_id` available?",
2. host preflight fails world-open when required route is missing.

## Binding Resolution Rules

At runtime dispatch:

1. If effect kind is kernel internal (`workspace.*`, `introspect.*`,
   `governance.*`) route internal first.
2. Else if manifest binding exists for kind, use bound `adapter_id`.
3. Else fallback to legacy kind-based routing in compatibility mode.
4. Missing required route is startup error (not first-use runtime
   error).

## Validation Rules

Static (manifest load):

1. `effect_bindings.kind` must exist in known effect defs for that world.
2. no duplicate `kind` entries.
3. internal kinds are forbidden in `effect_bindings`.

Runtime preflight (host open):

1. every required external kind resolves to an effective adapter route
   (binding or compatibility fallback).
2. diagnostics include:
   - world requires: `{kind -> adapter_id}`
   - host provides: `{adapter_id -> provider}`

## Compatibility and Migration

Phase-in strategy:

1. P2.1 (compat): `effect_bindings` optional.
2. If absent, legacy kind-based defaults still work.
3. Emit warning when external kind is used without explicit binding.
4. P3 strict mode may require explicit bindings for non-internal effects.

## Security and Governance

1. Binding metadata is part of manifest data and therefore governed by existing
   proposal/shadow/apply flow.
2. Caps/policy still gate effect emission before dispatch.
3. Binding does not bypass cap/policy, and does not grant authority.

## Interaction with Existing Built-ins

Built-in effects remain defined in:

- `spec/defs/builtin-effects.air.json`

and listed in world manifest as refs where used.

Internal kinds currently include:

- `workspace.*`, `introspect.*`, `governance.*`
  (`crates/aos-kernel/src/internal_effects/mod.rs:16`)

Those are always kernel-handled and must not appear in `effect_bindings`.

## Deliverables

1. AIR/schema extension for optional `effect_bindings`.
2. loader + validator support.
3. host preflight compatibility check and diagnostics.
4. compatibility fallback preserved during rollout.
5. docs update in `spec/03-air.md` for finalized binding shape.

## Completion Notes (2026-02-28)

1. `manifest.effect_bindings` is implemented in AIR model + schema and validated
   (declared kinds only, no duplicates, no internal kinds).
2. Host profile route map is implemented via `HostConfig.adapter_routes`
   (`adapter_id -> provider spec` where provider spec currently maps to concrete
   in-process adapter kind).
3. Host preflight at world-open verifies required external kinds resolve to an
   available effective route (binding or compatibility fallback) and fails fast
   when missing.
4. Runtime dispatch resolves by binding first, then legacy kind fallback.
5. Compatibility warning is emitted for external kinds without explicit
   `effect_bindings`.

## Open Questions

1. What exact release removes legacy kind-based fallback?
2. Do we want a warning-only period before strict mode?
3. What minimum `adapter_id` format constraints do we enforce in schema?
