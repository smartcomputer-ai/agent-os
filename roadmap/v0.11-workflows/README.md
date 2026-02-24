# v0.11 Workflows (Plan Removal)

Roadmap slice for removing plans and moving orchestration into code workflows.

## Execution Documents

1. `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`
2. `roadmap/v0.11-workflows/p2-kernel-plan-runtime-cutover.md`
3. `roadmap/v0.11-workflows/p3-air-manifest-reset.md`
4. `roadmap/v0.11-workflows/p4-governance-observability-cutover.md`
5. `roadmap/v0.11-workflows/p5-fixtures-spec-hardening.md`
6. `roadmap/v0.11-workflows/p6-deterministic-execution-bounds-optional.md` (optional extension)

## Sequence

1. Prove module-based orchestration works without plans.
2. Remove plan execution from kernel.
3. Remove plan model from AIR/manifest/schema/loading.
4. Cut governance/trace/control over to workflow-module vocabulary.
5. Rewrite fixtures/docs and harden replay/snapshot conformance.

Execution is intentionally serial and aggressive: run P1 -> P2 -> P3 -> P4 -> P5, allowing temporary in-between breakage.

Optional extension: run P6 after P1 (or after P5) to harden deterministic compute bounds for workflow-heavy worlds.

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

1. `workflow` modules own orchestration/state-machine logic in code (Rust or other WASM targets).
2. `workflow` modules emit effect intents directly.
3. `pure` modules are side-effect-free compute helpers and never emit effects.
4. Receipts return as events and are fed back into `workflow` modules.
5. Kernel still enforces structural module guardrails, capability gates, and policy gates for every effect.
6. Journal/snapshot/replay stay deterministic.

### 2.2 Responsibility boundaries

1. Workflow modules: deterministic state transitions and orchestration state.
2. Pure modules: deterministic pure computation with no side effects.
3. Kernel/effect manager: capability checks, policy checks, effect queueing, receipt ingestion.
4. Adapters: execute side effects and return receipts.

### 2.3 Non-goals for first cut

1. Static graph analyzability equivalent to plan DAGs.
2. Migration tooling for old plan instances.
3. Compatibility with old plan journal semantics.

### 2.4 Required authority boundary guardrail

After plan removal, authority must not depend on policy configuration alone.

Normative contract:

1. Module kinds are `workflow | pure` in the target model.
2. Only `workflow` modules may emit effects.
3. `workflow` modules must declare `effects_emitted` allowlist on module defs.
4. Kernel must reject any effect not in that module's declared allowlist before capability/policy evaluation.
5. Capability/policy remain mandatory additional gates; they are not the only boundary.

Migration note:

1. While runtime still uses reducer plumbing internally, treat reducer semantics as workflow semantics.
2. End-state AIR/docs/policy vocabulary should not expose `reducer` as an authority class.

### 2.5 Required workflow receipt identity contract

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

### 2.6 Required workflow instance state model

Waiting state must not be an implicit convention inside arbitrary module bytes.

Define a kernel-recognized persisted `WorkflowInstanceState` record (it may be physically stored in the existing cell store, but its structure is normative) with at least:

1. `state_bytes` (canonical encoded module state),
2. `inflight_intents: Map<intent_id -> metadata>`,
3. `status: running|waiting|completed|failed`,
4. `last_processed_event_seq`,
5. `module_version` (optional but strongly recommended for upgrade safety).

`inflight_intents` metadata must include enough data to route and diagnose receipts deterministically:

1. `origin_module_id`,
2. `origin_instance_key`,
3. `effect_kind`,
4. `params_hash` (optional but recommended),
5. `emitted_at_seq`.

Normative behavior:

1. Kernel creates `inflight_intents[intent_id]` when intent enqueue succeeds.
2. Kernel removes it when receipt settles (`ok|denied|faulted`) and updates `status`.
3. Snapshot/replay must restore instance status and inflight intent map byte-identically.
4. Governance/quiescence/observability must read pending-wait state from this model, not heuristics over opaque module bytes.
5. `module_version` (or module hash equivalent) is persisted for diagnostics and future upgrade policies; v0.11 apply safety uses strict quiescence.

### 2.7 Required upgrade semantics for in-flight workflows

Post-plan worlds must define manifest apply behavior for in-flight workflow instances.

Chosen model for v0.11: **strict quiescence**.

Normative apply rule:

1. Block manifest apply unless all workflow instances are terminal (`completed|failed`).
2. Block manifest apply if any workflow instance has non-empty `inflight_intents`.
3. Block manifest apply if effect queue/scheduler still has pending work.
4. Do not clear/abandon in-flight workflow state during apply.

Explicitly not in v0.11:

1. Per-instance version pinning across upgrades.
2. Mandatory workflow state migrations at apply time.

`module_version` in instance state remains useful for diagnostics and future extension to pinning/migration, but strict quiescence is the active safety rule in this roadmap slice.

### 2.8 Required governance/shadow prediction semantics

With Turing-complete workflow modules, governance/shadow must not imply complete static future prediction.

Chosen reporting model for v0.11:

1. Execute shadow deterministically on a forked world with deterministic adapter/receipt handling.
2. Report effects observed during the shadow run so far.
3. Report current in-flight intents.
4. Report declared module `effects_emitted` allowlists.
5. Report state hash deltas and relevant ledger deltas.

Non-goal in v0.11:

1. No promise of full future effect prediction for unexecuted workflow branches or unbounded loops.
2. "Predicted effects" in governance output means "effects observed in bounded shadow execution horizon," not static whole-program enumeration.

### 2.9 Required manifest subscription contract

Post-plan orchestration start wiring must be explicit in manifest and deterministic.

