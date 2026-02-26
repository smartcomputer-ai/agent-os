# P4: Agent SDK Workflow-Native Reset

**Stage**: P4  
**Status**: Proposed (2026-02-26)  
**Depends on**: `roadmap/v0.11-workflows` (P1-P4 complete; P5 SDK fixture/doc cleanup still open)

## Goal

Hard-reset `crates/aos-agent-sdk` to the post-plan workflow runtime model.

Breaking changes are intentional:

1. No compatibility bridge for pre-v0.11 SDK contracts.
2. No dual-contract period; update existing `aos.agent/*@1` definitions in place.
3. Replace SDK AIR/runtime surface with workflow-native contracts and fixtures.

## Workflow Model Review (Spec-Constrained)

From `spec/03-air.md`, `spec/05-workflows.md`, and `roadmap/v0.11-workflows/*`, the SDK must align to:

1. Active orchestration authority is `module_kind: "workflow"`; `defplan`/`triggers` are legacy-only.
2. Startup/event ingress wiring is `routing.subscriptions` (not `routing.events`).
3. Workflow modules emit effects directly; continuation comes back via receipt routing keyed by origin identity.
4. Receipt continuation routing is manifest-independent; subscriptions are for domain ingress.
5. Workflow event payloads and receipt payloads are schema-validated and canonicalized before journal/replay.
6. Apply safety is strict quiescence (in-flight workflow work blocks apply).
7. Vocabulary is workflow/system/governance (no plan-era ownership terms in active contracts).

## Current SDK Gaps (Why Refactor Is Required)

Today `aos-agent-sdk` and `20+` smoke fixtures still encode plan-era assumptions:

1. `crates/aos-agent-sdk/air/plans/*` still exports `defplan`-based workspace orchestration.
2. `crates/aos-agent-sdk/air/manifest.air.json` and `crates/aos-smoke/fixtures/20-agent-session/air/manifest.air.json` use `routing.events` and plan-era manifest sections.
3. `21-chat-live` and `22-agent-live` still depend on `plans` + `triggers`.
4. Module defs still use `module_kind: "reducer"` in fixture AIR instead of canonical `workflow`.
5. SDK reducer model is host-driven (`RunStarted`, `StepBoundary`, epoch fences) instead of receipt-driven workflow progression.
6. `RetryOwner::Plan` and similar wording in helpers keeps old responsibility boundaries alive.
7. Workspace sync in SDK is plan-owned, but active runtime model requires workflow or external system orchestration.

## Refactor Strategy (Aggressive, No Bridge)

## 1) Contract Reset In Place (`aos.agent/*@1`)

Replace current session envelope with a workflow-native event family:

1. Introduce `aos.agent/SessionIngress@1` for external commands only.
2. Introduce `aos.agent/SessionWorkflowEvent@1` as reducer ABI event union:
   - ingress command variant,
   - `sys/EffectReceiptEnvelope@1` variant,
   - `sys/EffectReceiptRejected@1` variant,
   - optional `sys/EffectStreamFrame@1` variant (for P7-aligned streaming continuations).
3. Rewrite `SessionState@1` to workflow-native structure:
   - explicit run typestate,
   - explicit pending-intent correlation map keyed by `intent_id`/`params_hash`,
   - remove `session_epoch`/`step_epoch` stale-receipt fence model.
4. Remove host-driven synthetic step orchestration events from core contract:
   - drop `RunStarted`, `StepBoundary`, and receipt epoch fields from public ingress model.
5. Rename or remove plan-era terms (`RetryOwner::Plan`, plan-specific error names).

## 2) Reducer/Helper Rewrite for Receipt-Driven Flow

Rewrite SDK reducer helpers to orchestrate via effects + continuations:

1. `RunRequested` ingress transitions to active run and emits `llm.generate` intent.
2. Receipt variants drive run progression deterministically (tool calls, follow-up turns, completion/failure).
3. Tool batch state is keyed by workflow effect identity, not synthetic step epochs.
4. `HostCommand` remains ingress-only control (pause/resume/cancel/steer/follow-up).
5. Provider/model validation remains deterministic preflight in reducer helper path.

Implementation target files:

1. `crates/aos-agent-sdk/src/contracts/*`
2. `crates/aos-agent-sdk/src/helpers/*`
3. `crates/aos-agent-sdk/src/bin/session_workflow.rs`

