# P6: Session Management and Context Scoping

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (the context engine may land against the wrong lifecycle model, and `SessionLifecycle` will continue to blur durable session ownership with per-run execution state)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.16-factory/p5-context-engine.md`, `roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md`, `roadmap/v0.13-demiurge2/p6-goal-manager.md`

## Goal

Clarify and improve the session model so it composes cleanly with the context engine.

Primary outcome:

1. durable session ownership is separated from per-run lifecycle,
2. context scope is explicit across world, session, and run,
3. multi-run sessions become a first-class concept,
4. worlds such as Demiurge can choose the cleanest integration style:
   - direct library wrapping, or
   - evented `SessionWorkflow` composition.

## Problem Statement

The current model is functional but semantically muddy.

Today:

1. a session often behaves like "one task = one session",
2. `SessionLifecycle` is effectively a run lifecycle,
3. terminal run states can transition back to `Running`,
4. transcript state, host-session handles, and future context state all live under the same umbrella,
5. there is no crisp answer to where durable session identity ends and one run begins.

That is manageable for narrow task launchers.
It is not the right shape for a context-aware factory agent.

## Design Stance

### 1) Separate durable session state from per-run lifecycle

The base model should distinguish:

1. session status
   - durable identity and ownership,
   - examples: open, paused, archived, expired, closed.
2. run lifecycle
   - one execution attempt inside a session,
   - examples: queued, running, waiting-input, completed, failed, cancelled.

This is the single most important semantic cleanup.

### 2) Context is not equal to session

The context model should use explicit scopes:

1. world scope
   - shared implementation inputs,
2. session scope
   - durable conversational memory and context state,
3. run scope
   - the context plan and active execution state for one run.

The session owns the durable state that the context engine can read.
The run owns the materialized context plan for one execution.

### 3) A session should support multiple runs by default

A durable session should be able to handle:

1. new user input,
2. follow-up turns,
3. review-and-continue loops,
4. reopen after pause,
5. archive or reset when truly finished.

This should not require inventing a new session id for every turn.

### 4) Keep the adapter style flexible

The durable session model should not assume that every consumer wants a separate workflow module boundary.

We should support:

1. direct library wrapping for worlds that want one cohesive reducer,
2. evented workflow composition for worlds that want loose coupling.

The contracts should make both styles possible.

### 5) Make telemetry match the new model

Once session and run are separate, telemetry should be separate too.

That likely means:

1. session-level update events,
2. run-level lifecycle events,
3. context-plan/report visibility tied to runs,
4. clearer operator diagnosis when a session is healthy but its current run is waiting.

## Proposed Model

### Session owns

1. session id and metadata,
2. durable transcript history,
3. durable context-engine state,
4. optional attached resources such as host-session handles,
5. policy/config defaults,
6. the roster or history of runs.

### Run owns

1. run id,
2. active lifecycle,
3. selected context plan,
4. current tool batch state,
5. active pending effects,
6. final output and outcome.

### World owns

1. implementation-level context sources,
2. skill activation policy,
3. workspace or repo loading strategy,
4. orchestration over multiple sessions if needed.

## Scope

### [ ] 1) Split session status from run lifecycle in contracts

Refactor the core contracts so that durable session state and current run state are separate.

Required outcome:

1. no more overloading one enum to mean both things,
2. a durable session can be open with no active run,
3. completed or failed runs do not imply that the session itself is terminal.

### [ ] 2) Add first-class multi-run session semantics

Add explicit operations for:

1. session create/open,
2. append input,
3. start run,
4. continue run,
5. pause/resume,
6. archive/expire/close,
7. optional context reset without discarding session identity.

### [ ] 3) Store context state at session scope

The context engine should read and update durable session-scoped context state.

Required outcome:

1. context summaries and accepted facts live with the session,
2. a run can record the exact context plan it used,
3. future runs can build from updated session state without reinterpreting the whole transcript every time.

### [ ] 4) Improve event and inspection surfaces

Add clearer observability for the new model.

Recommended direction:

1. `RunLifecycleChanged`
2. `SessionUpdated`
3. inspectable `current_run`
4. inspectable `last_context_report`

Exact names can change.
The separation should not.

### [ ] 5) Update Demiurge integration strategy

Demiurge should stop assuming "task id equals entire session story forever."

Recommended near-term direction:

1. keep task ingress as public sugar where useful,
2. map that onto explicit session creation/open plus first run start,
3. allow direct library wrapping if that yields a cleaner reducer than cross-module event choreography.

### [ ] 6) Add harness coverage for session continuity

We need tests that prove:

1. one session can handle multiple runs,
2. session-scoped context survives between runs,
3. archived/expired sessions behave deterministically,
4. evented and direct embedding paths both preserve the same semantics.

## Non-Goals

P6 does **not** attempt:

1. subagent trees or session-to-session delegation,
2. multi-tenant server API design,
3. final UI/operator product decisions,
4. skill packaging,
5. hosted fleet scheduling.

## Deliverables

1. Contracts that separate durable session status from run lifecycle.
2. Session-scoped context state model.
3. Multi-run session control operations.
4. Clearer session and run telemetry.
5. Demiurge migration plan aligned with the new model.

## Acceptance Criteria

1. A durable session can exist with zero or more runs over time.
2. Context state is explicitly session-scoped, while context plans are explicitly run-scoped.
3. Run lifecycle transitions no longer double as session status transitions.
4. Demiurge or a focused fixture proves session continuity across multiple runs.
5. The same session semantics can be used through direct library wrapping or evented `SessionWorkflow` composition.

## Recommended Implementation Order

1. define the session/run contract split,
2. update core state and reducer helpers,
3. add session-scoped context state integration,
4. add improved telemetry and inspection surfaces,
5. migrate Demiurge to the new model,
6. add continuity and reopen coverage in the harness/test lanes.
