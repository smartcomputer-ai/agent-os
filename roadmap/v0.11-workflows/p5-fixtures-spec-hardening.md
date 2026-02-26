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
- [x] Defer Agent SDK fixtures (`20/21/22`) until Agent SDK refresh lands.
- [ ] Implement all smoke fixture rewrites.
- [x] Implement all integration suite rewrites.

Smoke fixture rewrite checklist (`crates/aos-smoke/fixtures`):
- [x] `00-counter`: remove legacy manifest keys/vocabulary (`plans`, `triggers`, old routing/policy aliases), keep behavior identical.
- [x] `01-hello-timer`: remove legacy manifest keys/vocabulary, preserve reducer micro-effect flow.
- [x] `02-blob-echo`: remove legacy manifest keys/vocabulary and reducer-origin policy aliases, keep behavior identical.
- [x] `03-fetch-notify`: replace `defplan` assets with workflow-module orchestration; keep reducer as intent/result owner.
- [x] `04-aggregator`: replace plan orchestration with workflow module; preserve aggregation behavior.
- [x] `05-chain-comp`: replace charge/reserve/notify/refund plan chain with workflow compensation chain.
- [x] `06-safe-upgrade`: rewrite `air.v1`/`air.v2` to workflow modules; preserve upgrade-while-waiting semantics.
- [x] `07-llm-summarizer`: replace summarize plan with workflow module orchestration.
- [x] `08-retry-backoff`: rebuild retries/timeouts on workflow runtime and receipt/event model.
- [x] `09-workspaces`: replace workspace plan orchestration with workflow-module-first wiring.
- [x] `10-trace-failure-classification`: convert trace fixtures from plan modules to workflow modules.
- [x] `11-workflow-runtime-hardening`: workflow-runtime-hardening fixture set and outputs.
- [ ] `20-agent-session`: deferred (blocked on Agent SDK refresh); remove remaining legacy manifest vocabulary after SDK update.
- [ ] `21-chat-live`: deferred (blocked on Agent SDK refresh); remove wrapper plan and use workflow subscription/orchestration directly after SDK update.
- [ ] `22-agent-live`: deferred (blocked on Agent SDK refresh); remove wrapper plan and use workflow subscription/orchestration directly after SDK update.
- [x] Update `crates/aos-smoke/fixtures/README.md` to describe only workflow-era fixtures and scenarios.
- [x] Update `crates/aos-smoke/src` runners to remove plan-era naming/artifacts (`plan-summary`, `PlanEnded` assumptions, etc.).

Integration suite rewrite checklist:
- [x] `crates/aos-host/tests/world_integration.rs`: replace ignored plan-runtime tests with workflow-runtime equivalents.
- [x] `crates/aos-host/tests/journal_integration.rs`: migrate journal assertions from plan records to workflow records/state.
- [x] `crates/aos-host/tests/snapshot_integration.rs`: migrate snapshot/replay assertions to workflow in-flight state semantics.
- [x] `crates/aos-host/tests/governance_effects_integration.rs` (replaces `governance_plan_integration.rs`): governance propose/shadow/approve/apply exercised via workflow-origin effects (no plan APIs).
- [x] `crates/aos-host/tests/governance_integration.rs`: removed plan patch/apply assertions and kept workflow-manifest governance checks only.
- [x] `crates/aos-host/tests/fixtures.rs`: replace plan-oriented fixtures/helpers with workflow-oriented fixtures/helpers.
- [x] `crates/aos-host/tests/helpers.rs`: replace plan-oriented helpers and names with workflow-oriented helpers.

Required coverage outcomes from rewritten fixtures/suites:
- [x] external I/O orchestration.
- [x] retries/timeouts via receipts/events.
- [x] multi-instance concurrency and isolation.
- [x] governance apply on workflow manifests.
- [x] subscription wiring changes while receipts are in flight.
- [x] subscription wiring changes while continuation frames are in flight (if P7 enabled; N/A for current baseline where P7 is not enabled).
- [x] upgrade-while-waiting (`pending receipt + snapshot + blocked apply + late receipt + deterministic continuation`).