## 3) AIR Asset Reset (SDK-Owned)

Rewrite SDK AIR to active post-plan contract:

1. Remove `crates/aos-agent-sdk/air/plans/` and plan references from README/docs.
2. Update `crates/aos-agent-sdk/air/manifest.air.json`:
   - remove `plans`/`triggers`,
   - switch to `routing.subscriptions`,
   - include receipt envelope schemas used by reducer event union.
3. Update `crates/aos-agent-sdk/air/module.air.json`:
   - canonical `module_kind: "workflow"`,
   - explicit `effects_emitted` allowlist for emitted effects.
4. Publish only workflow-native reusable assets; no plan wrapper exports.

## 4) Workspace Sync Contract Cutover

Remove plan-owned workspace sync from SDK runtime path.

Chosen in-place approach:

1. Workspace snapshot materialization is system/host-orchestrated.
2. SDK reducer consumes typed ingress updates (`WorkspaceSnapshotReady@1`-style command payload).
3. Keep JSON validation helpers for prompt pack/tool catalog payloads in SDK.
4. Delete SDK plan wrappers (`core_*workspace_sync` and fixture wrapper plans).

This keeps SDK aligned with planless runtime while avoiding new hidden orchestration DSL inside SDK AIR.

## 5) Smoke Fixture Cutover (`20*`)

### 5.1 `20-agent-session`

1. Keep `aos.agent/*@1` names and update schema bodies/modules in place.
2. Use `routing.subscriptions` + `module_kind: workflow`.
3. Update conformance flow to receipt-driven lifecycle (no synthetic step-boundary loop).
4. Keep deterministic checks for:
   - provider/model rejection without state mutation,
   - cancellation and stale continuation handling,
   - run-config immutability per run,
   - replay parity.

### 5.2 `21-chat-live`

1. Delete `live_chat_plan.air.json`, trigger wiring, and plan-dependent secret allowlists.
2. Rewrite reducer as direct workflow orchestration:
   - emit `llm.generate`,
   - handle `EffectReceiptEnvelope` continuations,
   - emit `RunResult` domain event.
3. Keep live smoke semantics (tool-call roundtrip + follow-up turn + replay check).

### 5.3 `22-agent-live`

1. Delete `session_workspace_sync_wrapper.air.json` and imported SDK plan dependencies.
2. Move to workflow-native session reducer contracts (still `@1`) and `routing.subscriptions`.
3. Workspace sync is host/system pre-step + ingress update event into session workflow.
4. Preserve live agent assertions (multi-step tool traversal, follow-up answer, replay).

## 6) Package/API Shape Cleanup

1. Replace current public re-exports with explicit workflow-native modules; no parallel versioned API surface.
2. Remove dead binaries/assets that exist only for plan-era scaffolding.
3. Keep crate `no_std` where feasible; isolate any std-bound testing helpers in test-only paths.

## Deliverables

1. Workflow-native `aos-agent-sdk` contracts/helpers (updated in place under `aos.agent/*@1`).
2. SDK AIR assets with no active plan/triggers surface.
3. `20/21/22` fixtures updated to workflow-native manifests/modules/events.
4. Smoke runners updated for new ingress/continuation model.
5. Updated SDK docs (`crates/aos-agent-sdk/air/README.md`) and roadmap references.

## Acceptance Criteria

1. No `defplan` or manifest `triggers` remain in `crates/aos-agent-sdk/air` or fixtures `20/21/22`.
2. All three fixtures use `routing.subscriptions` and canonical `module_kind: "workflow"`.
3. Session workflow progression is receipt-driven; synthetic step-boundary control events are removed from ingress contract.
4. `20-agent-session` deterministic conformance and replay parity pass.
5. `21-chat-live` and `22-agent-live` complete end-to-end with workflow-native wiring.
6. Quiescence/replay behavior remains deterministic with no cross-delivery between concurrent session instances.

## Execution Order

1. Implement in-place `aos.agent/*@1` contract rewrite + reducer/helper rewrite in SDK crate.
2. Reset SDK AIR assets (remove plans/triggers, switch routing/module kinds).
3. Migrate `20-agent-session` deterministic fixture and runner first.
4. Migrate `21-chat-live`, then `22-agent-live`.
5. Run full smoke lanes (`all`, `all-agent`, `chat-live`, `agent-live`) and replay checks.
