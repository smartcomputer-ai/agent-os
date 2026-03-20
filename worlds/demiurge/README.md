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

The script uses the current local runtime path:
- starts `aos local` with `worlds/demiurge` as the local root
- creates a fresh local universe/world from `--local-root worlds/demiurge`
- submits `demiurge/TaskSubmitted@1` via `aos world send --follow`
- fetches both keyed workflow states plus the final output blob
- verifies keyed state exists for both workflows

It validates:

- `demiurge/Demiurge@1`
- `aos.agent/SessionWorkflow@1`

Provider selection defaults to:

- `openai-responses` when `OPENAI_API_KEY` is present
- `anthropic` when `ANTHROPIC_API_KEY` is present
- `mock` otherwise

With a live provider, the script waits for terminal completion and prints the extracted assistant
response when an output blob is available. With `mock`, success only means Demiurge and
SessionWorkflow start correctly in the local runtime; no real LLM call is made.

## Local Task Run

After starting the local node and selecting/creating the `demiurge` world, run:

```bash
worlds/demiurge/scripts/demiurge_task.sh --task "Read README.md and summarize the project name."
```

The script submits `demiurge/TaskSubmitted@1` through the same `aos world send --follow`
flow, waits for the task result, then prints the final task status and extracted assistant response.
