# P1: Module Workflow Foundation (No Plan Dependency for New Flows)

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/README.md`

## Goal

Enable deterministic, event-driven orchestration directly in modules so new flows can run with zero `defplan` usage.

This phase intentionally reuses the existing module/reducer execution path to avoid introducing a second VM/ABI track before behavior is proven.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

## Hard-Break Assumptions

1. Backward compatibility is not required.
2. We may change reducer effect semantics and break existing plan-era fixtures.
3. We prioritize clean forward architecture over preserving legacy behavior.

## Scope

### 1) Remove single-effect constraints in module execution

1. Remove kernel gate that limits reducers/modules to a single emitted effect.
2. Remove SDK trap that enforces one effect per invocation.
3. Introduce deterministic kernel limits:
   - max effects per tick,
   - max emitted events per tick,
   - max output bytes per tick.

### 2) Make receipt delivery generic for module-origin effects

1. Replace hardcoded timer/blob reducer receipt translation with a generic receipt event envelope.
2. Keep typed timer/blob envelopes as optional helpers, not runtime requirements.
3. Ensure module key propagation and routing remain deterministic.

### 3) Expand effect-origin permissions for module workflows

1. Update effect origin handling so orchestration modules can emit needed kinds (`http.request`, `llm.generate`, `workspace.*`, `governance.*`, `introspect.*`) under caps/policy.
2. Keep cap and policy enforcement unchanged in authority and order.
3. Keep internal effect handling boundary unchanged (kernel handles internal kinds).

### 4) Minimal workflow pattern fixtures

1. Add at least one flow fixture implemented without plans:
   - event in,
   - multi-effect chain,
   - receipt-driven continuation,
   - domain event out.
2. Add replay check for the fixture.

## Out of Scope

1. Removing plan runtime code.
2. Broad AIR model/schema reset (`DefPlan` removal, manifest section removals, patch op removal).
3. Governance summary model changes.

## Work Items by Crate

### `crates/aos-kernel`

1. `world/event_flow.rs`: remove single-effect guard and add deterministic per-tick limits.
2. `receipts.rs`: generic receipt event encoding path.
3. `effects.rs`: origin scope checks updated for module orchestration requirements.

### `crates/aos-wasm-sdk`

1. `reducers.rs`: remove `effect_used`/single-effect trap.
2. Keep deterministic failure messaging on output-limit violations (now enforced in kernel).

### `spec/defs` and `crates/aos-air-types`

1. Adjust built-in effect origin scopes as needed for module workflow emission.
2. Keep plan model intact in this phase.

### `crates/aos-smoke` / `crates/aos-host/tests`

1. Add no-plan orchestration fixture and replay assertions.

## Acceptance Criteria

1. At least one non-trivial workflow executes fully without any `defplan`.
2. Flow includes multiple effects emitted from module code in one logical workflow.
3. Replay from genesis remains deterministic for the new fixture.
4. Caps/policies still gate every effect intent.
