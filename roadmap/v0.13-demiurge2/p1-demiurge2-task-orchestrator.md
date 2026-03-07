# P1: Demiurge2 Task-Driven Orchestrator

**Priority**: P1  
**Effort**: High  
**Status**: Implemented (in progress validation)

## Goal

Rewrite `worlds/demiurge` as a task-ingress orchestrator that drives
`aos.agent/SessionWorkflow@1` via domain events.

## Scope

1. Task-first ingress: `demiurge/TaskSubmitted@1`.
2. Two-module world architecture:
   - `demiurge/Demiurge@1` (bootstrap/orchestrator)
   - `aos.agent/SessionWorkflow@1` (coding-agent loop)
3. `aos-agent` lifecycle telemetry event:
   - `aos.agent/SessionLifecycleChanged@1`.
4. `aos-agent-eval` entry mode for Demiurge task submission.

## Public Contracts

### New `aos-agent` schema

`aos.agent/SessionLifecycleChanged@1`

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
6. `demiurge/TaskFinished@1`
7. `demiurge/State@1`
8. `demiurge/WorkflowEvent@1`

## Runtime Flow

1. Receive `TaskSubmitted(task_id, workdir, task, config)`.
2. Emit `blob.put` for task message JSON.
3. On `blob.put` receipt, emit `host.session.open` (`network_mode=none`, `workdir`).
4. On ready host-session receipt, emit `aos.agent/SessionIngress@1`:
   - optional `ToolRegistrySet` when `allowed_tools` is present,
   - `HostSessionUpdated(ready)`,
   - `RunRequested(input_ref, run_overrides)`.
5. Subscribe to `SessionLifecycleChanged` and finalize task state:
   - `Running` -> running
   - `WaitingInput|Completed` -> succeeded + `TaskFinished`
   - `Failed` -> failed + `TaskFinished`
   - `Cancelled` -> cancelled + `TaskFinished`

## Implementation Notes

- `task_id` is reused as `session_id`.
- Default run config:
  - provider: `openai-responses`
  - model: `gpt-5.3-codex`
  - `max_tokens`: `4096`
- `allowed_tools` are validated against `aos_agent::default_tool_registry()`.
- Task submission only in this slice (no mid-run task commands yet).

## Validation Plan

1. `cargo check -p aos-agent`
2. `cargo check --manifest-path worlds/demiurge/workflow/Cargo.toml`
3. `cargo check -p aos-agent-eval`
4. `worlds/demiurge/scripts/smoke_task_submit.sh`

## Acceptance Criteria

1. Single `TaskSubmitted` event bootstraps a session for a local directory.
2. Demiurge no longer uses legacy tool-request wrapper path.
3. Orchestration happens through domain events + subscriptions.
4. `aos-agent` emits lifecycle domain events consumed by Demiurge.
5. Existing shell chat compatibility is intentionally out of scope for this version.
