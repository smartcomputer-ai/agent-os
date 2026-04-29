# P5: Session and Run Model

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (context, traces, interruption, and Demiurge will land against the wrong lifecycle boundary)  
**Status**: Core SDK and Demiurge integration complete; `aos-harness-py` E2E fixture still pending  
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`

## Goal

Separate durable session ownership from per-run execution state.

Primary outcome:

1. durable session status is distinct from run lifecycle,
2. one session can own zero or more runs over time,
3. transcript and future context state live at session scope,
4. tool batches, active effects, context plans, and output live at run scope,
5. every run records an extensible cause/provenance envelope,
6. evented `SessionWorkflow` and direct library wrapping preserve the same semantics.

## Current Fit

This is the most important model cleanup before the context engine.

Today:

1. `SessionLifecycle` mixes session state and run state.
2. terminal run states can transition back to `Running`.
3. `SessionState` stores durable session config, active run config, active tool batch, transcript refs, host-session handles, pending effects, and operator queues together.
4. `RunRequested` starts a run and clears `conversation_message_refs`.
5. `clear_active_run()` clears run fields and transcript refs together.
6. Demiurge still treats task id as the whole session story and converts `WaitingInput` into `RunCompleted`.

That shape works for narrow one-shot task launchers. It is not good enough for multi-run sessions,
context state, non-user run triggers, traces, or interruption.

## Design Stance

### 1) Session owns durable identity

Session state should own:

1. session id and metadata,
2. session status,
3. durable transcript/history,
4. durable context state,
5. config/defaults,
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
3. run cause/provenance,
4. active input refs,
5. selected context plan,
6. current tool batch state,
7. active pending effects,
8. final output and outcome,
9. trace/report refs.

Example run lifecycles:

1. queued,
2. running,
3. waiting-input,
4. completed,
5. failed,
6. cancelled,
7. interrupted.

### 3) Run cause records provenance, not product semantics

`RunRequested { input_ref }` is too chat-shaped as the only start story.

The SDK should add a stable `RunCause` contract, but it should not add a closed enum of every
possible product trigger. Native AOS workflows already route domain events through
`routing.subscriptions`; `RunCause` records why a run exists so context planning, traces, and
operators can inspect provenance.

Illustrative shape:

```rust
pub struct RunCause {
    pub kind: String,
    pub origin: RunCauseOrigin,
    pub input_refs: Vec<String>,
    pub payload_schema: Option<String>,
    pub payload_ref: Option<String>,
    pub subject_refs: Vec<CauseRef>,
}

pub enum RunCauseOrigin {
    DirectIngress { source: String, request_ref: Option<String> },
    DomainEvent { schema: String, event_ref: Option<String>, key: Option<String> },
    Internal { reason: String, ref_: Option<String> },
}

