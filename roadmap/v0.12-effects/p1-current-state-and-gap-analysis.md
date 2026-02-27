# P1: Effect Runtime Contract and Gap Analysis (as implemented)

**Priority**: P1  
**Status**: Proposed  
**Date**: 2026-02-22

## Goal

Document exactly how effect definitions and adapter dispatch work today, clarify
what `manifest.effects` currently controls, and identify the minimal changes
needed before introducing a pluggable adapter model.

This is a "truth first" doc: no new abstractions until the current contract is
explicit.

## Why this exists

The current implementation has a split control surface:

1. Effect kinds and schemas are world data (`defeffect` + manifest refs).
2. Adapter execution is mostly host wiring (default registry in `aos-host`).
3. Internal effects are kernel-handled (`workspace.*`, `introspect.*`, `governance.*`).

The split is valid, but it creates confusion about where world requirements are
declared versus where host capabilities are provided.

## Current Behavior (Code Reality)

## 1) `manifest.effects` is loaded and used

Manifest loading resolves `manifest.effects` as `NodeKind::Effect` and inserts
matching `defeffect` nodes (built-in or custom) into the catalog:

- `crates/aos-store/src/manifest.rs:87`
- `crates/aos-store/src/manifest.rs:183`

Authoring manifests may omit hashes; loader fills canonical hashes:

- `crates/aos-host/src/manifest_loader.rs:294`
- `crates/aos-host/src/manifest_loader.rs:328`
- `crates/aos-host/src/manifest_loader.rs:391`

## 2) Loaded manifest builds an effect catalog from loaded effect defs

Kernel `LoadedManifest.effect_catalog` is built from loaded `defeffect` nodes:

- `crates/aos-kernel/src/manifest.rs:80`

This catalog is then used at runtime for:

1. Origin scope checks (`reducer`/`plan`).
2. Param normalization schema lookup.
3. Capability type matching.

References:

- `crates/aos-kernel/src/effects.rs:225`
- `crates/aos-kernel/src/effects.rs:248`
- `crates/aos-kernel/src/effects.rs:267`
- `crates/aos-kernel/src/capability.rs:94`

## 3) Plan emit-effect literals depend on effect catalog membership

Plan normalization resolves `emit_effect.kind` -> params schema through
`EffectCatalog`. Missing kind in that catalog fails normalization:

- `crates/aos-air-types/src/plan_literals.rs:67`

Practical consequence: world manifests must include refs for effect defs used by
plans/reducers, even for built-ins.

## 4) Validator currently also includes built-ins globally

`validate_manifest` builds `known_effect_kinds` from:

1. all built-in effects, and
2. loaded manifest effect defs.

References:

- `crates/aos-air-types/src/validate.rs:614`
- `crates/aos-air-types/src/validate.rs:618`

This is broader than `effect_catalog`-driven plan normalization and is one
reason people perceive built-ins as "auto-available."

## 5) `agent-live` fixture does define `effects`

`22-agent-live` includes workspace effect refs:

- `crates/aos-smoke/fixtures/22-agent-live/air/manifest.air.json:90`

The same fixture's live LLM call is executed directly by harness code, not by
world plan dispatch:

- `crates/aos-smoke/src/agent_live.rs:448`

That is why `llm.generate` is not part of that fixture's world-level effect use.

## 6) Adapter dispatch today is host registry wiring

`WorldHost::run_cycle` partitions:

1. internal intents (`kernel.handle_internal_intent`),
2. timer scheduling path,
3. external intents -> adapter registry.

References:

- `crates/aos-host/src/host.rs:365`
- `crates/aos-host/src/host.rs:382`
- `crates/aos-host/src/host.rs:407`

Default registry wiring is currently hardcoded by kind + feature flags:

- `crates/aos-host/src/host.rs:518`

Missing adapter yields `adapter.missing` error receipt:

- `crates/aos-host/src/adapters/registry.rs:69`

Internal effect kinds are explicit in kernel:

- `crates/aos-kernel/src/internal_effects/mod.rs:16`

## 7) Custom adapter registration already exists (programmatic)

The host exposes mutable adapter registry access and testhost registration:

- `crates/aos-host/src/host.rs:286`
- `crates/aos-host/src/testhost.rs:206`

So pluggability exists at process wiring level, but not yet as governed
manifest/runtime configuration.

## Spec Intent and Mismatch Notes

AIR prose says:

1. `manifest.effects` is authoritative for world effect catalog.
2. list every effect used by the world.
3. future adapter binding should be manifest-level (`effect_bindings`) and
   separate from `defeffect`.

References:

- `spec/03-air.md:144`
- `spec/03-air.md:263`
- `spec/03-air.md:522`
- `spec/03-air.md:525`

Manifest schema already requires an `effects` array:

- `spec/schemas/manifest.schema.json:25`
- `spec/schemas/manifest.schema.json:121`

Main mismatch to resolve in this slice:

1. Built-ins are globally known in validator.
2. Adapter availability is discovered late (runtime missing adapter receipt),
   not as a startup compatibility check.

## Decision for v0.10.3

1. Keep current hardwired defaults for immediate compatibility.
2. Add fail-fast startup checks for external effect availability.
3. Preserve kernel as the deterministic boundary (intent/receipt/journal).
4. Keep adapter binding metadata out of `defeffect`; use manifest-level binding
   in P2.

## P1 Deliverables

1. Runtime/docs contract for "what goes in `manifest.effects`":
   every effect def used by plans/reducers in that world.
2. Host preflight check:
   - enumerate non-internal effect kinds reachable from plan/reducer emits,
   - ensure adapter route exists for each,
   - fail world open if required kind has no route.
3. Diagnostics:
   - include kind, plan/reducer origins, and proposed adapter ids in error.
4. Conformance tests:
   - fixture with missing adapter fails at startup,
   - fixture with all routes opens and runs.

## Non-Goals

1. No kernel execution semantics change.
2. No receipt model changes.
3. No dynamic plugin loading contract yet.

