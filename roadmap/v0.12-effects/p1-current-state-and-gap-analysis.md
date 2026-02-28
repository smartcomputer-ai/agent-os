# P1: Effect Runtime Contract and Gap Analysis (as implemented)

**Priority**: P1  
**Status**: Complete  
**Date**: 2026-02-28

## Goal

Document the current effect contract in the workflow-era runtime, then define
the minimum P1 changes needed before adapter-route pluggability (`P2`/`P3`).

This document is intentionally "truth first": code reality before new design.

## Why this exists

The current control surface is still split:

1. Effect type/cap/schema definitions are world data (`defeffect` + `manifest.effects` refs).
2. External execution is host wiring (`aos-host` adapter registry).
3. Internal effects are kernel-handled (`workspace.*`, `introspect.*`, `governance.*`).

That split is valid. As of this update, manifest-level route metadata is
enforced at host open (preflight) and consumed at dispatch with compatibility
fallback.

## Current Behavior (Code Reality)

## 1) `manifest.effects` is required by schema and loaded into catalog nodes

Manifest schema requires `effects`, and the model includes `effects: Vec<NamedRef>`:

- `spec/schemas/manifest.schema.json:21`
- `spec/schemas/manifest.schema.json:101`
- `crates/aos-air-types/src/model.rs:655`

Store manifest loading resolves `manifest.effects` as `NodeKind::Effect`; built-ins
are only loaded when referenced:

- `crates/aos-store/src/manifest.rs:82`
- `crates/aos-store/src/manifest.rs:176`

AIR JSON asset loading patches canonical effect hashes into manifest refs:

- `crates/aos-host/src/manifest_loader.rs:277`

## 2) Effect catalog is built from loaded `defeffect` nodes only

Both kernel and host asset loader build `EffectCatalog` from loaded effect defs:

- `crates/aos-kernel/src/manifest.rs:73`
- `crates/aos-host/src/manifest_loader.rs:461`

At enqueue time, effect params are normalized via catalog/schema lookup and cap
type is read from the same catalog:

- `crates/aos-kernel/src/effects.rs:236`
- `crates/aos-kernel/src/effects.rs:255`
- `crates/aos-effects/src/normalize.rs:22`

If kind is missing from the catalog, enqueue fails (`unknown effect params schema`
or `UnsupportedEffectKind`), i.e. this is runtime, not manifest-open, failure.

## 3) Validator still treats built-ins as globally known kinds

`validate_manifest` builds `known_effect_kinds` from:

1. built-in effects, plus
2. loaded manifest effect defs.

References:

- `crates/aos-air-types/src/validate.rs:88`
- `crates/aos-air-types/src/validate.rs:92`

This set is used to validate module `abi.workflow.effects_emitted` and
policy `when.effect_kind`:

- `crates/aos-air-types/src/validate.rs:301`
- `crates/aos-air-types/src/validate.rs:339`

So validator semantics are broader than the runtime catalog assembled from
`manifest.effects`.

## 4) Workflow emission authority is enforced at runtime

Workflow output processing enforces:

1. emitted kind must be in module `abi.workflow.effects_emitted`,
2. cap slot binding (or unique default grant) must resolve,
3. effect enqueue then runs canonicalization + cap/policy checks.

References:

- `crates/aos-kernel/src/world/event_flow.rs:391`
- `crates/aos-kernel/src/world/event_flow.rs:466`
- `crates/aos-kernel/src/world/event_flow.rs:483`
- `crates/aos-kernel/src/effects.rs:236`

## 5) Adapter dispatch now resolves route id, with kind-fallback compatibility

`WorldHost::run_cycle` partitions internal vs timer vs external intents, then
dispatches external intents through `AdapterRegistry`:

- `crates/aos-host/src/host.rs:369`
- `crates/aos-host/src/host.rs:386`
- `crates/aos-host/src/host.rs:400`
- `crates/aos-host/src/host.rs:411`

Registry now supports route ids (`adapter_id -> adapter kind`) while preserving
legacy kind-based lookup as compatibility fallback when no binding is present.
Missing route still yields `adapter.missing` error receipts at dispatch time:

- `crates/aos-host/src/adapters/registry.rs:47`
- `crates/aos-host/src/adapters/registry.rs:71`

Default adapter wiring remains hardcoded + feature-flag driven, now with
compatibility aliases such as `http.default` and `llm.default`:

- `crates/aos-host/src/host.rs:522`
- `crates/aos-host/src/host.rs:526`

## 6) Internal effect kinds are explicitly kernel-handled

Internal effect list is explicit in kernel and bypasses external adapters:

