# P2: Manifest Effect Bindings and Host Profiles

**Priority**: P2  
**Status**: Proposed  
**Date**: 2026-02-22

## Goal

Introduce a governed, world-visible way to route effect kinds to adapter
identities while preserving:

1. deterministic kernel semantics,
2. world portability, and
3. local/cloud implementation swapping without rebaking adapters into host code.

This phase introduces configuration shape and validation, not a remote adapter
runtime yet.

## Design Principles

1. `defeffect` remains type/cap/origin contract only.
2. Adapter implementation details remain outside reducer/plan logic.
3. Manifest controls routing intent; host controls implementation availability.
4. Kernel keeps responsibility for emit/cap/policy/journal; host remains the
   external execution boundary.
5. Same manifest should run under multiple host profiles (local/dev/cloud),
   with explicit compatibility checks.

## Proposed AIR Additions

Add optional manifest field:

`effect_bindings: [EffectBinding ...]`

Proposed shape (v1-compatible extension):

```json
{
  "kind": "llm.generate",
  "adapter_id": "llm.default",
  "required": true
}
```

Fields:

1. `kind` (EffectKind): must correspond to a loaded effect definition.
2. `adapter_id` (text): logical route id (not implementation binary/class name).
3. `required` (bool, default `true`):
   - `true`: world must fail startup if host cannot satisfy route.
   - `false`: host may allow startup if effect is unreachable or policy-blocked.

Notes:

1. Internal kernel-handled kinds may omit bindings entirely.
2. If bound, internal kinds must target reserved adapter ids (`kernel.*`) and
   validate accordingly.

## Why `adapter_id` is logical

Do not encode "native Rust class path", "WASI module path", or queue topic
details in manifest. Use a stable logical id (`llm.default`, `exec.sandbox`).
Host profile maps logical id -> concrete implementation.

This enables:

1. local: in-process adapter,
2. cloud: remote worker-backed adapter,
3. test: deterministic mock adapter.

without changing plan/reducer AIR.

## Host Profile Model

Introduce host config concept:

`adapter_routes: { adapter_id -> AdapterProviderSpec }`

Examples:

1. `llm.default -> InProcess(host.llm.openai)`
2. `llm.default -> Remote(queue://universe/<u>/adapters/llm)`
3. `exec.sandbox -> InProcess(host.exec.shell)`

Compatibility check at world-open:

1. collect externally executed effect kinds required by world,
2. resolve `kind -> adapter_id` using manifest bindings or fallback defaults,
3. verify host profile can provide each adapter id.

If not, fail open with deterministic diagnostic message.

## Binding Resolution Rules

At runtime dispatch:

1. If effect kind is kernel internal (`workspace.*`, `introspect.*`,
   `governance.*`) route internal first.
2. Else if manifest binding exists for kind, use bound `adapter_id`.
3. Else fallback to legacy kind-based registry mapping for compatibility in
   v0.10.x.
4. Missing route for required kind is startup error (not first-use runtime
   error).

## Validation Rules

Static (manifest load):

1. `effect_bindings.kind` must exist in known effect defs for that world.
2. no duplicate `kind` entries.
3. bindings for reducer-only effects are allowed only if actually externally
   dispatched on that host path (timer special-cased as today).

Runtime preflight (host open):

1. every required bound external kind has a resolved provider.
2. produce diff-style diagnostics:
   - world requires: `{kind -> adapter_id}`
   - host provides: `{adapter_id -> provider}`

## Compatibility and Migration

Phase-in strategy:

1. P2.1 (compat): `effect_bindings` optional.
2. If absent, legacy kind-based defaults still work.
3. Emit warning when external kind is used without explicit binding.
4. Future phase can require explicit bindings for non-internal effects.

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

Those are always kernel-handled regardless of external profile.

## Deliverables

1. AIR/schema extension for optional `effect_bindings`.
2. loader + validator support.
3. host preflight compatibility check and diagnostics.
4. legacy fallback preserved in v0.10.x.
5. docs update in `spec/03-air.md` for finalized binding shape.

## Open Questions

1. Should `required` default to `true` for all bindings or infer by reachability?
2. Should internal kinds be forbidden in `effect_bindings` to reduce ambiguity?
3. At what release do we remove legacy kind-based fallback?

