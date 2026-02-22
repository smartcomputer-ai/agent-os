# P3: Plan Runtime Hardening (Factory-Ready, No New AIR Ops)

**Priority**: P3  
**Status**: In progress (single-fixture + tooling implementation on 2026-02-22)  
**Depends on**: AIR v1 current plan semantics, P1 import reuse  
**May run alongside**: P2 composition work

## Context

`factory.md` requires headless, parallel, long-running agent workflows with strong replay confidence and low operator toil. `infra.md` assumes worlds are movable and recover from queue/journal replay without losing in-flight orchestration.

We can lift only the v1.1 ideas that are needed for this and implementable now, without adding new AIR step kinds.

## Decision Summary

Lift forward now:

1. Standardize timeout/race patterns using existing `emit_effect` + `await_receipt` + guards (`timer.set`, explicit decision vars).
2. Standardize approval patterns using explicit `approval.request` effects and receipts.
3. Enforce correlation-safe request/response patterns using existing `triggers[].correlate_by` + `await_event.where`.
4. Add conformance gates (replay, cross-talk, crash/resume) for these patterns.
5. Add lightweight journal-derived operational summaries (no new kernel event types, no new UI requirement).

Defer:

1. `await_any`, `cancel_plan`, `call_pure`, native await deadlines.
2. Per-plan concurrency quotas and policy rate counters.
3. New plan language constructs beyond P2.

## Why This Is Sufficient for v0.10.1

This gives the factory initiative the required reliability primitives (timeouts, approvals, correlation hygiene, replay confidence) while keeping AIR and kernel complexity bounded.

Parallelism remains a plan concern; reducers stay focused on domain state transitions.

## Scope

### In scope

1. Pattern docs + fixtures for:
   - timeout/deadline race,
   - approval gate,
   - correlated cross-world request/response.
2. Tooling lints/checks (validator-adjacent, no new AIR grammar):
   - warn when a correlated trigger path lacks correlation predicate use in waits,
   - warn on long waits without explicit timeout branch,
   - warn when non-read effects omit explicit idempotency keys.
3. Determinism and recovery gates:
   - replay parity for pattern fixtures,
   - crash/resume coverage for plans parked on `await_event` / `await_receipt`,
   - duplicate ingress/receipt retry scenarios to verify idempotent behavior.
4. Minimal ops visibility:
   - journal-derived per-plan run summaries (success/error counts, timeout branch counts, effect allow/deny counts),
   - correlation-id traceability for request/response flows.

### Out of scope

1. New AIR step types or polymorphism.
2. Rich plan inspector UI.
3. Global scheduling/admission-control features.
4. Budget/rate-limit policy extensions.

## Milestones

### C1: Pattern Pack + Examples

**Status**: Complete (single high-value fixture, 2026-02-22)

1. [x] Add reusable fixture docs and scenario guidance.
2. [x] Add one consolidated fixture that cuts across approval gating, correlated waits, subplan composition, and idempotent worker effects.
3. [x] Keep fixture consumable from standard AIR layout (no special export-only folder assumptions).

Implemented:

1. `crates/aos-smoke/fixtures/11-plan-runtime-hardening/` (single scenario).
2. `crates/aos-smoke/src/plan_runtime_hardening.rs` runner + artifact emission.
3. `crates/aos-smoke/src/main.rs` + `crates/aos-smoke/tests/cli.rs` wiring (`plan-runtime-hardening`).

### C2: Conformance Gates

**Status**: Mostly complete for selected high-value gates (2026-02-22)

1. [x] Add single-fixture conformance for correlated instance isolation + crash/resume with pending subplan receipts.
2. [x] Add concurrent-instance cross-talk test (same schema, different correlation ids).
3. [x] Add crash/resume test where waits survive restart and complete correctly.
4. [ ] Strict replay parity for this subplan-heavy fixture shape (deferred; currently not a release gate for this slice).

Implemented in this pass:

1. `correlated_await_event_prevents_cross_talk_between_instances` (`crates/aos-host/tests/world_integration.rs`)
2. `subplan_receipt_wait_survives_restart_and_resumes_parent` (`crates/aos-host/tests/world_integration.rs`)
3. Fixture assertions in `crates/aos-smoke/src/plan_runtime_hardening.rs` for:
   - approval cross-talk isolation,
   - restart/recovery of pending worker receipts,
   - idempotent worker dispatch (`idempotency_key`).

### C3: Lightweight Operational Summaries

**Status**: Complete (2026-02-22)

1. [x] Add a CLI/test helper that produces plan-flow summaries from journal records.
2. [x] Surface failures by category (`policy deny`, `invariant_violation`, timeout path, adapter error).
3. [x] Emit CI artifact(s) for at least one factory-like fixture run.

Implemented in this pass:

1. `aos_host::trace::plan_run_summary(...)` with per-plan/totals aggregation from journal records.
2. Category aggregation for policy/cap decisions, invariant failures, timeout signals, and adapter errors.
3. Correlation-key event indexing (`correlation_events`) in summary output.
4. Control command `plan-summary` (`crates/aos-host/src/control.rs`, `crates/aos-host/src/modes/daemon.rs`).
5. CLI `aos trace-summary` (`crates/aos-cli/src/commands/trace_summary.rs`).
6. Fixture artifact output at `crates/aos-smoke/fixtures/11-plan-runtime-hardening/.aos/artifacts/plan-summary.json`.

### P3 Lints/Checks

**Status**: Complete (2026-02-22)

1. [x] Warn when correlated trigger paths use `await_event` without correlation predicates.
2. [x] Warn on wait-heavy plans without explicit timeout branch signals.
3. [x] Warn when non-read effects omit explicit `idempotency_key`.

Implemented in:

1. `crates/aos-cli/src/commands/plans.rs` (`aos plans check` lint pass).
## Conditional Follow-Through After P2

If P2 subplan composition lands in this initiative, apply the same C1-C3 gates to subplan flows (parent/child paths and fan-out barriers) without introducing additional language features.

## Definition of Done

1. Three canonical patterns (timeout, approval, correlated response) are documented and fixture-backed.
2. Replay + crash/resume + cross-talk tests pass for those fixtures.
3. Lightweight journal-derived summaries are available in CI for at least one migrated flow.
4. No additional AIR plan ops are required beyond current spec (plus any already approved in P2).
