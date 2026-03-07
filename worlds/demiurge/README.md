# Demiurge v0.13 (Task-Driven)

`worlds/demiurge` is a task-ingress orchestrator for `aos.agent/SessionWorkflow@1`.

## What It Does

1. Accepts `demiurge/TaskSubmitted@1`.
2. Writes task text as a user message blob.
3. Opens a host session for the provided `workdir`.
4. Emits `aos.agent/SessionIngress@1` events to bootstrap and run the agent session.
5. Tracks `aos.agent/SessionLifecycleChanged@1` and emits `demiurge/TaskFinished@1`.

## Input Event

Schema: `demiurge/TaskSubmitted@1`

Required fields:

- `task_id` (UUID; reused as `session_id`)
- `observed_at_ns`
- `workdir` (absolute local directory)
- `task`

Optional `config` fields:

- `provider`, `model`, `reasoning_effort`, `max_tokens`
- `tool_profile`, `allowed_tools`, `tool_enable`, `tool_disable`, `tool_force`
- `session_ttl_ns`

## Local Smoke

Run:

```bash
worlds/demiurge/scripts/smoke_task_submit.sh
```

The script compiles required wasm modules, initializes/pushes the world, submits a task event,
and verifies keyed state exists for both:

- `demiurge/Demiurge@1`
- `aos.agent/SessionWorkflow@1`