### 2) Spec/doc rewrite

- [x] Update `spec/03-air.md`, `spec/05-workflows.md`, and architecture docs to remove plan model claims.
- [x] Update `AGENTS.md` architecture summary and boundaries.
- [x] Ensure schema docs match implementation.

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
13. [x] Strict receipt settlement invariant: malformed receipt payloads are terminalized (pending intent consumed) so faulty adapters cannot clog runtime progress.
14. [x] Optional rejected-receipt continuation: workflows may handle `sys/EffectReceiptRejected@1`; if not handled, instance is marked failed and remaining in-flight intents are drained.
15. [x] Remove receipt/event decode compatibility fallbacks in runtime SDK layers; require canonical schema-conformant payloads.

Implementation log (completed 2026-02-26):
- [x] Kernel receipt handling now terminalizes malformed receipt paths: pending intent is consumed; optional `sys/EffectReceiptRejected@1` event is delivered when supported; otherwise the workflow instance is marked `failed` and remaining in-flight intents are drained.
  - `crates/aos-kernel/src/world/plan_runtime.rs`
- [x] Added built-in rejected-receipt schema for optional workflow handling.
  - `spec/defs/builtin-schemas.air.json`
  - `crates/aos-air-types/src/builtins.rs`
  - `crates/aos-kernel/src/receipts.rs`
- [x] Removed receipt decode compatibility fallback (`self-describe` tag stripping); decoding now requires canonical schema-conformant CBOR.
  - `crates/aos-effects/src/receipt.rs`
  - `crates/aos-wasm-sdk/src/reducers.rs`
- [x] Tightened event decode compatibility: `aos_event_union!` now requires canonical tagged event payloads (`$tag`/`$value`) and no longer accepts untagged fallback payloads.
  - `crates/aos-wasm-sdk/src/reducers.rs`
- [x] Removed adapter receipt-payload encoding fallbacks (`unwrap_or_default`) in host HTTP/LLM adapters.
  - `crates/aos-host/src/adapters/http.rs`
  - `crates/aos-host/src/adapters/llm.rs`
- [x] Replaced generic stub receipt payloads with per-effect typed schema-conformant receipts (`http.request`, `llm.generate`, `blob.put`, `blob.get`, `timer.set`).
  - `crates/aos-host/src/adapters/stub.rs`
- [x] Added active regression coverage for both rejected-receipt modes:
  - workflow without rejected variant: malformed receipt fails instance and clears pending intents.
  - workflow with rejected variant: malformed receipt raises rejected event and workflow continues deterministically.
  - `crates/aos-host/tests/journal_integration.rs`
- [x] Verified with targeted checks:
  - `cargo test -p aos-effects -q`
  - `cargo test -p aos-wasm-sdk -q`
  - `cargo test -p aos-air-types -q`
  - `cargo test -p aos-kernel receipts::tests::workflow_rejected_receipt_event_is_structured -q`
  - `cargo check -p aos-host`
  - `cargo test -p aos-host --test journal_integration malformed_workflow_receipt -q`
  - `cargo run -p aos-smoke -- hello-timer`
  - `cargo run -p aos-smoke -- blob-echo`
  - `cargo run -p aos-smoke -- fetch-notify`
  - `cargo run -p aos-smoke -- retry-backoff`
- [x] Rewrote `06-safe-upgrade` (`air.v1` + `air.v2`) to workflow modules and removed plan triggers/assets; smoke flow now proves `pending receipt + snapshot + blocked apply + late receipt continuation + post-apply upgraded behavior`.
  - `crates/aos-smoke/fixtures/06-safe-upgrade/air.v1/*`
  - `crates/aos-smoke/fixtures/06-safe-upgrade/air.v2/*`
  - `crates/aos-smoke/fixtures/06-safe-upgrade/reducer/*`
  - `crates/aos-smoke/fixtures/06-safe-upgrade/reducer-v2/*`
  - `crates/aos-smoke/src/safe_upgrade.rs`
