# v0.13 Demiurge2: Task-Driven AOS Agent Orchestrator

## Summary

This plan rewrites Demiurge as a task-ingress workflow that orchestrates `aos-agent` sessions through domain events, with a two-module architecture:

1. `demiurge/Demiurge@1` accepts `TaskSubmitted` events, bootstraps input blob + host session for a requested local workdir, and emits `aos.agent/SessionIngress@1`.
2. `aos.agent/SessionWorkflow@1` executes the coding-agent loop.
3. `aos-agent` is extended to emit lifecycle domain events so Demiurge can subscribe and finalize task status deterministically.

This is task-first and intentionally breaks current shell/chat compatibility in this iteration.


## Public API / Interface Changes

1. Add `aos-agent` lifecycle event schema and emission.
2. Replace Demiurge world public ingress with task event(s), not direct old wrapper/tool-request events.
3. Keep `aos.agent/SessionIngress@1` as internal inter-module control lane (Demiurge -> SessionWorkflow).

### New `aos-agent` schema

1. `aos.agent/SessionLifecycleChanged@1`
2. Proposed fields:
   - `session_id: aos.agent/SessionId@1`
   - `observed_at_ns: time`
   - `from: aos.agent/SessionLifecycle@1`
   - `to: aos.agent/SessionLifecycle@1`
   - `run_id: option<aos.agent/RunId@1>`
   - `in_flight_effects: nat`

### New Demiurge schemas

1. `demiurge/TaskConfig@1`
2. `demiurge/TaskSubmitted@1`
3. `demiurge/TaskStatus@1`
4. `demiurge/TaskFailure@1`
5. `demiurge/PendingStage@1`
6. `demiurge/State@1`
7. `demiurge/WorkflowEvent@1`
8. `demiurge/TaskFinished@1` (emitted by Demiurge for terminal outcome signaling)

### `TaskSubmitted` payload (decision-complete)

1. `task_id: aos.agent/SessionId@1` (UUID, required; also used as `session_id`)
2. `observed_at_ns: time`
3. `workdir: text` (absolute local path expected)
4. `task: text`
5. `config: option<demiurge/TaskConfig@1>`

### `TaskConfig` fields

1. `provider: option<text>` (default `openai-responses`)
2. `model: option<text>` (default `gpt-5.3-codex`)
3. `reasoning_effort: option<aos.agent/ReasoningEffort@1>`
4. `max_tokens: option<nat>` (default `4096`)
5. `tool_profile: option<text>`
6. `allowed_tools: option<list<text>>`
7. `tool_enable: option<list<text>>`
8. `tool_disable: option<list<text>>`
9. `tool_force: option<list<text>>`
10. `session_ttl_ns: option<nat>`

## Architecture and Data Flow

1. `TaskSubmitted` routes to `demiurge/Demiurge@1` keyed by `task_id`.
2. Demiurge emits `blob.put` with user message blob from `task`.
3. On `blob.put` receipt, Demiurge emits `host.session.open` with:
   - `target.local.workdir = workdir`
   - `target.local.network_mode = "none"`
4. On `host.session.open` receipt `status=ready`, Demiurge emits domain events:
   - optional `aos.agent/SessionIngress@1::ToolRegistrySet` when `allowed_tools` is provided
   - `aos.agent/SessionIngress@1::HostSessionUpdated(ready)`
   - `aos.agent/SessionIngress@1::RunRequested(input_ref, run_overrides)`
5. `aos.agent/SessionWorkflow@1` runs normally and drives LLM/tools.
6. `aos.agent` emits `SessionLifecycleChanged` events.
7. Demiurge subscribes to lifecycle events and updates task state:
   - `to=Running` -> `Running`
   - `to=WaitingInput|Completed` -> `Succeeded` + emit `demiurge/TaskFinished@1`
   - `to=Failed` -> `Failed` + emit `demiurge/TaskFinished@1`
   - `to=Cancelled` -> `Cancelled` + emit `demiurge/TaskFinished@1`

## Implementation Plan

1. **Extend `aos-agent` with lifecycle events**
   - Update [crates/aos-agent/air/schemas.air.json](/Users/lukas/dev/aos/crates/aos-agent/air/schemas.air.json).
   - Update [crates/aos-agent/air/manifest.air.json](/Users/lukas/dev/aos/crates/aos-agent/air/manifest.air.json).
   - Add Rust contract type in [crates/aos-agent/src/contracts/events.rs](/Users/lukas/dev/aos/crates/aos-agent/src/contracts/events.rs).
   - Emit lifecycle events in [crates/aos-agent/src/bin/session_workflow.rs](/Users/lukas/dev/aos/crates/aos-agent/src/bin/session_workflow.rs) by diffing pre/post reducer lifecycle.
   - Add tests in `aos-agent` for lifecycle emission logic.

2. **Rewrite Demiurge defs and module contracts**
   - Replace [worlds/demiurge/air/schemas.air.json](/Users/lukas/dev/aos/worlds/demiurge/air/schemas.air.json) with task-first schemas.
   - Replace [worlds/demiurge/air/module.air.json](/Users/lukas/dev/aos/worlds/demiurge/air/module.air.json) to emit only `blob.put` and `host.session.open`.
   - Replace [worlds/demiurge/air/manifest.air.json](/Users/lukas/dev/aos/worlds/demiurge/air/manifest.air.json) with dual-module routing:
     - `demiurge/TaskSubmitted@1` -> `demiurge/Demiurge@1`
     - `aos.agent/SessionIngress@1` -> `aos.agent/SessionWorkflow@1`
     - `aos.agent/SessionLifecycleChanged@1` -> `demiurge/Demiurge@1`
   - Rebuild [worlds/demiurge/air/policies.air.json](/Users/lukas/dev/aos/worlds/demiurge/air/policies.air.json) for both origins.
   - Rebuild [worlds/demiurge/air/capabilities.air.json](/Users/lukas/dev/aos/worlds/demiurge/air/capabilities.air.json) for llm/host grants.
   - Remove legacy tool-request defs from Demiurge world (no `ToolCallRequested` lane).

