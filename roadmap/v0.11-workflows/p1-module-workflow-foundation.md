# P1: Module Workflow Foundation (No Plan Dependency for New Flows)

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/README.md`

## Goal

Enable deterministic, event-driven orchestration directly in modules so new flows can run with zero `defplan` usage.

This phase intentionally reuses the existing module/reducer execution path to avoid introducing a second VM/ABI track before behavior is proven, while targeting `workflow` semantics for orchestrating modules.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

## Hard-Break Assumptions

1. Backward compatibility is not required.
2. We may change reducer/module effect semantics and break existing plan-era fixtures.
3. We prioritize clean forward architecture over preserving legacy behavior.

## Scope

### 1) Remove single-effect constraints in workflow execution

1. Remove kernel gate that limits workflow modules to a single emitted effect.
2. Remove SDK trap that enforces one effect per invocation.
3. Introduce deterministic kernel limits:
   - max effects per tick,
   - max emitted events per tick,
   - max output bytes per tick.

### 2) Make receipt delivery generic for module-origin effects

1. Replace hardcoded timer/blob reducer receipt translation with a generic receipt event envelope.
2. Define required envelope fields:
   - `origin_module_id`,
   - `origin_instance_key`,
   - `intent_id`,
   - `effect_kind`,
   - `params_hash` (optional),
   - `receipt_payload`,
   - `status` (`ok|denied|faulted`),
   - `emitted_at_seq`.
3. Define deterministic `intent_id` generation that includes origin instance identity and effect identity.
4. Route receipts to `(origin_module_id, origin_instance_key)` without consulting manifest subscriptions.
5. Keep typed timer/blob envelopes as optional helpers, not runtime requirements.

### 2.1) Define workflow instance waiting model

1. Define a kernel-recognized persisted workflow instance state record, including:
   - `state_bytes`,
   - `inflight_intents`,
   - `status` (`running|waiting|completed|failed`),
   - `last_processed_event_seq`,
   - `module_version` (optional but recommended).
2. Define deterministic transitions for `running <-> waiting` based on inflight receipt count.
3. Define how `last_processed_event_seq` advances for event and receipt handling.

### 3) Enforce module authority boundary and expand workflow effect origins

1. Enforce structural guardrail before cap/policy:
   - only workflow modules may emit effects,
   - emitted kinds must be in module `effects_emitted` allowlist.
2. Update effect origin handling so orchestration modules can emit needed kinds (`http.request`, `llm.generate`, `workspace.*`, `governance.*`, `introspect.*`) under allowlist + caps + policy.
3. Keep cap and policy enforcement unchanged in authority and order.
4. Keep internal effect handling boundary unchanged (kernel handles internal kinds).

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
3. `effects.rs`: add pre-policy module authority checks (`workflow`-only emission + `effects_emitted` allowlist), then origin scope/cap/policy checks.
4. `world/mod.rs` + `world/snapshot_replay.rs`: persist/restore pending receipt routing identity for replay.
5. `world/mod.rs` + `snapshot.rs`: add persisted workflow instance waiting metadata model.

### `crates/aos-wasm-sdk`

1. `reducers.rs`: remove `effect_used`/single-effect trap.
2. Keep deterministic failure messaging on output-limit violations (now enforced in kernel).

### `spec/defs` and `crates/aos-air-types`

1. Adjust built-in effect origin scopes as needed for workflow emission.
2. Validate `effects_emitted` declarations against known effect kinds.
3. Keep plan model intact in this phase.

### `crates/aos-smoke` / `crates/aos-host/tests`

1. Add no-plan orchestration fixture and replay assertions.

## Acceptance Criteria

1. At least one non-trivial workflow executes fully without any `defplan`.
2. Flow includes multiple effects emitted from module code in one logical workflow.
3. Replay from genesis remains deterministic for the new fixture.
4. Caps/policies still gate every effect intent.
5. Concurrent workflow instances emitting similar effects do not cross-deliver receipts.
6. Receipt routing remains correct after manifest routing changes because delivery does not depend on subscriptions.
7. Workflow instance lifecycle status (`running|waiting|completed|failed`) is persisted and restored on replay.
8. Effects not declared in module `effects_emitted` are rejected before policy evaluation.
9. Pure modules cannot originate effects.