- [x] Adjusted strict-quiescence apply gate to block on actual in-flight runtime work (inflight workflow intents / pending workflow receipts / queued effects / scheduler) so apply succeeds once waiting work is settled.
  - `crates/aos-kernel/src/world/governance_runtime.rs`
- [x] Migrated `07-llm-summarizer` from `summarize_plan` to workflow-native orchestration (`Start + Receipt(sys/EffectReceiptEnvelope@1)`), with direct `http.request -> llm.generate` emission in reducer/workflow.
  - `crates/aos-smoke/fixtures/07-llm-summarizer/air/*`
  - `crates/aos-smoke/fixtures/07-llm-summarizer/reducer/src/lib.rs`
  - `crates/aos-smoke/src/llm_summarizer.rs`
- [x] Rebuilt `08-retry-backoff` around workflow runtime receipts/events: reducer/workflow now handles `Receipt(sys/EffectReceiptEnvelope@1)` directly for both `http.request` and `timer.set`, and drives exponential backoff retries deterministically.
  - `crates/aos-smoke/fixtures/08-retry-backoff/air/*`
  - `crates/aos-smoke/fixtures/08-retry-backoff/reducer/src/lib.rs`
  - `crates/aos-smoke/src/retry_backoff.rs`
- [x] Rewrote `09-workspaces` to workflow-native orchestration with direct workspace effects (`resolve -> [empty_root] -> write -> list -> diff`) and deterministic per-workspace commit emission.
  - `crates/aos-smoke/fixtures/09-workspaces/air/*` (removed legacy `workspace_plan.air.json`; workflow subscriptions/policy/module wiring only)
  - `crates/aos-smoke/fixtures/09-workspaces/reducer/src/lib.rs`
  - `crates/aos-smoke/src/workspaces.rs`
  - verification: `cargo test --manifest-path crates/aos-smoke/fixtures/09-workspaces/reducer/Cargo.toml -q`, `cargo run -p aos-smoke -- workspaces`
- [x] Rewrote `10-trace-failure-classification` to workflow-native failure routing across all fixture variants (`allow`, `cap_deny`, `policy_deny`): removed `defplan` assets/triggers, switched policy origin checks to `workflow`, and updated reducer to emit `http.request` directly and handle `Receipt(sys/EffectReceiptEnvelope@1)`.
  - `crates/aos-smoke/fixtures/10-trace-failure-classification/air.allow/*`
  - `crates/aos-smoke/fixtures/10-trace-failure-classification/air.cap_deny/*`
  - `crates/aos-smoke/fixtures/10-trace-failure-classification/air.policy_deny/*`
  - `crates/aos-smoke/fixtures/10-trace-failure-classification/reducer/src/lib.rs`
  - `crates/aos-smoke/src/trace_failure_classification.rs`
  - verification: `cargo run -p aos-smoke -- trace-failure-classification`
- [x] Replaced fixture `11` with workflow-runtime-hardening behavior/output: removed plan assets, rewired AIR to workflow subscriptions, implemented direct workflow state-machine orchestration (`Start/Approval/Receipt`) for cross-talk isolation + crash/resume, and renamed runner/CLI surface to `workflow-runtime-hardening`.
  - `crates/aos-smoke/fixtures/11-workflow-runtime-hardening/air/*`
  - `crates/aos-smoke/fixtures/11-workflow-runtime-hardening/reducer/*`
  - `crates/aos-smoke/src/workflow_runtime_hardening.rs`
  - `crates/aos-smoke/src/main.rs`
  - verification: `cargo run -p aos-smoke -- workflow-runtime-hardening`
- [x] Deferred Agent SDK smoke fixtures (`20-agent-session`, `21-chat-live`, `22-agent-live`) pending Agent SDK update; fixture migration resumes after SDK refresh.
  - tracked in this checklist as deferred scope, not yet implemented
- [x] Removed remaining plan-era wording/artifacts from smoke fixture index and runner metadata/diagnostics (workflow-first terminology only).
  - `crates/aos-smoke/fixtures/README.md`
  - `crates/aos-smoke/src/main.rs`
  - `crates/aos-smoke/src/chat_live.rs`
