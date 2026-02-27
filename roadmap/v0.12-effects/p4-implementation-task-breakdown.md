# P4: Implementation Task Breakdown (Crate-by-Crate)

**Priority**: P4  
**Status**: Proposed  
**Date**: 2026-02-22

## Goal

Turn P1-P3 in this slice into an executable work plan with:

1. ordered milestones,
2. crate-level task checklists,
3. test and rollout gates,
4. explicit ownership boundaries.

Reference docs:

1. `roadmap/v0.10.3-effects/p1-current-state-and-gap-analysis.md`
2. `roadmap/v0.10.3-effects/p2-manifest-effect-bindings-and-host-profiles.md`
3. `roadmap/v0.10.3-effects/p3-adapter-pluggability-architecture-and-rollout.md`

## Scope Summary

This plan covers:

1. startup compatibility checks for external effects,
2. optional manifest-level `effect_bindings`,
3. adapter-provider abstraction (`adapter_id` routing),
4. remote-provider pilot path.

This plan does not change:

1. reducer/plan business logic boundary,
2. kernel cap/policy authority,
3. receipt-driven replay semantics.

## Milestone Order

## M1: Fail-Fast Startup Compatibility (no schema changes)

Objective: Replace runtime `adapter.missing` surprises with world-open failures
for required external effects.

## M2: Manifest `effect_bindings` (optional)

Objective: Add governed route metadata (`kind -> adapter_id`) while preserving
legacy fallback in v0.10.x.

## M3: Provider Abstraction in Host

Objective: Decouple adapter route id from concrete in-process implementation.

## M4: Remote Provider Pilot

Objective: Prove one effect family can run via remote worker/provider while
keeping replay and audit guarantees.

## M5: Strict Mode + Hardening

Objective: Require explicit bindings/routes in production mode and tighten
signature/diagnostics behavior.

## Crate-by-Crate Checklist

## 1) `spec/`

- [ ] Add manifest schema extension for optional `effect_bindings` entries:
  `kind`, `adapter_id`, `required?`.
- [ ] Update `spec/03-air.md` manifest section with final binding semantics and
  precedence rules.
- [ ] Clarify internal effect behavior with bindings (ignored/forbidden/reserved
  ids; choose one and document).
- [ ] Add migration note: legacy fallback remains in v0.10.x and strict mode is
  opt-in.

Exit criteria:

- [ ] `spec/schemas/manifest.schema.json` validates examples with and without
  bindings.
- [ ] `spec/03-air.md` and schema are semantically aligned.

## 2) `crates/aos-air-types`

- [ ] Extend `Manifest` type with `effect_bindings: Vec<...>` (or optional).
- [ ] Add type definitions for binding entries.
- [ ] Extend JSON/CBOR serde support for the new field.
- [ ] Add validator checks:
  - duplicate `kind`,
  - unknown `kind`,
  - invalid `adapter_id` format (if constrained),
  - internal-kind rule enforcement.
- [ ] Add normalization behavior for deterministic ordering of bindings.
- [ ] Add tests for:
  - valid manifest with bindings,
  - duplicate kind rejection,
  - unknown kind rejection,
  - omitted bindings compatibility.

Exit criteria:

- [ ] Existing manifests (without bindings) still load.
- [ ] New bindings are stable in canonical form and included in manifest hash.

## 3) `crates/aos-store`

- [ ] Ensure manifest load path handles new field transparently.
- [ ] Ensure no regressions in catalog loading when `effect_bindings` present.
- [ ] Add tests verifying load/roundtrip from canonical CBOR with bindings.

Exit criteria:

- [ ] `load_manifest_from_bytes` behavior is unchanged for old manifests.
- [ ] `effect_bindings` survives load/store/rehydrate.

## 4) `crates/aos-kernel`

- [ ] Keep current internal effect execution path unchanged.
- [ ] Add helper APIs exposing required external effect kinds for host preflight:
  - from loaded plans (`emit_effect.kind`),
  - from reducer micro-effects (if externally dispatched on host path).
- [ ] Add runtime metadata accessors for resolved binding map
  (`kind -> adapter_id`, `required`).
- [ ] Ensure governance apply/swap carries new manifest field safely.
- [ ] Add tests for:
  - loaded manifest with bindings available to host API,
  - governance apply updates binding map.

Exit criteria:

- [ ] No kernel replay behavior changes.
- [ ] Host can query binding requirements without introspecting private state.

## 5) `crates/aos-host`

## M1 tasks (preflight first)

- [ ] Add world-open compatibility preflight:
  - compute required external effect kinds,
  - check route availability,
  - fail open with detailed diagnostics.
- [ ] Keep internal kinds excluded from adapter requirement set.
- [ ] Add explicit error type for startup compatibility failures.

## M2/M3 tasks (bindings + providers)

- [ ] Introduce route resolver:
  - internal handling first,
  - manifest binding next,
  - legacy fallback last (v0.10.x compatibility).
- [ ] Add provider abstraction (`adapter_id -> provider`), preserving current
  in-process adapters through wrapper providers.