pub struct CauseRef {
    pub kind: String,
    pub id: String,
    pub ref_: Option<String>,
}
```

Names can change. The important part is that cause `kind` and `CauseRef.kind` are namespaced/open
strings, so worlds can attach causes such as user input, work item readiness, timer wake, review
request, or operator action without expanding the SDK API.

Core reducer behavior should only depend on generic properties such as input refs and whether a run
can start. Product-specific cause kinds belong to embedding workflows.

### 4) Transcript is durable session state

The current run-start behavior that clears `conversation_message_refs` is the wrong long-term default.

The new model should distinguish:

1. durable session transcript/history,
2. run input refs,
3. run-selected context refs,
4. run-produced output refs,
5. compaction/summary refs.

### 5) Keep evented and direct composition aligned

Worlds should be able to choose:

1. evented composition through `aos.agent/SessionWorkflow@1`,
2. direct library composition inside a larger world workflow.

Both paths should use the same contract semantics.

### 6) Migrate Demiurge without preserving the old confusion

Demiurge can keep task submission as public sugar, but internally it should map a task onto:

1. session create/open,
2. host/session attachment if needed,
3. first run start,
4. run completion observation,
5. task completion.

`task_id == session_id` may remain as a compatibility convenience for now, but the code should stop
assuming that a task is the only thing a session can ever represent. Demiurge task ingress should
populate `RunCause` rather than relying on implicit user-turn semantics.

## Scope

### [x] 1) Split contracts

Add separate contracts for:

1. `SessionStatus`,
2. `RunLifecycle`,
3. durable `SessionState` fields,
4. active/current `RunState` fields,
5. open-ended `RunCause`, `RunCauseOrigin`, and `CauseRef` fields,
6. run outcome/failure details.

Retire or narrow `SessionLifecycle` once migration is complete.

Done:

1. added `SessionStatus` for durable session state.
2. added `RunLifecycle` for per-run execution state.
3. added `RunState` and `RunRecord` with open-ended `RunCause`.
4. added `RunCauseOrigin`, `CauseRef`, `RunOutcome`, and `RunFailure`.
5. kept legacy `SessionLifecycle` as a compatibility mirror for existing consumers while new run/session contracts become the semantic model.

### [x] 2) Add explicit session operations

Add first-class ingress operations for:

1. create/open session,
2. update session config/defaults,
3. attach/update host session resource,
4. append user input,
5. start run with an explicit `RunCause`,
6. continue run,
7. complete/fail/cancel run,
8. pause/resume session,
9. archive/expire/close session,
10. reset context without discarding session identity.

The exact event names can change, but the semantic separation should not.

Done:

1. added `SessionOpened`, `SessionConfigUpdated`, `SessionPaused`, `SessionResumed`, `SessionArchived`, `SessionExpired`, and `SessionClosed`.
2. added `RunStartRequested { cause, run_overrides }` while preserving legacy `RunRequested` as user-input sugar.
3. retained host session attachment via `HostSessionUpdated`.
4. retained run completion/failure/cancel ingress and records terminal run outcome in run history.
5. closed, paused, archived, and expired sessions reject new run starts independently from run lifecycle.

### [x] 3) Move active execution into run state

Move or re-scope:

1. `active_run_id`,
2. `active_run_config`,
3. `active_tool_batch`,
4. run pending effects,
5. queued LLM turn refs,
6. pending follow-up turn,
7. last output ref.

The session may expose `current_run`, but the fields should be run-scoped.

Done:

1. `current_run` stores run id, lifecycle, cause, config, input refs, active tool batch, pending effects, queued LLM refs, follow-up state, last output, and in-flight count.
2. existing reducer fields remain as compatibility mirrors for this cut, but are synced into `current_run` after each reduction.
3. terminal runs move into bounded `run_history` records with outcome data.
4. durable transcript refs stay at session scope and are no longer cleared when a run ends.

### [x] 4) Preserve deterministic replay

Add focused tests for:

1. one session with no runs,
2. one session with multiple sequential runs,
3. failed run followed by a later run in the same open session,
4. paused session with no active run,
5. archived/closed session rejecting new run starts,
6. a run started from a non-user domain-event cause,
7. replay from genesis producing byte-identical state.

Use Rust unit tests for reducer transition invariants and `aos-harness-py` workflow fixtures for
end-to-end session/run stories. Live `aos-agent-eval` coverage should only prove provider/tool
acceptance still works.

Done:

1. added Rust reducer tests for session-with-no-runs, multiple sequential runs, failed-then-later run, paused/closed session boundaries, and non-user domain-event causes.
2. generated AIR is checked against Rust source.
3. downstream `aos-agent-eval`, `aos-smoke`, and Demiurge compile against the new contracts.
4. `aos-harness-py` E2E coverage is still pending because that package does not currently have an agent session fixture lane.

### [x] 5) Update telemetry

Add separate events for:

1. session update/status changes,
2. run lifecycle changes,
3. run cause/provenance,
4. run outcome,
5. current run inspection,
6. bounded run history inspection.

This prepares the ground for P7 run traces.

Done:

1. added `SessionStatusChanged` and `RunLifecycleChanged`.
2. run lifecycle telemetry carries run cause/provenance and output ref.
3. session lifecycle telemetry remains for compatibility while P7 can build on the new run/session events.
4. current-run and run-history inspection data is present in `SessionState`.

### [x] 6) Update Demiurge and fixtures

Required outcome:

1. Demiurge task ingress maps onto explicit session/run operations,
2. existing task-driven behavior remains functional,
3. Demiurge populates `RunCause` explicitly,
4. a fixture proves one durable session can run multiple turns/tasks,
5. evented and direct reducer helper paths preserve the same model.

Done:

1. Demiurge handoff now emits `RunStartRequested` through the helper path.
2. Demiurge populates `RunCause` with `demiurge/task_submitted` provenance and task subject refs.
3. existing Demiurge task-driven behavior still compiles and passes its unit tests.
4. focused SDK reducer tests prove session continuity across multiple runs and direct domain-event causes.
5. `aos-harness-py` fixture coverage remains the explicit follow-up from this P5 cut.

## Non-Goals

P5 does **not** attempt:

1. the context engine itself,
2. run traces beyond the fields needed to attach them later,
3. final interrupt/steer behavior,
4. subagent trees,
5. timer-chain scheduling, heartbeat semantics, or external trigger services,
6. factory work-item, agenda, worker-invocation, review, or test workflows,
7. policy/capability gating or approval semantics,
8. multi-tenant server API design,
9. final UI/operator product decisions.

## Acceptance Criteria

### [x] 1) A durable session can exist with zero or more runs.

Covered by `SessionStatus`, optional `current_run`, `run_history`, and reducer tests for a session
with no runs plus sequential runs in one session.

### [x] 2) Run lifecycle transitions no longer double as session status transitions.

Covered by separate `SessionStatus` and `RunLifecycle` contracts. Legacy `SessionLifecycle` remains
only as a compatibility mirror for existing event consumers.

### [x] 3) Transcript/history is explicitly session-scoped.

Covered by `transcript_message_refs` on `SessionState`; `clear_active_run()` no longer clears the
durable transcript.

### [x] 4) Active effects and tool batches are explicitly run-scoped.

Covered by `RunState` fields for active tool batch, pending effects, pending blob work, queued LLM
refs, follow-up turn, last output, tool materialization, and in-flight count. Existing session-level
fields remain as reducer compatibility mirrors in this cut.

### [x] 5) Every run records an open-ended `RunCause` without requiring SDK changes for product-specific triggers.

Covered by `RunCause`, `RunCauseOrigin`, and `CauseRef` with namespaced/open string `kind` fields.

### [x] 6) A deterministic fixture proves a non-user/domain-event cause can start a run through normal session ingress.

Covered by the Rust reducer test for `RunStartRequested` with a `DomainEvent` origin.

### [x] 7) Demiurge or a focused fixture proves session continuity across multiple runs.

Covered by focused SDK reducer tests for multiple sequential runs and failed-run-then-later-run in
the same durable session. Demiurge now populates `RunCause` for task handoff.

### [ ] 8) A deterministic `aos-harness-py` fixture proves multi-run session continuity without provider credentials.

Pending. `aos-harness-py` currently has no dedicated agent session fixture lane; this remains the
explicit follow-up for P5 end-to-end coverage.

### [x] 9) Existing one-shot live agent evals still work through the new model.

Covered by `cargo check -p aos-agent-eval` against the new contracts; live provider execution is
still the acceptance lane rather than a unit test.