- `crates/aos-kernel/src/internal_effects/mod.rs:16`
- `crates/aos-kernel/src/internal_effects/mod.rs:90`

Current internal set includes `introspect.*`, `workspace.*`, and `governance.*`.

## 7) Programmatic pluggability exists; governed routing metadata is now present

Host/testhost support programmatic adapter registration:

- `crates/aos-host/src/host.rs:287`
- `crates/aos-host/src/testhost.rs:208`

Manifest-level `effect_bindings` now exists in AIR schema/model, and validator
enforces key invariants:

1. bound kinds must be declared in loaded `defeffect` set (`manifest.effects`),
2. no duplicate `kind` entries,
3. internal kinds (`workspace.*`, `introspect.*`, `governance.*`) are forbidden.

- `spec/schemas/manifest.schema.json:25`
- `crates/aos-air-types/src/model.rs:657`
- `crates/aos-air-types/src/validate.rs:256`

Host dispatch now resolves effective route by:

1. manifest binding (`kind -> adapter_id`) when present,
2. fallback to legacy `kind` route when absent (compat mode).

## 8) Startup compatibility preflight is now implemented

`WorldHost::open`/`open_dir` now performs fail-fast preflight for required
external effect routes before kernel construction:

- `crates/aos-host/src/host.rs:75`
- `crates/aos-host/src/host.rs:139`
- `crates/aos-host/src/host.rs:160`
- `crates/aos-host/src/host.rs:258`

Preflight semantics:

1. enumerate non-internal kinds from loaded effect defs,
2. resolve effective route (`binding` or `kind` fallback),
3. fail startup with diagnostics when route is unavailable,
4. include required kind/route and host-provided routes in diagnostics.

## 9) `agent-live` still includes direct harness-side live LLM execution

Fixture manifest includes explicit effect refs:

- `crates/aos-smoke/fixtures/22-agent-live/air/manifest.air.json:112`

The smoke harness also constructs and executes `llm.generate` intents directly
through adapter code:

- `crates/aos-smoke/src/agent_live.rs:447`
- `crates/aos-smoke/src/agent_live.rs:489`

This can obscure "world dispatch path vs harness path" when debugging adapters.

## Spec Intent and Mismatch Notes

Spec intent remains:

1. effect catalog is data-driven from `defeffect` + `manifest.effects`,
2. built-ins should be listed in world manifests when used,
3. adapter binding should live at manifest level (`effect_bindings`), not in `defeffect`.

References:

- `spec/03-air.md:262`
- `spec/03-air.md:522`
- `spec/03-air.md:525`

Main mismatches now:

1. Validator accepts built-in kinds globally, but runtime catalog only includes
   defs that were actually loaded via `manifest.effects`.
2. `origin_scope` exists in type model, but is not currently enforced in
   enqueue/dispatch paths.

## Decision for v0.12 P1

1. Keep kernel determinism and receipt boundary unchanged.
2. Keep current default adapter registry behavior for compatibility.
3. Add fail-fast host preflight for required external effect availability.
4. Keep adapter route metadata out of `defeffect`; land it as manifest-level
   data in P2.
5. Update docs/diagnostics terminology to workflow modules (not plans/reducers).

## Progress Update (2026-02-28)

Completed:

1. Governed manifest route metadata landed (`manifest.effect_bindings`) in AIR
   schema + model.
2. Semantic validation landed for binding correctness (declared kinds only,
   duplicate prevention, internal-kind rejection), with tests.
3. Baseline fixtures/manifest constructors across workspace were updated for the
   new manifest field.
4. Host startup preflight now fails open when required external routes are
   missing, with route diagnostics.
5. Host dispatch now resolves `kind -> adapter_id` via manifest bindings, with
   legacy kind fallback preserved.
6. Host conformance tests cover:
   - startup failure for missing bound route,
   - startup success for available bound route,
   - internal kinds ignored by preflight.

Still open for P1:

1. None.

## P1 Deliverables

1. Clear contract for `manifest.effects`: every effect definition that can be
   emitted or referenced by this world at runtime.
2. Host preflight check at world open:
   - enumerate non-internal externally dispatched effect kinds,
   - verify route availability for each required kind,
   - fail open with actionable diagnostics.
3. Diagnostics include effect kind, workflow origin(s), and expected route id
   (or fallback route policy).
4. Conformance tests:
   - missing required external route fails startup,
   - complete route set opens and runs,
   - validator/runtime catalog mismatch cases are explicitly covered.

## Non-Goals

1. No kernel execution semantics change.
2. No receipt model change.
3. No dynamic plugin loading contract in P1.
