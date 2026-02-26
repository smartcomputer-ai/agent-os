# P5 World Integration Migration Map

Status: Proposed (planning map only; no checklist item here is implicitly completed)
Scope: `crates/aos-host/tests/world_integration.rs`

## Legend
- `Keep`: keep in `world_integration.rs` as-is (or only minimal cleanup).
- `Drop`: retire with no 1:1 replacement because behavior is plan-runtime-only or covered elsewhere.
- `Migrate`: rewrite as workflow-era coverage in a target file.

## Test-by-test map

| Current test | Current state | Decision | Target file | What needs to change |
|---|---|---|---|---|
| `rejects_event_payload_that_violates_schema` | active | Keep | `crates/aos-host/tests/world_integration.rs` | Keep as schema-ingress guardrail for workflow reducers. |
| `sugar_literal_plan_executes_http_flow` | ignored | Drop | n/a | Pure plan-literal normalization/execution path (`defplan` sugar/canonical) is retired. |
| `single_plan_orchestration_completes_after_receipt` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replace plan with workflow reducer event variant (`Start`/`Receipt`), emit `http.request` directly, settle receipt, assert reducer continuation/result state. |
| `reducer_and_plan_effects_are_enqueued` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replace mixed reducer+plan source with two workflow modules (or one module + micro-effect) to assert shared outbox ordering/containment without plan origin. |
| `reducer_timer_receipt_routes_event_to_handler` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Keep timer receipt routing semantics; assert handler state transition, duplicate receipt dedupe, unknown receipt error behavior. |
| `guarded_plan_branches_control_effects` | ignored | Drop | n/a | Plan edge/guard semantics are retired; branch logic belongs in reducer logic tests/smoke fixtures, not kernel plan runtime. |
| `blob_put_receipt_routes_event_to_handler` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Keep receipt-envelope routing + variant-wrap coverage for `sys/BlobPutResult@1`, but from workflow reducer emissions only. |
| `blob_get_receipt_routes_event_to_handler` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Keep receipt-envelope routing + variant-wrap coverage for `sys/BlobGetResult@1`, workflow-only origin. |
| `plan_waits_for_receipt_and_event_before_progressing` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Rebuild as workflow reducer state machine: emit effect, wait receipt, then await domain event, then emit follow-up effect; assert deterministic progression. |
| `replay_does_not_double_apply_receipt_spawned_domain_events` | ignored | Migrate | `crates/aos-host/tests/journal_integration.rs` | Reframe as workflow receipt -> domain event fanout; replay journal and assert emitted domain event is not duplicated on replay. |
| `plan_event_wakeup_only_resumes_matching_schema` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replace plan wakeup bookkeeping with workflow routing isolation: only module subscribed to matching schema/tag resumes. |
| `trigger_when_filters_plan_start` | ignored | Drop | n/a | Trigger `when` is a plan-start feature; no workflow-era equivalent in module routing path. |
| `trigger_input_expr_projects_event_into_plan_input` | ignored | Drop | n/a | Trigger input projection is plan-start specific and should be retired with trigger/plan runtime. |
| `spawn_plan_await_plan_and_plan_started_parent_linkage` | ignored | Drop | n/a | Subplan spawn/await linkage is plan runtime behavior and should be removed, not ported. |
| `await_plan_surfaces_error_variant_from_failed_child` | ignored | Drop | n/a | Child-plan error variant propagation is plan runtime behavior; no workflow counterpart. |
| `spawn_for_each_await_plans_all_preserves_order` | ignored | Drop | n/a | Plan fan-out/fan-in (`spawn_for_each` + `await_plans_all`) is retired. |
| `correlated_await_event_prevents_cross_talk_between_instances` | ignored | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Port intent to keyed workflow instances: submit distinct keys/correlation, ensure one instance advances while other remains waiting until its own event. |
| `subplan_receipt_wait_survives_restart_and_resumes_parent` | ignored | Migrate | `crates/aos-host/tests/journal_integration.rs` and `crates/aos-host/tests/snapshot_integration.rs` | Replace parent/child plan flow with single workflow instance waiting on receipt across restart/replay; assert late receipt resumes same instance deterministically. |
| `plan_outputs_are_journaled_and_replayed` | ignored | Drop | n/a | `PlanResult` journal surface is retired; equivalent observability is workflow instance snapshot/status plus domain events (already covered elsewhere). |
| `invariant_failure_records_plan_ended_error` | ignored | Drop | n/a | `PlanEnded` error record is retired; workflow failure semantics already covered by malformed-receipt/failure-path tests in `journal_integration.rs`. |
| `raised_events_are_routed_to_reducers` | ignored | Migrate | `crates/aos-host/tests/world_integration.rs` (retain in-file) | Keep this behavior but rewrite source from plan `raise_event` to workflow reducer `domain_events` emission and route to subscribed reducer. |

## Proposed extraction structure after migration

- Keep `world_integration.rs` focused on ingress/routing fundamentals (schema rejection, raised-event routing).
- Add `workflow_runtime_integration.rs` for workflow-only runtime sequencing and receipt routing (timer/blob/http, schema-match wakeups, keyed isolation).
- Put replay/restart semantics in `journal_integration.rs` and `snapshot_integration.rs` (no runtime-flow duplication in `world_integration.rs`).

## Drop summary (plan-only surfaces)

The following behaviors are intentionally removed with plan runtime:
- plan literal/sugar normalization execution.
- plan edge guards and trigger projection/start filtering.
- subplan spawn/await/fanout.
- plan output/result and `PlanEnded` journal records.
