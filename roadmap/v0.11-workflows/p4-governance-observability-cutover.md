# P4: Governance + Observability Cutover (Post-Plan Runtime)

**Priority**: P2  
**Status**: Completed  
**Depends on**: `roadmap/v0.11-workflows/p3-air-manifest-reset.md`

## Implementation Status

### Scope
- [x] Scope 1.1: Removed shadow summary `plan_results` / `pending_plan_receipts` fields from active governance receipts and summaries.
- [x] Scope 1.2: Shadow/governance receipts now report bounded workflow-era data (`pending_workflow_receipts`, `workflow_instances`, `module_effect_allowlists`, `ledger_deltas`).
- [x] Scope 2.1: Journal tail/trace surfaces no longer depend on `plan_*` labels; legacy record kinds are surfaced as `legacy_plan_*` when present.
- [x] Scope 2.2: Trace/diagnose waiting and failure classification now key off workflow instance state + receipt outcomes.
- [x] Scope 3.1: Removed control/CLI `plan-summary`; replaced with `trace-summary`.
- [x] Scope 3.2: Updated `trace`, `trace-diagnose`, and `trace-summary` outputs to workflow-centric wait/continuation fields.
- [x] Scope 4.1: Policy origin model aligned to post-plan semantics (`workflow|system|governance`) in active model + schema.
- [x] Scope 4.2: Secret policy checks are cap-oriented and no longer rely on plan identity.

### Work Items by Crate
- [x] `crates/aos-kernel`: shadow summary/governance receipt structures updated; policy origin matching aligned to workflow semantics.
- [x] `crates/aos-host`: control command cutover (`trace-summary`), trace live-wait/failure diagnosis cutover, manifest loader made tolerant of legacy `defplan` assets during fixture transition.
- [x] `crates/aos-cli`: trace/journal/summary command surfaces updated to workflow terminology; import lock hashing ignores legacy `defplan` nodes.
- [x] `crates/aos-air-types`: `OriginKind` and `defpolicy` schema aligned to post-plan origins; validation/tests updated.
- [x] `spec/*`: control/shadow/policy references updated for workflow-era governance/observability fields.

### Validation
- [x] `cargo check -p aos-kernel -p aos-host -p aos-cli`
- [x] `cargo check --tests -p aos-kernel -p aos-host -p aos-cli -p aos-air-types`
- [x] `cargo test -p aos-air-types -p aos-kernel -p aos-host -p aos-cli`

### Acceptance Criteria
- [x] AC1: Governance/shadow outputs contain no active `plan_results`/`pending_plan_receipts` fields.
- [x] AC2: Journal tail and trace tooling function without relying on `plan_*` output kinds.
- [x] AC3: Control/CLI no longer expose `plan-summary`.
- [x] AC4: Policy/secret enforcement does not require `plan` origin identity.
- [x] AC5: Observability exposes deterministic continuation routing identity (`intent_hash`, origin module, instance key, effect kind, emitted seq).
- [x] AC6: Observability exposes workflow instance lifecycle status (`running|waiting|completed|failed`) and waiting counters.
- [x] AC7: Policy/trace/control surfaces use workflow-era authority vocabulary.
- [x] AC8: Apply-block diagnostics report strict-quiescence blockers via workflow/inflight/queue counters.
- [x] AC9: Shadow/governance reporting is explicitly bounded to observed execution horizon.

## Goal

Remove plan-centric governance and diagnostics vocabulary, replacing it with module-workflow-centric reporting.

This phase ensures shadow, governance effects, trace, and control APIs describe the new architecture instead of legacy plan concepts.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

## Hard-Break Assumptions

1. Existing trace consumers and control clients may break.
2. Journal-tail interpretation can change.
3. Old plan-specific summaries are removed, not translated.

## Scope

### 1) Governance/shadow summary cleanup

1. Remove `plan_results` and `pending_plan_receipts` structures from shadow summaries.
2. Replace with module workflow execution summaries:
   - effects observed during bounded shadow execution,
   - current in-flight intents,
   - declared module `effects_emitted` allowlists,
   - pending receipts by `(origin_module_id, origin_instance_key, intent_id)`,
   - stream cursors/last-seq by `(origin_module_id, origin_instance_key, intent_id)` when P7 is enabled,
   - workflow instance status (`running|waiting|completed|failed`),
   - `last_processed_event_seq`,
   - strict-quiescence apply-block reasons when present,
   - state hash deltas and relevant ledger deltas.
3. Keep propose/shadow/approve/apply lifecycle unchanged.

### 2) Journal model cleanup for plan records

1. Remove `PlanStarted`, `PlanResult`, `PlanEnded` record kinds.
2. Update decoding and tail-scan fallback behavior accordingly.
3. Update daemon journal kind naming tables.

### 3) Trace and control API cleanup

1. Remove `plan-summary` control command.
2. Replace trace failure classification currently keyed on `plan_ended` with module workflow failure signals.
3. Update `trace`, `trace-diagnose`, and `trace-summary` CLI surfaces.

### 4) Policy and secret semantics cleanup

1. Replace `origin_kind: plan|reducer` model with post-plan origin semantics (`workflow|system|governance`; `pure` is non-effect origin).
2. Replace `allowed_plans` secret policy with module-oriented policy fields.

## Out of Scope

1. Smoke/tutorial/spec full rewrite.
2. Additional runtime features.

## Work Items by Crate

### `crates/aos-kernel`

1. `shadow/*`, `governance_effects.rs`, `journal/mod.rs`, replay decode paths.
2. Policy model alignment for post-plan origins.
3. Secret policy type usage alignment.

### `crates/aos-host`

1. `trace.rs`, `control.rs`, daemon mode command handling.
2. HTTP debug endpoints and schema output structures.

### `crates/aos-cli`

1. Remove/replace plan summary commands and plan-centric trace assumptions.

## Acceptance Criteria

1. Governance/shadow outputs contain no plan fields.
2. Journal tail and trace tools work without plan record kinds.
3. Control/CLI no longer expose `plan-summary` or plan-state diagnostics.
4. Policy and secret checks no longer depend on `plan` origin identity.
5. Observability surfaces expose deterministic continuation routing identity for debugging (receipts, and stream frames when P7 is enabled).
6. Observability surfaces expose workflow instance waiting/running/completed/failed status from persisted state.
7. Policy/trace/control surfaces no longer require or expose `reducer` as an authority class in post-plan mode.
8. Apply-block diagnostics clearly report strict-quiescence failures (in-flight instances/intents/queues).
9. Shadow/governance surfaces do not claim full future effect prediction beyond bounded shadow execution.
