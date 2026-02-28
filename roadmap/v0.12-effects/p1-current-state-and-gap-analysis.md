# P1: Effect Runtime Contract and Gap Analysis (as implemented)

**Priority**: P1  
**Status**: In Progress (baseline + partial implementation)  
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

That split is valid, but today there is still no governed world-level adapter
route **enforcement** in host dispatch/preflight.

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

## 5) Adapter dispatch is still kind-keyed host registry wiring

`WorldHost::run_cycle` partitions internal vs timer vs external intents, then
dispatches external intents through `AdapterRegistry`:

- `crates/aos-host/src/host.rs:369`
- `crates/aos-host/src/host.rs:386`
- `crates/aos-host/src/host.rs:400`
- `crates/aos-host/src/host.rs:411`

Registry lookup is by effect `kind`; missing adapter returns `adapter.missing`
error receipt:

- `crates/aos-host/src/adapters/registry.rs:47`
- `crates/aos-host/src/adapters/registry.rs:71`

Default adapter wiring remains hardcoded + feature-flag driven:

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

However, host dispatch is still kind-keyed (`intent.kind`) and does not yet
route by `adapter_id` from `effect_bindings`.

## 8) No startup compatibility preflight for required external kinds

`WorldHost::open`/`open_dir` load manifest, build kernel, build default registry,
and return host; there is no fail-fast compatibility check for required external
effect routes:

- `crates/aos-host/src/host.rs:75`
- `crates/aos-host/src/host.rs:139`
- `crates/aos-host/src/host.rs:160`
- `crates/aos-host/src/host.rs:258`

Result: missing routes surface late as runtime `adapter.missing` receipts.

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
2. Adapter-route availability is not checked at startup.
3. `origin_scope` exists in type model, but is not currently enforced in
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

Still open for P1:

1. Host preflight at world-open for required external route availability.
2. Startup diagnostics for required routes vs provided routes.
3. Conformance coverage for startup missing-route failure path.
4. Any host/runtime use of `effect_bindings` for dispatch (still kind-keyed;
   adapter-id routing remains P2/P3 runtime work).

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