Chosen manifest surface for v0.11:

1. Replace `routing.events` with `routing.subscriptions`.
2. `routing.subscriptions` controls domain-event ingress to workflow modules.
3. Receipt continuation remains manifest-independent and uses origin-instance routing contract.

Each subscription entry must define:

1. `event` (event schema),
2. `module` (target workflow module),
3. `instance_key_derivation` (for example from `event.key`, event field path, or literal),
4. `delivery` (`fanout` or `single`),
5. `on_missing_instance` (`create` or `reject`).

Deterministic semantics:

1. Evaluate matching subscriptions in canonical manifest order.
2. `fanout` delivers to all matches in order.
3. `single` delivers only to the first match in order.
4. If key derivation fails for keyed delivery, fail deterministically.
5. No legacy trigger or implicit startup fallback after P3.

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

### 1.2 Generalize workflow/module receipt handling

Current module receipt plumbing is hardcoded around reducer timer/blob result schemas.

1. Replace hardcoded mapping in `crates/aos-kernel/src/receipts.rs` with a generic effect-receipt event envelope.
2. Add explicit receipt identity fields (`origin_module_id`, `origin_instance_key`, `intent_id`, `effect_kind`, `params_hash`, `status`, `receipt_payload`, `emitted_at_seq`).
3. Route receipts by stored origin identity, not by manifest routing subscriptions.
4. Keep typed timer/blob schemas as optional convenience, not core runtime requirement.
5. Persist pending receipt routing identity in snapshot/journal restore paths so restart/replay preserves delivery.
6. Persist workflow instance wait state (`status`, `inflight_intents`, `last_processed_event_seq`, `module_version`) in kernel-recognized instance records.

### 1.3 Expand allowed effect origins

1. Effects currently enforce origin scopes with plan/reducer distinctions.
2. Update origin model to support workflow-module orchestration cleanly.
3. Ensure required effect kinds (HTTP/LLM/workspace/governance/introspect) are only emitted by `workflow` modules where schema allowlist + cap + policy all allow.
4. Ensure `pure` modules cannot emit effects.

Deliverable: a module can run a multi-step async workflow without plans.

### Phase 2: Remove plan runtime from kernel

1. Delete plan scheduler queue and `Task::Plan` flow (`crates/aos-kernel/src/scheduler.rs`).
2. Remove plan instance lifecycle state from `Kernel` (`crates/aos-kernel/src/world/mod.rs`).
3. Remove plan start/wait/spawn/runtime logic (`crates/aos-kernel/src/world/plan_runtime.rs`, `crates/aos-kernel/src/plan/*`).
4. Remove plan-trigger startup paths from event processing (`crates/aos-kernel/src/world/event_flow.rs`).
5. Remove plan-specific replay identity reconciliation.
6. Keep a module-workflow pending receipt index keyed by `intent_id` with target `(origin_module_id, origin_instance_key)`.
7. Ensure receipt wakeups are manifest-independent and deterministic under concurrency.
8. Replace plan waiters with workflow instance lifecycle state (`running|waiting|completed|failed`) backed by persisted instance records.
9. Replace plan-era apply checks with workflow strict-quiescence checks (instance status + inflight intents + effect queue/scheduler).

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
4. Replace old `triggers` semantics with manifest `routing.subscriptions` as the only orchestration entry wiring.
5. Keep receipt return routing out of manifest schema; receipts route via recorded origin identity only.
6. Replace reducer-era module authority vocabulary with target module kinds (`workflow|pure`) in AIR models/schemas.

Deliverable: AIR no longer contains plans as a first-class definition.

### Phase 4: Governance and shadow updates

1. Remove plan-derived fields from shadow/governance summaries.
2. Remove `PlanStarted`, `PlanResult`, `PlanEnded` journal kinds and replay decode paths.
3. Replace plan-centric debug/trace/control APIs with module/workflow equivalents.
4. Replace plan-era policy/secret semantics (`origin_kind`, `allowed_plans`) with module-oriented semantics.
5. Keep governance propose/shadow/approve/apply mechanics, but report bounded shadow-observed effects plus in-flight intents/allowlists/deltas (no full-future prediction claim).
6. Ensure `ManifestApplyBlockedInFlight` checks reflect new runtime state names.
7. Report pending receipts by `(origin_module_id, origin_instance_key, intent_id)`.
8. Report workflow instance status and `last_processed_event_seq` from kernel-recognized instance state.
9. Expose strict-quiescence block reasons in governance/trace outputs for apply attempts.

Deliverable: governance remains, but no plan concepts remain in reports or checks.

### Phase 5: Tooling/docs/fixtures cleanup

1. Remove `aos plans` command family or replace with module-workflow tooling.
2. Rewrite smoke fixtures and integration tests to workflow modules, including the required upgrade-while-waiting scenario:
   - start workflow instance and emit external effect,
   - snapshot while waiting on receipt,
   - attempt governance apply and assert strict-quiescence block,
   - deliver receipt and assert deterministic continuation,
   - re-apply and assert deterministic success.
3. Ensure this scenario is covered both in `crates/aos-host` end-to-end tests and in `crates/aos-smoke/fixtures/06-safe-upgrade`.
4. Rewrite specs to describe code-workflow orchestration instead of plan DAGs.
5. Update AGENTS/overview docs to remove plan guidance.

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

1. `workflow`
2. `system`
3. `governance`
4. `pure` should be represented only if needed for diagnostics; it should not originate effects.

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
3. Introduce dedicated `workflow` module kind at the AIR/schema layer as early as practical (with temporary runtime aliasing if needed).

This gets working code workflows earlier and de-risks the design before a second ABI expansion while still converging on explicit `workflow|pure` authority semantics.

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