- [x] Integration deletion pass (plan surface first):
  - replaced `crates/aos-host/tests/governance_plan_integration.rs` with `crates/aos-host/tests/governance_effects_integration.rs` using workflow-origin effect intents (`enqueue_reducer_effect_with_grant`).
  - rewrote `crates/aos-host/tests/governance_integration.rs` to workflow-manifest governance assertions only (removed `DefPlan`/`AirNode::Defplan`/manifest `plans` assumptions).
  - targeted verification:
    - `cargo test -p aos-host --test governance_effects_integration --features e2e-tests -q`
- [x] `world_integration.rs` drop-only pass completed per migration map: removed all plan-only tests explicitly marked `Drop`; retained `Keep`/`Migrate` tests for follow-up workflow rewrites.
  - map: `roadmap/v0.11-workflows/p5-world-integration-migration-map.md`
  - verification: `cargo test -p aos-host --test world_integration --features e2e-tests -q`
- [x] `world_integration.rs` migration batch 1 completed: moved five workflow-relevant ignored tests into `crates/aos-host/tests/workflow_runtime_integration.rs` and removed the old plan-era variants from `world_integration.rs`.
  - migrated: `single_plan_orchestration_completes_after_receipt`, `reducer_and_plan_effects_are_enqueued`, `reducer_timer_receipt_routes_event_to_handler`, `blob_put_receipt_routes_event_to_handler`, `blob_get_receipt_routes_event_to_handler`
  - verification:
    - `cargo test -p aos-host --test workflow_runtime_integration --features e2e-tests -q`
    - `cargo test -p aos-host --test world_integration --features e2e-tests -q`
- [x] `world_integration.rs` migration batch 2 completed: migrated remaining workflow-relevant plan-era tests and removed all ignored plan-runtime cases from `world_integration.rs`.
  - migrated to `crates/aos-host/tests/workflow_runtime_integration.rs`: `plan_waits_for_receipt_and_event_before_progressing`, `plan_event_wakeup_only_resumes_matching_schema`, `correlated_await_event_prevents_cross_talk_between_instances`
  - migrated to `crates/aos-host/tests/journal_integration.rs`: `replay_does_not_double_apply_receipt_spawned_domain_events`
  - migrated to `crates/aos-host/tests/snapshot_integration.rs`: `subplan_receipt_wait_survives_restart_and_resumes_parent`
  - rewrote in-place (workflow-native): `raised_events_are_routed_to_reducers`
  - verification:
    - `cargo test -p aos-host --features e2e-tests --test world_integration --test workflow_runtime_integration --test journal_integration --test snapshot_integration -q`
- [x] `journal_integration.rs` + `snapshot_integration.rs` workflow cutover cleanup completed:
  - removed all remaining ignored plan-runtime fixtures from both files and replaced with workflow-native replay/snapshot coverage.
  - `journal_integration.rs`: active workflow coverage now includes receipt replay idempotency, malformed-receipt settlement modes, workflow-origin policy/cap decision journaling, and fs-journal waiting-state restoration.
  - `snapshot_integration.rs`: active workflow coverage now includes in-flight queue persistence, receipt-wait restart continuation, cap-decision snapshot replay, and workflow-manifest replay-authority checks.
  - verification:
    - `cargo test -p aos-host --features e2e-tests --test journal_integration --test snapshot_integration -q`
    - `cargo test -p aos-host --features e2e-tests --test world_integration --test workflow_runtime_integration --test journal_integration --test snapshot_integration -q`
- [x] Completed workflow-only fixtures/helpers migration and integration fallout cleanup.
  - `crates/aos-host/tests/fixtures.rs`: removed legacy plan helper surface (`build_loaded_manifest` now module/routing only; plan trigger helpers removed; workflow-first comments and signatures).
  - `crates/aos-host/tests/helpers.rs`: removed plan-era helper types/imports and kept workflow-native helper surface only.
  - `crates/aos-host/tests_e2e/cap_enforcer_e2e.rs`: rewritten to workflow reducer-based enforcement scenarios (no plan APIs).
  - verification:
    - `cargo test -p aos-host --features e2e-tests -q`
