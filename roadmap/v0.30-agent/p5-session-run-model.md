# P5: Session and Run Model

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (context, traces, interruption, and Demiurge will land against the wrong lifecycle boundary)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`

## Goal

Separate durable session ownership from per-run execution state.

Primary outcome:

1. durable session status is distinct from run lifecycle,
2. one session can own zero or more runs over time,
3. transcript and future context state live at session scope,
4. tool batches, active effects, context plans, and output live at run scope,
5. evented `SessionWorkflow` and direct library wrapping preserve the same semantics.

## Current Fit

This is the most important model cleanup before the context engine.

Today:

1. `SessionLifecycle` mixes session state and run state.
2. terminal run states can transition back to `Running`.
3. `SessionState` stores durable session config, active run config, active tool batch, transcript refs, host-session handles, pending effects, and operator queues together.
4. `RunRequested` starts a run and clears `conversation_message_refs`.
5. `clear_active_run()` clears run fields and transcript refs together.
6. Demiurge still treats task id as the whole session story and converts `WaitingInput` into `RunCompleted`.

That shape works for narrow one-shot task launchers. It is not good enough for multi-run sessions, context state, traces, or interruption.

## Design Stance

### 1) Session owns durable identity

Session state should own:

1. session id and metadata,
2. session status,
3. durable transcript/history,
4. durable context state,
5. policy/config defaults,
6. attached resources such as host session handles,
7. run roster or bounded run history.

Example session statuses:

1. open,
2. paused,
3. archived,
4. expired,
5. closed.

Exact names can change. The important part is that completed or failed runs do not make the session itself terminal.

### 2) Run owns one execution attempt

Run state should own:

1. run id,
2. run lifecycle,
3. active input refs,
4. selected context plan,
5. current tool batch state,
6. active pending effects,
7. final output and outcome,
8. trace/report refs.

Example run lifecycles:

1. queued,
2. running,
3. waiting-input,
4. completed,
5. failed,
6. cancelled,
7. interrupted.

### 3) Transcript is durable session state

The current run-start behavior that clears `conversation_message_refs` is the wrong long-term default.

The new model should distinguish:

1. durable session transcript/history,
2. run input refs,
3. run-selected context refs,
4. run-produced output refs,
5. compaction/summary refs.

### 4) Keep evented and direct composition aligned

Worlds should be able to choose:

1. evented composition through `aos.agent/SessionWorkflow@1`,
2. direct library composition inside a larger world workflow.

Both paths should use the same contract semantics.

### 5) Migrate Demiurge without preserving the old confusion

Demiurge can keep task submission as public sugar, but internally it should map a task onto:

1. session create/open,
2. host/session attachment if needed,
3. first run start,
4. run completion observation,
5. task completion.

`task_id == session_id` may remain as a compatibility convenience for now, but the code should stop assuming that a task is the only thing a session can ever represent.

## Scope

### [ ] 1) Split contracts

Add separate contracts for:

1. `SessionStatus`,
2. `RunLifecycle`,
3. durable `SessionState` fields,
4. active/current `RunState` fields,
5. run outcome/failure details.

Retire or narrow `SessionLifecycle` once migration is complete.

### [ ] 2) Add explicit session operations

Add first-class ingress operations for:

1. create/open session,
2. update session config/defaults,
3. attach/update host session resource,
4. append user input,
5. start run,
6. continue run,
7. complete/fail/cancel run,
8. pause/resume session,
9. archive/expire/close session,
10. reset context without discarding session identity.

The exact event names can change, but the semantic separation should not.

### [ ] 3) Move active execution into run state

Move or re-scope:

1. `active_run_id`,
2. `active_run_config`,
3. `active_tool_batch`,
4. run pending effects,
5. queued LLM turn refs,
6. pending follow-up turn,
7. last output ref.

The session may expose `current_run`, but the fields should be run-scoped.

### [ ] 4) Preserve deterministic replay

Add focused tests for:

1. one session with no runs,
2. one session with multiple sequential runs,
3. failed run followed by a later run in the same open session,
4. paused session with no active run,
5. archived/closed session rejecting new run starts,
6. replay from genesis producing byte-identical state.

### [ ] 5) Update telemetry

Add separate events for:

1. session update/status changes,
2. run lifecycle changes,
3. run outcome,
4. current run inspection,
5. bounded run history inspection.

This prepares the ground for P7 run traces.

### [ ] 6) Update Demiurge and fixtures

Required outcome:

1. Demiurge task ingress maps onto explicit session/run operations,
2. existing task-driven behavior remains functional,
3. a fixture proves one durable session can run multiple turns/tasks,
4. evented and direct reducer helper paths preserve the same model.

## Non-Goals

P5 does **not** attempt:

1. the context engine itself,
2. run traces beyond the fields needed to attach them later,
3. final interrupt/steer behavior,
4. subagent trees,
5. multi-tenant server API design,
6. final UI/operator product decisions.

## Acceptance Criteria

1. A durable session can exist with zero or more runs.
2. Run lifecycle transitions no longer double as session status transitions.
3. Transcript/history is explicitly session-scoped.
4. Active effects and tool batches are explicitly run-scoped.
5. Demiurge or a focused fixture proves session continuity across multiple runs.
6. Existing one-shot agent evals still work through the new model.
