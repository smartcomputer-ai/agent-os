# P5: Fixtures, Specs, and Hardening (Finish the Reset)

**Priority**: P2  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/p4-governance-observability-cutover.md`

## Goal

Finalize the plans-to-workflows reset by replacing old fixtures/docs and proving replay-or-die invariants on the new architecture.

This phase is the completion pass that makes the repository internally consistent and operationally usable.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

## Hard-Break Assumptions

1. Old smoke fixtures can be deleted.
2. Old tutorials and examples can be rewritten from scratch.
3. Conformance gates are defined for the new model only.

## Scope

### 1) Rewrite smoke fixtures and integration suites

1. Remove plan-based fixture ladders.
2. Add workflow-module-first fixtures for:
   - external I/O orchestration,
   - retries/timeouts via receipts/events,
   - multi-instance concurrency and isolation,
   - governance apply on workflow manifests,
   - manifest subscription wiring changes while receipts are in flight,
   - upgrade-while-waiting (pending receipt + snapshot + apply attempt + late receipt).
3. Update host integration tests accordingly.

### 2) Spec/doc rewrite

1. Update `spec/03-air.md`, `spec/05-workflows.md`, and architecture docs to remove plan model claims.
2. Update `AGENTS.md` architecture summary and boundaries.
3. Ensure schema docs match implementation.

### 3) Hardening and quality gates

1. Replay-or-die gates for workflow fixtures.
2. Snapshot create/load/replay gates with in-flight module receipt waits.
3. Deterministic tail/trace assertions on workflow runs.
4. Performance sanity checks for many concurrent module instances.
5. Deterministic receipt routing checks for concurrent identical effect emissions from distinct instances.
6. Workflow instance state machine checks (`running|waiting|completed|failed`) and `last_processed_event_seq` monotonicity checks.
7. Upgrade safety checks for `module_version` behavior with in-flight instances.
8. Structural authority checks: workflow-only effect emission and module `effects_emitted` pre-policy rejection behavior.
9. Strict-quiescence governance tests: apply blocked with in-flight instances/intents and allowed only after terminalization.
10. Shadow semantics tests: reported "predicted effects" must equal effects observed within bounded shadow execution horizon.
11. Killer upgrade scenario tests:
    - start workflow instance and emit external effect,
    - snapshot while waiting on receipt,
    - attempt governance apply and assert strict-quiescence block,
    - deliver receipt and assert deterministic continuation,
    - re-apply and assert deterministic success.

### 4) Dead code and roadmap cleanup

1. Remove archived plan runtime code paths that survived earlier phases.
2. Update roadmap indexes/READMEs to mark completion and superseded docs.

## Out of Scope

1. New product features unrelated to plan removal.
2. Distributed scaling redesign.

## Work Items by Crate

### `crates/aos-smoke`

1. Replace plan fixtures with module-workflow fixtures.
2. Keep fixtures small, deterministic, and replay-testable.
3. Extend `fixtures/06-safe-upgrade` to cover upgrade-while-waiting end-to-end, including snapshot + blocked apply + post-receipt apply.

### `crates/aos-host/tests` and `crates/aos-kernel/tests`

1. Replace plan-era assumptions with workflow-instance assertions.
2. Add regression coverage for replay/snapshot/tail behavior.
3. Add an end-to-end host test for upgrade-while-waiting with strict-quiescence apply blocking and deterministic post-receipt continuation.

### `spec/*` and repo docs

1. Update workflow architecture descriptions.
2. Remove references to plan interpreter behavior and reducer-as-authority semantics.

## Acceptance Criteria

1. Smoke and integration suites pass without any plan fixture dependencies.
2. Specs/docs align with code (no plan runtime references in active architecture sections).
3. Replay-or-die checks pass on new workflow fixtures.
4. No critical dead code remains for plan execution paths.
5. Restart/replay preserves receipt delivery to the same module instance identity.
6. Restart/replay preserves workflow instance status and inflight intent map exactly.
7. Fixtures/docs validate `workflow|pure` authority model and module allowlist guardrails.
8. Fixture suite proves strict-quiescence manifest apply semantics for post-plan worlds.
9. Fixture suite proves honest shadow observability semantics (no full-future prediction claim).
10. Fixture suite includes upgrade-while-waiting with snapshot and verifies deterministic behavior under the selected upgrade rule.