- [x] Fixed canonical receipt decoding for optional payloads represented as CBOR `null` (required for schema-conformant `option` receipts like `workspace.read_ref` when missing).
  - `crates/aos-effects/src/receipt.rs`: decode through CBOR value conversion path so `Option<T>` payloads decode deterministically from canonical payloads.
  - `crates/aos-host/tests_e2e/workspace_e2e.rs`: `workspace_tree_effects_roundtrip` now passes end-to-end under canonical receipt semantics.
  - verification:
    - `cargo test -p aos-effects -q`
    - `cargo test -p aos-host --features e2e-tests workspace_tree_effects_roundtrip -- --nocapture`
    - `cargo test -p aos-host --features e2e-tests -q`
- [x] Required coverage outcomes section completed with explicit workflow-era fixture/suite evidence:
  - external I/O orchestration:
    - `crates/aos-host/tests/workflow_runtime_integration.rs::workflow_orchestration_completes_after_receipt`
    - smoke fixtures `03-fetch-notify`, `04-aggregator`, `05-chain-comp`, `07-llm-summarizer`
  - retries/timeouts via receipts/events:
    - smoke fixtures `08-retry-backoff`, `10-trace-failure-classification` (`adapter_timeout`)
  - multi-instance concurrency and isolation:
    - `crates/aos-host/tests/workflow_runtime_integration.rs::keyed_workflow_receipt_routing_is_instance_isolated`
    - smoke fixture `11-workflow-runtime-hardening` (cross-talk gate + crash/resume)
  - governance apply on workflow manifests:
    - `crates/aos-host/tests/governance_integration.rs::governance_flow_applies_manifest_patch`
    - `crates/aos-host/tests/governance_effects_integration.rs::governance_effects_apply_patch_doc_from_workflow_origin`
  - subscription wiring changes while receipts are in flight:
    - smoke fixture `06-safe-upgrade` (proposal+approval attempted while waiting; apply blocked by strict quiescence; late receipt settles deterministic continuation; apply then succeeds on upgraded wiring)
  - continuation-frame wiring:
    - marked N/A because P7 streaming continuation frames are optional and not enabled in the current baseline.
  - verification:
    - `cargo test -p aos-host --features e2e-tests --test workflow_runtime_integration workflow_orchestration_completes_after_receipt -q`
    - `cargo test -p aos-host --features e2e-tests --test workflow_runtime_integration keyed_workflow_receipt_routing_is_instance_isolated -q`
    - `cargo test -p aos-host --features e2e-tests --test governance_integration governance_flow_applies_manifest_patch -q`
    - `cargo test -p aos-host --features e2e-tests --test governance_effects_integration governance_effects_apply_patch_doc_from_workflow_origin -q`
    - `cargo run -p aos-smoke -- safe-upgrade`
    - `cargo run -p aos-smoke -- retry-backoff`
    - `cargo run -p aos-smoke -- workflow-runtime-hardening`
    - `cargo run -p aos-smoke -- trace-failure-classification`
- [x] Spec/doc rewrite completed for post-plan active architecture contract.
  - rewritten specs:
    - `spec/01-overview.md`
    - `spec/02-architecture.md`
    - `spec/03-air.md`
    - `spec/04-reducers.md`
    - `spec/05-workflows.md`
    - `spec/06-cells.md`
  - updated agent guidance:
    - `AGENTS.md` (and `CLAUDE.md` symlink target)
  - key updates captured:
    - workflow-only orchestration model (`defplan` removed from active spec narrative),
    - manifest `routing.subscriptions` ingress model + manifest-independent receipt continuation routing,
    - strict-quiescence governance apply semantics,
    - canonical event/effect/receipt normalization and rejected-receipt handling invariants,
    - policy origin semantics aligned to `workflow|system|governance`.

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
3. [x] Extend `fixtures/06-safe-upgrade` to cover upgrade-while-waiting end-to-end, including snapshot + blocked apply + post-receipt apply.

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
10. [x] Fixture suite includes upgrade-while-waiting with snapshot and verifies deterministic behavior under the selected upgrade rule.