3. **Rewrite Demiurge workflow from scratch**
   - Replace [worlds/demiurge/workflow/src/lib.rs](/Users/lukas/dev/aos/worlds/demiurge/workflow/src/lib.rs) with deterministic bootstrap-orchestrator state machine.
   - Use `ctx.intent("aos.agent/SessionIngress@1")` to drive session workflow.
   - Validate `allowed_tools` against `aos_agent::default_tool_registry()` when provided.
   - Maintain `next_observed_at_ns` in Demiurge state for deterministic ingress timestamps.
   - Map bootstrap receipt failures into terminal `TaskFailure` codes.

4. **Align all worlds/fixtures that run `SessionWorkflow`**
   - Add `aos.agent/SessionLifecycleChanged@1` to schema lists in:
     - [crates/aos-agent-eval/fixtures/eval-world/air/manifest.air.json](/Users/lukas/dev/aos/crates/aos-agent-eval/fixtures/eval-world/air/manifest.air.json)
     - [crates/aos-smoke/fixtures/21-agent-session/air/manifest.air.json](/Users/lukas/dev/aos/crates/aos-smoke/fixtures/21-agent-session/air/manifest.air.json)
     - [crates/aos-smoke/fixtures/22-agent-live/air/manifest.air.json](/Users/lukas/dev/aos/crates/aos-smoke/fixtures/22-agent-live/air/manifest.air.json)
     - [crates/aos-smoke/fixtures/23-agent-tools/air/manifest.air.json](/Users/lukas/dev/aos/crates/aos-smoke/fixtures/23-agent-tools/air/manifest.air.json)

5. **Validation harness integration (using `aos-agent-eval` groundwork)**
   - Add Demiurge entry mode to [crates/aos-agent-eval/src/main.rs](/Users/lukas/dev/aos/crates/aos-agent-eval/src/main.rs):
     - send `demiurge/TaskSubmitted@1` instead of direct `SessionIngress`.
     - keep existing effect-driving loop and case expectations.
   - Add helper mappings in [crates/aos-agent-eval/src/eval_host.rs](/Users/lukas/dev/aos/crates/aos-agent-eval/src/eval_host.rs) for reading Demiurge task state and SessionWorkflow state by same `task_id/session_id`.

6. **Smoke and docs**
   - Replace or add task smoke script at [worlds/demiurge/scripts/smoke_task_submit.sh](/Users/lukas/dev/aos/worlds/demiurge/scripts/smoke_task_submit.sh).
   - Document task API and CLI examples in [worlds/demiurge/README.md](/Users/lukas/dev/aos/worlds/demiurge/README.md) (new).
   - Write this roadmap content to [roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md](/Users/lukas/dev/aos/roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md).

## Test Cases and Scenarios

1. **Unit: Demiurge bootstrap**
   - `task_submitted_emits_blob_put`
   - `blob_put_receipt_emits_host_session_open_with_workdir`
   - `host_session_open_receipt_emits_session_ingress_sequence`
   - `host_session_open_error_marks_task_failed`
   - `duplicate_task_id_rejected_or_ignored_deterministically`

2. **Unit: `aos-agent` lifecycle events**
   - lifecycle event emitted on `Idle -> Running`
   - lifecycle event emitted on `Running -> WaitingInput`
   - lifecycle event emitted on `Running -> Failed`
   - no lifecycle event emitted when lifecycle unchanged

3. **Integration: world wiring**
   - send one `TaskSubmitted`; assert:
     - SessionWorkflow keyed state exists for same UUID
     - host session is set ready before run starts
     - task state reaches `Succeeded` when lifecycle hits `WaitingInput`
   - replay from genesis yields byte-identical snapshot

4. **Eval: coding-agent validation path**
   - run at least 3 existing eval cases through Demiurge entry mode (`read/write`, `grep`, `apply_patch`/`edit_file` provider-specific)
   - pass-rate threshold same as case metadata
   - verify filesystem side effects in workdir seeded from task event path

## Acceptance Criteria

1. One `demiurge/TaskSubmitted@1` event is sufficient to start a coding-agent run against a specified local directory.
2. Demiurge no longer depends on outdated tool-settling wrapper logic.
3. Session orchestration is domain-event-driven across two workflows.
4. Lifecycle subscription is implemented via new `aos-agent` lifecycle events.
5. Deterministic replay passes for new flow.
6. Old shell/chat compatibility is not required in this release.

## Assumptions and Defaults

1. `task_id` is caller-provided UUID and is reused as `session_id`.
2. v0.13 scope is task submission only; no mid-run steering command schema yet.
3. Provider/model defaults: `openai-responses` + `gpt-5.3-codex`.
4. Success criterion for coding tasks is first terminal useful lifecycle: `WaitingInput` or `Completed`.
5. Host capability is permissive enough for validation tasks unless explicitly tightened later.
6. Existing `worlds/demiurge` state/data is treated as migration-breaking; re-init world before validation.
7. During planning exploration, [worlds/demiurge/workflow/Cargo.lock](/Users/lukas/dev/aos/worlds/demiurge/workflow/Cargo.lock) changed due `cargo check`; implementation pass should intentionally regenerate/finalize lockfile with the rewrite.