- [ ] Update dispatch path to resolve by `adapter_id` route, not only `kind`.
- [ ] Preserve timeout and error wrapping semantics in registry/provider path.
- [ ] Add config support for host profiles (`adapter_routes` map).

## M4 tasks (remote pilot)

- [ ] Add one remote provider implementation (queue or RPC backed) for a pilot
  effect family (`llm.generate` or `exec.shell`).
- [ ] Add dedupe/idempotency behavior keyed by `intent_hash`.
- [ ] Add receipt ingress validation hooks for provider-id/signature policy.

## M5 tasks (strict mode)

- [ ] Add strict mode flag:
  - require explicit binding for every external effect,
  - disable legacy fallback.
- [ ] Improve diagnostics output for operator-facing route mismatches.

Exit criteria:

- [ ] Missing required route fails at open, not first execution.
- [ ] Existing fixtures still run in compatibility mode.
- [ ] Strict mode blocks ambiguous routing.

## 6) `crates/aos-effects`

- [ ] Confirm effect/receipt shared types need no contract changes for bindings.
- [ ] If adding provider/route metadata in receipts, define minimal typed helper
  additions without changing receipt determinism semantics.
- [ ] Add tests for any new helper serialization, if introduced.

Exit criteria:

- [ ] No mandatory downstream migration in existing adapters unless required by
  explicit new features.

## 7) `crates/aos-cli`

- [ ] Add surfacing in `aos world info` / defs-related commands for:
  - effect binding count,
  - unresolved route diagnostics (if host profile provided).
- [ ] Add init/template guidance for optional `effect_bindings`.
- [ ] Add docs/help text for compatibility vs strict mode.

Exit criteria:

- [ ] Operators can discover route config issues from CLI without reading logs.

## 8) `crates/aos-smoke`

- [ ] Add fixture proving startup failure on missing required adapter route.
- [ ] Add fixture proving successful startup with explicit bindings.
- [ ] Add compatibility fixture proving no-bindings legacy fallback still works.
- [ ] Add pilot remote-provider smoke (opt-in lane if external infrastructure is
  required).
- [ ] Add replay-parity check for provider-mode runs where deterministic receipts
  are available.

Exit criteria:

- [ ] Smoke coverage includes fail-fast, success, and compatibility modes.

## 9) `crates/aos-host/tests*` and integration suites

- [ ] Add unit tests for route resolution precedence.
- [ ] Add integration tests for startup preflight error payloads.
- [ ] Add provider timeout/error mapping tests with `adapter_id` context.
- [ ] Add journal/trace assertions to ensure adapter route identity is visible
  and stable.

Exit criteria:

- [ ] No regression in current adapter integration tests.

## 10) Docs and project indexing

- [ ] Update `roadmap/v0.10.3-effects/README.md` as milestones close.
- [ ] Add cross-links from `roadmap/v0.10-agent-sdk/p4-agent-effects.md` to
  this slice when implementation starts.
- [ ] If architecture semantics change, update `AGENTS.md` index pointers.

Exit criteria:

- [ ] Roadmap docs reflect shipped behavior, not intended-only behavior.

## Milestone Gates

## Gate M1 (required before M2)

- [ ] Host preflight exists and is enabled by default in compatibility mode.
- [ ] Missing-route failure is test-covered.

## Gate M2 (required before M3)

- [ ] Manifest schema + AIR types + validator support for `effect_bindings`
  merged.
- [ ] Governance/apply path handles the new field.

## Gate M3 (required before M4)

- [ ] Provider abstraction merged with in-process parity.
- [ ] Legacy fallback path still green.

## Gate M4 (required before M5)

- [ ] One remote provider pilot path operational.
- [ ] Replay and trace invariants validated for pilot flows.

## Gate M5 (release hardening)

- [ ] Strict mode production-ready.
- [ ] Signature/route diagnostics policies documented and tested.

## Suggested Workstream Split

## Track A: Contracts and Validation

Scope:

1. `spec/`,
2. `aos-air-types`,
3. `aos-store`,
4. `aos-kernel` manifest metadata APIs.

## Track B: Host Routing and Providers

Scope:

1. `aos-host` preflight,
2. route resolver,
3. provider abstraction,
4. strict mode and diagnostics.

## Track C: Integration and Ops

Scope:

1. `aos-cli`,
2. `aos-smoke`,
3. integration test suites,
4. docs and rollout.

## Risk Register

- [ ] Risk: behavior split between validator and runtime catalog remains
  confusing.
  Mitigation: enforce and document one authoritative route-resolution path.
- [ ] Risk: legacy fallback masks misconfiguration.
  Mitigation: warnings in compatibility mode; strict mode gate.
- [ ] Risk: remote provider introduces opaque failures.
  Mitigation: standardized diagnostics and adapter_id-centric tracing.

## Definition of Done (v0.10.3 slice)

- [ ] External effect availability failures are detected at world-open.
- [ ] Optional `effect_bindings` are supported end-to-end.
- [ ] Host dispatch can route by logical `adapter_id`.
- [ ] At least one remote-provider pilot effect family is validated.
- [ ] Compatibility mode preserves current fixture behavior.
- [ ] Strict mode and documentation are complete for production adoption.

