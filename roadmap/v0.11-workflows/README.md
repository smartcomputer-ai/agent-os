# v0.11 Workflows (Plan Removal)

Roadmap slice for removing plans and moving orchestration into code workflows.

## Execution Documents

1. `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`
2. `roadmap/v0.11-workflows/p2-kernel-plan-runtime-cutover.md`
3. `roadmap/v0.11-workflows/p3-air-manifest-reset.md`
4. `roadmap/v0.11-workflows/p4-governance-observability-cutover.md`
5. `roadmap/v0.11-workflows/p5-fixtures-spec-hardening.md`

## Sequence

1. Prove module-based orchestration works without plans.
2. Remove plan execution from kernel.
3. Remove plan model from AIR/manifest/schema/loading.
4. Cut governance/trace/control over to workflow-module vocabulary.
5. Rewrite fixtures/docs and harden replay/snapshot conformance.

Execution is intentionally serial and aggressive: run P1 -> P2 -> P3 -> P4 -> P5, allowing temporary in-between breakage.

## Migration Contract

This slice assumes intentional breaking changes:

1. No backward compatibility with old plan worlds/manifests is required.
2. AIR semantics are reset to module-code workflows as orchestration authority.
3. Determinism, caps/policy, receipts, and replay invariants remain mandatory.

---

# Spec: Replace Plans With Code Workflows

## 0. Summary

We will remove plans and move orchestration into deterministic WASM modules.

This is a deliberate shift from a declarative orchestration DSL (`defplan` + DAG interpreter) to code-defined orchestration state machines. In this repo, plan authoring and plan/reducer lockstep changes have become the dominant friction. Complex plans are already behaving like a programming language, but with worse readability and weaker tooling than Rust/WASM code.

Why we are doing this:

1. Reduce authoring complexity.
2. Eliminate dual maintenance of reducer logic plus plan logic.
3. Keep orchestration in one place with real language tooling.
4. Preserve the things that matter most: deterministic replay, policy/cap gates, receipts, journaling, snapshots.

### Breaking-change stance

This migration is intentionally **not** backward compatible.

1. Old worlds, manifests, plans, and traces can break.
2. We can delete transitional code aggressively instead of preserving old formats.
3. We can treat this as an AIR-next semantic reset, even if we temporarily keep `air_version: "1"` during bring-up.

In short: this is effectively a new control-plane model, implemented in the current codebase.

---

## 1. Current Code Reality (What Makes This Non-trivial)

Plan logic is not only in one interpreter file. It is spread across:

1. Plan runtime and scheduler (`crates/aos-kernel/src/plan/*`, `crates/aos-kernel/src/world/plan_runtime.rs`, `crates/aos-kernel/src/scheduler.rs`).
2. Manifest assembly and capability wiring (`crates/aos-kernel/src/world/manifest_runtime.rs`).
3. Event flow and receipt wakeups (`crates/aos-kernel/src/world/event_flow.rs`, `crates/aos-kernel/src/world/plan_runtime.rs`).
4. Journal and snapshot model (`crates/aos-kernel/src/journal/mod.rs`, `crates/aos-kernel/src/snapshot.rs`, `crates/aos-kernel/src/world/snapshot_replay.rs`).
5. Governance and shadow summaries (`crates/aos-kernel/src/world/governance_runtime.rs`, `crates/aos-kernel/src/shadow/*`, `crates/aos-kernel/src/governance_effects.rs`).
6. AIR model + validation + schemas (`crates/aos-air-types/src/model.rs`, `crates/aos-air-types/src/validate.rs`, `spec/schemas/defplan.schema.json`, `spec/schemas/manifest.schema.json`).
7. Host/CLI/trace commands and smoke fixtures (`crates/aos-host`, `crates/aos-cli`, `crates/aos-smoke`).

Implication: deleting plans is feasible, but it is a cross-cutting redesign, not a single runtime swap.

---

## 2. Target Architecture

### 2.1 Core model

1. Modules own orchestration logic in code (Rust or other WASM targets).
2. Modules emit effect intents directly.
3. Receipts return as events and are fed back into modules.
4. Kernel still enforces capability and policy gates for every effect.
5. Journal/snapshot/replay stay deterministic.

### 2.2 Responsibility boundaries

1. Modules: deterministic state transitions and orchestration state.
2. Kernel/effect manager: capability checks, policy checks, effect queueing, receipt ingestion.
3. Adapters: execute side effects and return receipts.

