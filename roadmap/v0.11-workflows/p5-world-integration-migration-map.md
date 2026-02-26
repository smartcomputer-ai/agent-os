# P5 World Integration Migration Map

Status: Completed
Scope: `crates/aos-host/tests/world_integration.rs`

## Legend
- `Keep`: keep in `world_integration.rs` as-is (or only minimal cleanup).
- `Drop`: retire with no 1:1 replacement because behavior is plan-runtime-only or covered elsewhere.
- `Migrate`: rewrite as workflow-era coverage in a target file.

## Completion marks

- [x] Removed all tests currently marked `Drop` from `crates/aos-host/tests/world_integration.rs`.
  - `sugar_literal_plan_executes_http_flow`
  - `guarded_plan_branches_control_effects`
  - `trigger_when_filters_plan_start`
  - `trigger_input_expr_projects_event_into_plan_input`
  - `spawn_plan_await_plan_and_plan_started_parent_linkage`
  - `await_plan_surfaces_error_variant_from_failed_child`
  - `spawn_for_each_await_plans_all_preserves_order`
  - `plan_outputs_are_journaled_and_replayed`
  - `invariant_failure_records_plan_ended_error`
- [x] Verified after drop-only removal:
  - `cargo test -p aos-host --test world_integration --features e2e-tests -q`
- [x] Migrated workflow runtime batch 1 from `world_integration.rs` to `workflow_runtime_integration.rs`:
  - `single_plan_orchestration_completes_after_receipt`
  - `reducer_and_plan_effects_are_enqueued`
  - `reducer_timer_receipt_routes_event_to_handler`
  - `blob_put_receipt_routes_event_to_handler`
  - `blob_get_receipt_routes_event_to_handler`
- [x] Verified migrated batch 1:
  - `cargo test -p aos-host --test workflow_runtime_integration --features e2e-tests -q`
  - `cargo test -p aos-host --test world_integration --features e2e-tests -q`
- [x] Migrated workflow runtime batch 2 from `world_integration.rs`:
  - `plan_waits_for_receipt_and_event_before_progressing`
  - `plan_event_wakeup_only_resumes_matching_schema`
  - `correlated_await_event_prevents_cross_talk_between_instances`
- [x] Migrated replay/restart coverage from `world_integration.rs`:
  - `replay_does_not_double_apply_receipt_spawned_domain_events` -> `crates/aos-host/tests/journal_integration.rs`
  - `subplan_receipt_wait_survives_restart_and_resumes_parent` -> `crates/aos-host/tests/snapshot_integration.rs`
- [x] Rewrote `raised_events_are_routed_to_reducers` in-place as workflow-native reducer event routing and removed all remaining ignored plan-era tests from `crates/aos-host/tests/world_integration.rs`.
- [x] Verified completed map migration:
  - `cargo test -p aos-host --features e2e-tests --test world_integration --test workflow_runtime_integration --test journal_integration --test snapshot_integration -q`

## Test-by-test map

| Current test | Current state | Decision | Target file | What needs to change |
|---|---|---|---|---|
| `rejects_event_payload_that_violates_schema` | active | Keep | `crates/aos-host/tests/world_integration.rs` | Keep as schema-ingress guardrail for workflow reducers. |
| `sugar_literal_plan_executes_http_flow` | ignored | Drop | n/a | Pure plan-literal normalization/execution path (`defplan` sugar/canonical) is retired. |
| `single_plan_orchestration_completes_after_receipt` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with workflow reducer event variant (`Start`/`Receipt`) and direct `http.request` receipt continuation assertion. |
| `reducer_and_plan_effects_are_enqueued` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with workflow-only shared outbox assertion across `timer.set` + `http.request` emitters. |
| `reducer_timer_receipt_routes_event_to_handler` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with workflow timer receipt continuation test including duplicate receipt dedupe + unknown receipt error behavior. |
| `guarded_plan_branches_control_effects` | ignored | Drop | n/a | Plan edge/guard semantics are retired; branch logic belongs in reducer logic tests/smoke fixtures, not kernel plan runtime. |
| `blob_put_receipt_routes_event_to_handler` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Migrated as workflow-origin `blob.put` receipt envelope routing assertion (`sys/BlobPutResult@1`). |
| `blob_get_receipt_routes_event_to_handler` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Migrated as workflow-origin `blob.get` receipt envelope routing assertion (`sys/BlobGetResult@1`). |
| `plan_waits_for_receipt_and_event_before_progressing` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with workflow-native staged receipt/event progression coverage (`workflow_receipt_and_event_progression_emit_followups_in_order`). |
| `replay_does_not_double_apply_receipt_spawned_domain_events` | migrated `[x]` | Migrate | `crates/aos-host/tests/journal_integration.rs` | Reframed as workflow receipt-driven continuation replay check (`workflow_replay_does_not_double_apply_receipt_spawned_domain_events`) with journal-length/idempotency assertions. |
| `plan_event_wakeup_only_resumes_matching_schema` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with schema-scoped workflow routing isolation coverage (`workflow_event_routing_only_matches_subscribed_schema`). |
| `trigger_when_filters_plan_start` | ignored | Drop | n/a | Trigger `when` is a plan-start feature; no workflow-era equivalent in module routing path. |
| `trigger_input_expr_projects_event_into_plan_input` | ignored | Drop | n/a | Trigger input projection is plan-start specific and should be retired with trigger/plan runtime. |
| `spawn_plan_await_plan_and_plan_started_parent_linkage` | ignored | Drop | n/a | Subplan spawn/await linkage is plan runtime behavior and should be removed, not ported. |
| `await_plan_surfaces_error_variant_from_failed_child` | ignored | Drop | n/a | Child-plan error variant propagation is plan runtime behavior; no workflow counterpart. |
| `spawn_for_each_await_plans_all_preserves_order` | ignored | Drop | n/a | Plan fan-out/fan-in (`spawn_for_each` + `await_plans_all`) is retired. |
| `correlated_await_event_prevents_cross_talk_between_instances` | migrated `[x]` | Migrate | `crates/aos-host/tests/workflow_runtime_integration.rs` (new) | Replaced with keyed workflow receipt isolation coverage (`keyed_workflow_receipt_routing_is_instance_isolated`) proving per-instance receipt settlement without cross-talk. |
| `subplan_receipt_wait_survives_restart_and_resumes_parent` | migrated `[x]` | Migrate | `crates/aos-host/tests/snapshot_integration.rs` | Replaced with single workflow receipt-wait restart/resume coverage (`workflow_receipt_wait_survives_restart_and_resumes_continuation`). |
| `plan_outputs_are_journaled_and_replayed` | ignored | Drop | n/a | `PlanResult` journal surface is retired; equivalent observability is workflow instance snapshot/status plus domain events (already covered elsewhere). |
| `invariant_failure_records_plan_ended_error` | ignored | Drop | n/a | `PlanEnded` error record is retired; workflow failure semantics already covered by malformed-receipt/failure-path tests in `journal_integration.rs`. |
| `raised_events_are_routed_to_reducers` | migrated `[x]` | Migrate | `crates/aos-host/tests/world_integration.rs` (retain in-file) | Rewritten as workflow reducer `domain_events` source and verified routed delivery to subscribed reducer (`raised_events_are_routed_to_reducers`). |

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
