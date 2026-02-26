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

Status checklist for this section:
- [x] Inventory all smoke fixtures and integration suites for plan-era dependencies.
- [x] Define fixture-by-fixture rewrite plan and target assertions.
- [ ] Implement all smoke fixture rewrites.
- [ ] Implement all integration suite rewrites.

Smoke fixture rewrite checklist (`crates/aos-smoke/fixtures`):
- [x] `00-counter`: remove legacy manifest keys/vocabulary (`plans`, `triggers`, old routing/policy aliases), keep behavior identical.
- [x] `01-hello-timer`: remove legacy manifest keys/vocabulary, preserve reducer micro-effect flow.
- [x] `02-blob-echo`: remove legacy manifest keys/vocabulary and reducer-origin policy aliases, keep behavior identical.
- [x] `03-fetch-notify`: replace `defplan` assets with workflow-module orchestration; keep reducer as intent/result owner.
- [ ] `04-aggregator`: replace plan orchestration with workflow module; preserve aggregation behavior.
- [ ] `05-chain-comp`: replace charge/reserve/notify/refund plan chain with workflow compensation chain.
- [ ] `06-safe-upgrade`: rewrite `air.v1`/`air.v2` to workflow modules; preserve upgrade-while-waiting semantics.
- [ ] `07-llm-summarizer`: replace summarize plan with workflow module orchestration.
- [ ] `08-retry-backoff`: rebuild retries/timeouts on workflow runtime and receipt/event model.
- [ ] `09-workspaces`: replace workspace plan orchestration with workflow-module-first wiring.
- [ ] `10-trace-failure-classification`: convert trace fixtures from plan modules to workflow modules.
- [ ] `11-plan-runtime-hardening`: replace with workflow-runtime-hardening fixture set and outputs.
- [ ] `20-agent-session`: remove remaining legacy manifest vocabulary; keep behavior unchanged.
- [ ] `21-chat-live`: remove wrapper plan; use workflow subscription/orchestration directly.
- [ ] `22-agent-live`: remove wrapper plan; use workflow subscription/orchestration directly.
- [ ] Update `crates/aos-smoke/fixtures/README.md` to describe only workflow-era fixtures and scenarios.
- [ ] Update `crates/aos-smoke/src` runners to remove plan-era naming/artifacts (`plan-summary`, `PlanEnded` assumptions, etc.).

Integration suite rewrite checklist:
- [ ] `crates/aos-host/tests/world_integration.rs`: replace ignored plan-runtime tests with workflow-runtime equivalents.
- [ ] `crates/aos-host/tests/journal_integration.rs`: migrate journal assertions from plan records to workflow records/state.
- [ ] `crates/aos-host/tests/snapshot_integration.rs`: migrate snapshot/replay assertions to workflow in-flight state semantics.
- [ ] `crates/aos-host/tests/governance_plan_integration.rs`: migrate governance coverage to workflow manifests/subscriptions.
- [ ] `crates/aos-host/tests/governance_integration.rs`: replace plan patch/apply checks with workflow wiring/apply checks.
- [ ] `crates/aos-host/tests/fixtures.rs`: replace plan-oriented fixtures/helpers with workflow-oriented fixtures/helpers.
- [ ] `crates/aos-host/tests/helpers.rs`: replace plan-oriented helpers and names with workflow-oriented helpers.

Required coverage outcomes from rewritten fixtures/suites:
- [ ] external I/O orchestration.
- [ ] retries/timeouts via receipts/events.
- [ ] multi-instance concurrency and isolation.
- [ ] governance apply on workflow manifests.
- [ ] subscription wiring changes while receipts are in flight.
- [ ] subscription wiring changes while continuation frames are in flight (if P7 enabled).
- [ ] upgrade-while-waiting (`pending receipt + snapshot + blocked apply + late receipt + deterministic continuation`).

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
6. If P7 is enabled: deterministic stream-frame routing checks and sequence cursor replay checks for concurrent streaming intents.
7. Workflow instance state machine checks (`running|waiting|completed|failed`) and `last_processed_event_seq` monotonicity checks.
8. Upgrade safety checks for `module_version` behavior with in-flight instances.
9. Structural authority checks: workflow-only effect emission and module `effects_emitted` pre-policy rejection behavior.
10. Strict-quiescence governance tests: apply blocked with in-flight instances/intents and allowed only after terminalization.
11. Shadow semantics tests: reported "predicted effects" must equal effects observed within bounded shadow execution horizon.
12. Killer upgrade scenario tests:
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