### 2.3 Non-goals for first cut

1. Static graph analyzability equivalent to plan DAGs.
2. Migration tooling for old plan instances.
3. Compatibility with old plan journal semantics.

### 2.4 Required workflow receipt identity contract

Plans previously owned receipt wakeups. After plan removal, receipt routing identity must be explicit and deterministic.

Every settled effect must produce a generic receipt envelope with at least:

1. `origin_module_id` (module content hash/module key at emit time),
2. `origin_instance_key` (cell key/instance routing key),
3. `intent_id` (deterministic unique identifier for this emitted intent),
4. `effect_kind`,
5. `params_hash` (optional but recommended),
6. `receipt_payload` (validated against effect receipt schema),
7. `status` (`ok|denied|faulted`),
8. `emitted_at_seq` (journal seq/logical clock for diagnostics).

Deterministic routing rule:

1. The kernel routes each receipt to `(origin_module_id, origin_instance_key)` using pending intent state.
2. Receipt routing does not consult manifest event subscriptions.
3. Manifest routing remains for domain-event ingress only.

Deterministic `intent_id` rule:

1. `intent_id` may reuse the existing `intent_hash` name, but its preimage must include origin instance identity (`origin_module_id`, `origin_instance_key`) in addition to effect kind/cap/params/idempotency key.
2. This prevents ambiguous wakeups when multiple instances emit structurally similar effects concurrently.
3. The same preimage must be used on replay.

---

## 3. Pragmatic Migration Strategy

Use a two-track approach:

1. Get code workflows working quickly by reusing the existing reducer/module execution path.
2. Then delete plan-specific control-plane structures.

This avoids introducing a brand new workflow VM path before validating semantics.

---

## 4. Phase Plan

### Phase 0: Hard-reset policy and scope

1. Declare plan removal as a hard break.
2. Stop requiring replay compatibility with historical journals.
3. Allow fixture and integration test rewrites.
4. Freeze new plan features immediately.

Deliverable: a documented migration contract saying old plan worlds are unsupported.

### Phase 1: Make modules capable of orchestration

### 1.1 Remove single-effect constraints

1. Remove `ensure_single_effect` gate in `crates/aos-kernel/src/world/event_flow.rs`.
2. Remove `effect_used` trap in `crates/aos-wasm-sdk/src/reducers.rs`.
3. Keep deterministic output limits via explicit per-tick guardrails in kernel.

### 1.2 Generalize reducer/module receipt handling

Current reducer receipt plumbing is hardcoded around timer/blob result schemas.

1. Replace hardcoded mapping in `crates/aos-kernel/src/receipts.rs` with a generic effect-receipt event envelope.
2. Add explicit receipt identity fields (`origin_module_id`, `origin_instance_key`, `intent_id`, `effect_kind`, `params_hash`, `status`, `receipt_payload`, `emitted_at_seq`).
3. Route receipts by stored origin identity, not by manifest routing subscriptions.
4. Keep typed timer/blob schemas as optional convenience, not core runtime requirement.
5. Persist pending receipt routing identity in snapshot/journal restore paths so restart/replay preserves delivery.

### 1.3 Expand allowed effect origins

1. Effects currently enforce origin scopes with plan/reducer distinctions.
2. Update origin model to support module-origin orchestration cleanly.
3. Ensure all required effect kinds (HTTP/LLM/workspace/governance/introspect) can be emitted by orchestrating modules where policy allows.

Deliverable: a module can run a multi-step async workflow without plans.

### Phase 2: Remove plan runtime from kernel

1. Delete plan scheduler queue and `Task::Plan` flow (`crates/aos-kernel/src/scheduler.rs`).
2. Remove plan instance lifecycle state from `Kernel` (`crates/aos-kernel/src/world/mod.rs`).
3. Remove plan start/wait/spawn/runtime logic (`crates/aos-kernel/src/world/plan_runtime.rs`, `crates/aos-kernel/src/plan/*`).
4. Remove plan-trigger startup paths from event processing (`crates/aos-kernel/src/world/event_flow.rs`).
5. Remove plan-specific replay identity reconciliation.
6. Keep a module-workflow pending receipt index keyed by `intent_id` with target `(origin_module_id, origin_instance_key)`.
7. Ensure receipt wakeups are manifest-independent and deterministic under concurrency.

Deliverable: kernel has no plan interpreter or plan instance state.

### Phase 3: AIR and manifest model reset

