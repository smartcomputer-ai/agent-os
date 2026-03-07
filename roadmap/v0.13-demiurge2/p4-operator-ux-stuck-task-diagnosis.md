# P4: Operator UX for Stuck-Task Diagnosis

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md`

## Goal

Ship one opinionated operator workflow for the question:

`why is this task stuck?`

This is not a generic observability surface rewrite. It is a product slice that
packages existing control-plane verbs into a single first-class diagnostic path
for Demiurge/session-driven worlds before more governance work lands.

## Why This Comes Before More Governance

The runtime already has the low-level ingredients:

1. `trace-get`
2. `trace-summary`
3. `state-get`
4. `state-list`
5. `workspace-diff`

These are defined in `spec/09-control.md` and already reachable through the
daemon/control layer.

What is missing is operator synthesis:

1. choosing the right correlation root,
2. classifying the current wait condition,
3. showing the exact workflow/cell state involved,
4. surfacing whether the workspace changed,
5. presenting a concrete next action.

Without that, AgentOS still requires too much runtime literacy to debug a live
task.

## Product Slice

Add a first-class CLI command:

`aos task-diagnose`

Optional later follow-up:

`/api/debug/task` or a small UI panel built on the same backend response shape.

The first version is CLI-first and must work against a running daemon.

## Primary User Flows

### 1) Diagnose by task/session id

Operator provides a `task_id` / `session_id`.

The command:

1. finds the trace root,
2. loads task and session workflow state,
3. inspects current live wait / pending intent status,
4. compares relevant workspace roots when available,
5. prints one diagnosis summary plus supporting detail.

### 2) Diagnose by event hash

Operator already has the root domain event hash.

The command bypasses correlation lookup and renders the same diagnosis output.

### 3) Fleet summary

Operator wants a compact overview of current blocked work.

The command uses `trace-summary` plus `state-list` to show:

1. waiting task count,
2. failed task count,
3. workflows with in-flight intents,
4. oldest waiting cells,
5. most common blocker classes.

## Non-Goals

1. Full graph UI.
2. Streaming timeline viewer.
3. New kernel trace semantics.
4. Governance proposal inspection.
5. General-purpose APM/metrics.

## Proposed CLI

### Command shape

```text
aos task-diagnose --task-id <uuid>
aos task-diagnose --session-id <uuid>
aos task-diagnose --event-hash <sha256:...>
aos task-diagnose --task-id <uuid> --json
```

### Human output contract

Minimum sections:

1. identity
2. terminal/waiting classification
3. likely cause
4. active workflow states
5. latest intent / receipt summary
6. workspace delta summary
7. next suggested action

Example shape:

```text
task-diagnose: task=... terminal=waiting cause=awaiting_host_receipt
workflow=demiurge/Demiurge@1 key=...
session_workflow=aos.agent/SessionWorkflow@1 lifecycle=Running in_flight=1
last_intent=host.exec sha256:...
workspace_diff=3 changed paths
next_action=inspect adapter/host session for stalled host.exec receipt
```

## Data Sources and Call Plan

### Step 1: find the root

Preferred query order:

1. `trace-get { schema, correlate_by, value }` using the task ingress schema
2. fallback `trace-get` by supplied event hash

For Demiurge, the initial correlation target should be the task ingress lane:

1. `demiurge/TaskSubmitted@1`
2. `correlate_by = task_id` or `$value.task_id` depending on the trace query path

If the user passes a session id directly, use the session lineage correlation
already supported by the trace surface.

### Step 2: classify runtime health

Use `trace-get` + existing diagnosis logic from `aos trace-diagnose` as the
base classifier:

1. `policy_denied`
2. `capability_denied`
3. `adapter_timeout`
4. `adapter_error`
5. `waiting_for_receipt`
6. `waiting_for_event`
7. `completed`
8. `failed`

The new command should extend this with operator-facing wording specific to task
orchestration:

1. `bootstrap_failed`
2. `session_not_started`
3. `host_session_not_ready`
4. `tool_batch_blocked`
5. `workspace_not_materialized`

### Step 3: inspect workflow state

Load both keyed workflow states when present:

1. `state-get reducer=demiurge/Demiurge@1 key_b64=<task_id>`
2. `state-get reducer=aos.agent/SessionWorkflow@1 key_b64=<task_id>`

If the state is missing:

1. report that explicitly,
2. do not fail the whole diagnosis path,
3. downgrade to partial diagnosis.

### Step 4: inspect population-level context

Use `state-list` to show whether the task is isolated or part of broader queue
pressure:

1. `state-list reducer=demiurge/Demiurge@1`
2. `state-list reducer=aos.agent/SessionWorkflow@1`

Extract:

1. total active cells,
2. oldest `last_active_ns`,
3. whether many cells are stalled at once.

### Step 5: inspect workspace movement

If Demiurge/session state exposes relevant workspace roots or refs, call
`workspace-diff` and summarize:

1. no diff,
2. diff exists but task not terminal,
3. diff exists and task is terminal,
4. workspace root missing / unavailable.

First version only needs a compact changed-path count plus top few paths.

## Implementation Plan

### WP1: Introduce an operator-focused command

Add a new CLI command in `crates/aos-cli`:

1. `task-diagnose` command module
2. argument parsing for task/session/event-hash modes
3. shared control client plumbing

This command should compose existing control verbs, not invent new daemon APIs.

### WP2: Reuse and extend trace diagnosis

Factor shared diagnosis helpers out of the current `trace-diagnose` command or
call through to the same library-level classifier.

Current assets to reuse:

1. `crates/aos-cli/src/commands/trace_diagnose.rs`
2. `aos_host::trace::diagnose_trace`

### WP3: Add a task-oriented aggregator

Build one aggregator function that:

1. runs `trace-get`,
2. fetches workflow state,
3. fetches state lists,
4. optionally fetches workspace diff,
5. returns one typed JSON object for rendering.

Suggested file target:

1. `crates/aos-cli/src/commands/task_diagnose.rs`
2. optional shared helper module under `crates/aos-cli/src/commands/task_diag.rs`

### WP4: Render concise human output

Make the non-JSON view short and decisive:

1. no raw dump by default,
2. one likely cause,
3. one next action,
4. expandable detail in JSON mode.

### WP5: Add integration coverage

Add CLI/integration tests that cover:

1. waiting on receipt,
2. policy deny,
3. capability deny,
4. adapter timeout,
5. missing keyed state,
6. workspace diff present.

## Suggested JSON Result Shape

```json
{
  "identity": {
    "task_id": "...",
    "session_id": "...",
    "event_hash": "sha256:..."
  },
  "diagnosis": {
    "terminal_state": "waiting",
    "cause": "awaiting_host_receipt",
    "hint": "inspect host.exec adapter health"
  },
  "task_state": {},
  "session_state": {},
  "trace": {
    "live_wait": {},
    "last_intent_hash": "sha256:..."
  },
  "population": {
    "demiurge_cells": 0,
    "session_cells": 0
  },
  "workspace": {
    "available": true,
    "change_count": 3,
    "paths": ["src/main.rs", "README.md"]
  }
}
```

## Acceptance Criteria

1. An operator can diagnose one stuck Demiurge task with a single CLI command.
2. The command uses existing control-plane verbs only; no new kernel/runtime
   semantics are required.
3. The human output names one likely blocker and one next action.
4. JSON output includes trace, task state, session state, and workspace summary.
5. The feature ships before further governance/operator surface expansion.

## Follow-Ups

1. Add `aos task-list --waiting`.
2. Add HTTP/UI wrapper over the same typed response.
3. Add links from `trace-summary` output into per-task diagnosis.
