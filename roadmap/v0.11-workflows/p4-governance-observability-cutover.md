# P4: Governance + Observability Cutover (Post-Plan Runtime)

**Priority**: P2  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/p3-air-manifest-reset.md`

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
   - predicted effects,
   - pending receipts by module instance,
   - relevant ledger deltas.
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

1. Replace `origin_kind: plan|reducer` model with post-plan origin semantics.
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