1. Remove `DefPlan`, plan steps, and trigger-to-plan bindings from `crates/aos-air-types/src/model.rs` and `crates/aos-air-types/src/validate.rs`.
2. Remove/replace schemas:
   - `spec/schemas/defplan.schema.json`
   - `manifest.plans` references in `spec/schemas/manifest.schema.json`
   - plan-related patch operations and validators
3. Update manifest loading and storage paths in:
   - `crates/aos-store/src/manifest.rs`
   - `crates/aos-kernel/src/manifest.rs`
   - `crates/aos-host/src/manifest_loader.rs`
   - `crates/aos-host/src/world_io.rs`
4. Replace old `triggers` semantics with module/event subscriptions (or expand routing rules) as the only orchestration entry wiring.
5. Keep receipt return routing out of manifest schema; receipts route via recorded origin identity only.

Deliverable: AIR no longer contains plans as a first-class definition.

### Phase 4: Governance and shadow updates

1. Remove plan-derived fields from shadow/governance summaries.
2. Remove `PlanStarted`, `PlanResult`, `PlanEnded` journal kinds and replay decode paths.
3. Replace plan-centric debug/trace/control APIs with module/workflow equivalents.
4. Replace plan-era policy/secret semantics (`origin_kind`, `allowed_plans`) with module-oriented semantics.
5. Keep governance propose/shadow/approve/apply mechanics, but report module/effect-level predictions.
6. Ensure `ManifestApplyBlockedInFlight` checks reflect new runtime state names.
7. Report pending receipts by `(origin_module_id, origin_instance_key, intent_id)`.

Deliverable: governance remains, but no plan concepts remain in reports or checks.

### Phase 5: Tooling/docs/fixtures cleanup

1. Remove `aos plans` command family or replace with module-workflow tooling.
2. Rewrite smoke fixtures and integration tests to workflow modules.
3. Rewrite specs to describe code-workflow orchestration instead of plan DAGs.
4. Update AGENTS/overview docs to remove plan guidance.

Deliverable: repository UX and docs match the new architecture.

---

## 5. AIR / Versioning Position

Given current maturity, we prioritize velocity over compatibility.

1. We can keep `air_version: "1"` during migration if that reduces churn.
2. Semantically, this is AIR-next behavior and should be treated as such in docs.
3. If keeping version `1` causes confusion, we can bump later as a cleanup step, not a blocker.

The key point: version label is secondary; architecture reset is primary.

---

## 6. Policy and Security Model Adjustments

### 6.1 Origin model

Current policy matching uses `plan` vs `reducer`. Replace with runtime-accurate origins, for example:

1. `module` (or `reducer`/`workflow` if we keep both labels)
2. `system`
3. `governance`

### 6.2 Secret policy model

Current secret policy fields include `allowed_plans`. Replace with module-oriented controls such as:

1. `allowed_modules`
2. `allowed_caps`

### 6.3 Effect origin scopes

Update effect definitions so origin scope semantics match module-based orchestration (remove plan-only assumptions).

---

## 7. Practical Implementation Choice: Reuse Existing Module ABI First

To minimize lift, first implement workflows on top of the existing module/reducer ABI path.

1. Do not introduce a separate workflow VM ABI in phase 1.
2. Use current event->module dispatch and persisted module state cells.
3. Add a dedicated workflow module kind only if needed later for clarity.

This gets working code workflows earlier and de-risks the design before a second ABI expansion.

---

## 8. What We Keep Intact

1. Deterministic event-sourced stepping.
2. Effect manager capability and policy enforcement.
3. Adapter boundary and receipts, including deterministic origin-instance receipt routing.
4. Journal/snapshot/replay invariants.
5. Governance apply/shadow lifecycle (with new summary model).

These are the core AOS properties worth preserving while plans are removed.

---

## 9. Rough Lift Estimate

1. Workflow-first enablement without full schema purge: moderate lift.
2. Full hard deletion of plans across kernel/AIR/host/CLI/tests/specs: large but straightforward mechanical lift.

Given the explicit no-compat stance, this is mostly an execution and sequencing problem, not a research problem.

---

## 10. Exit Criteria

We consider plan removal complete when all are true:

1. No `defplan` in AIR model, schemas, or loader paths.
2. No plan runtime code or journal kinds in kernel.
3. No plan-specific APIs in host/control/trace/CLI.
4. All smoke/integration fixtures run without plans.
5. Docs/specs describe module-code workflows as the only orchestration model.
