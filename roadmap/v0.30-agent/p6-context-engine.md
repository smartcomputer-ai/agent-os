# P6: Overridable Context Engine

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (skills, memories, repo instructions, and world-specific behavior will keep accreting around ad hoc prompt refs)  
**Status**: Core SDK work complete; `aos-harness-py` E2E fixture deferred  
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.30-agent/p5-session-run-model.md`

## Goal

Introduce a first-class, inspectable context engine API for `aos-agent`.

Primary outcome:

1. context is assembled explicitly and deterministically for each run,
2. source-specific concerns feed the engine through normalized inputs,
3. context plans and reports are visible for tests and operators,
4. worlds that link `aos-agent` can replace or extend context selection,
5. legacy prompt-ref behavior survives as the smallest default engine.

## Current Fit

The current prompt surface is too thin.

Today we mostly have:

1. `SessionConfig.default_prompt_refs`,
2. `RunConfig.prompt_refs`,
3. `conversation_message_refs`,
4. world-specific code that chooses what to put in those lists.

That works for static prompts and narrow smoke flows.

It is not enough for:

1. budgeted context assembly,
2. context-source inspection,
3. deterministic compaction decisions,
4. repo/bootstrap file loading,
5. skill and memory integration,
6. world-specific selection of what the model may see.

This item now depends on P5 because context needs the session/run split first:

1. durable context state belongs at session scope,
2. materialized context plans belong at run scope,
3. context reports should be tied to run traces.

## Design Stance

### 1) Context is built per run

The engine should build a run-scoped plan from:

1. world-level pinned inputs,
2. session-level durable state,
3. current run input,
4. run cause/provenance,
5. transcript/history state,
6. optional implementation-level sources such as domain state summaries, repo files, workspace refs, memory refs, or resolved skills.

The plan is per-run. The state informing it can live longer.

### 2) Keep planning deterministic

Context planning must not hide effectful work.

Effectful work such as:

1. LLM summarization,
2. transcript compaction,
3. remote knowledge fetch,
4. embedding index updates,

must be explicit runtime work. The context engine can request or consume already-materialized refs, but it should not perform hidden I/O.

### 3) Keep source loading outside core

The engine should accept normalized inputs such as:

1. pinned refs,
2. run cause refs,
3. transcript segments,
4. summary refs,
5. extracted facts,
6. domain-event or workflow-state summary refs,
7. workspace/artifact refs,
8. repo/bootstrap file refs,
9. memory refs,
10. skill-provided refs.

It should not care whether those came from local files, workspaces, CAS blobs, static assets, or future registries.
It also should not care whether a domain-state summary represents a work item, review, test run,
chat channel, or another application workflow.

### 4) Support override without pretending WASM is dynamic

There are two composition styles:

1. evented `SessionWorkflow`,
2. direct library wrapping by an embedding world.

The evented reusable workflow can only use deterministic code compiled into that workflow. For that path, the default engine should be configurable through normalized inputs and session/run config.

Worlds that need a genuinely custom engine should use direct library composition or a wrapper workflow that links `aos-agent` helpers and calls the custom engine before requesting the LLM turn.

### 5) Context reports are first-class

The engine must explain what it did.

At minimum the report should include:

1. selected inputs,
2. dropped inputs,
3. budget reasoning,
4. compaction recommendations,
5. unresolved prerequisites,
6. selection decisions that affected visibility.

## Proposed Contracts

Illustrative shapes:

```rust
pub struct ContextRequest<'a> {
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub run_cause: Option<&'a RunCause>,
    pub budget: ContextBudget,
    pub world_inputs: &'a [ContextInput],
    pub session_context: &'a SessionContextState,
    pub transcript_refs: &'a [String],
    pub run_input_refs: &'a [String],
}

pub struct ContextPlan {
    pub selected_refs: Vec<String>,
    pub selected_inputs: Vec<ContextSelection>,
    pub pending_actions: Vec<ContextAction>,
    pub report: ContextReport,
}

pub trait ContextEngine {
    type Error;

    fn observe(
        &self,
        state: &mut SessionContextState,
        event: ContextEvent,
    ) -> Result<(), Self::Error>;

    fn build_plan(
        &self,
        request: ContextRequest<'_>,
    ) -> Result<ContextPlan, Self::Error>;
}
```

The exact names can change.

## Scope

### [x] 1) Define shared context contracts

Add core types for:

1. context input identity,
2. source and scope metadata,
3. priority and budget hints,
4. deterministic selection result,
5. context action requests,
6. context report payloads,
7. observation events,
8. open input kind/correlation metadata for domain workflows,
9. session-scoped context state.

These should be source-agnostic and live in the `aos-agent` library surface.

Done:

1. added source-agnostic `ContextInput`, `ContextInputScope`, `ContextInputKind`, `ContextPriority`, and `ContextBudget`.
2. added deterministic `ContextSelection`, `ContextAction`, `ContextReport`, and `ContextPlan`.
3. added `ContextObservation` for feeding completed summaries and pinned/removed inputs back into session state.
4. added `SessionContextState` with pinned inputs, summary refs, and last report.
5. exported context contracts from the `aos-agent` library and generated AIR schemas.

### [x] 2) Add the run-planning hook

The session runtime should stop treating prompt refs as the entire context story.

Required outcome:

1. run start calls the context engine,
2. tool follow-up turns call the context engine,
3. selected refs feed `llm.generate`,
4. context reports are recorded for inspection and P7 traces,
5. run cause/provenance can be presented as an input,
6. old prompt refs become inputs to the default engine.

Done:

1. `dispatch_pending_llm_turn` now builds a context plan before emitting `sys/llm.generate@1`.
2. selected context refs feed `LlmGenerateParams.message_refs`; prompt refs are no longer appended in the LLM primitive.
3. `RunState.context_plan` records the per-run plan for inspection.
4. `SessionContextState.last_report` records the latest context report.
5. run cause payload and subject refs are normalized into context inputs.
6. legacy prompt refs are normalized into default-engine inputs.

### [x] 3) Provide a minimal default engine

The default engine should be deliberately conservative:

1. include pinned/default prompt refs,
2. include run cause refs when present,
3. include current run input refs,
4. include recent transcript refs,
5. include completed summary refs if present,
6. respect simple budget bounds,
7. report dropped inputs.

This is a reference implementation, not the final factory brain.

Done:

1. added `ContextEngine` plus `DefaultContextEngine`.
2. default selection includes prompt refs, pinned inputs, summary refs, run cause refs, transcript refs, and current turn refs.
3. default selection deduplicates refs deterministically.
4. simple `ContextBudget.max_refs` limits non-required inputs and records dropped selections.
5. default reports include selected/dropped counts and selection decisions.

### [x] 4) Add compaction hooks

The engine contract should support compaction without hiding summarization.

Required outcome:

1. engine can signal that a summary is desirable or required,
2. summary generation remains explicit runtime work,
3. completed summary refs can be observed back into session context state,
4. tests can assert compaction recommendations deterministically.

Done:

1. default engine emits `ContextActionKind::Compact` when budgeted selection drops inputs.
2. compaction actions are recommendations only; summarization remains explicit runtime work.
3. `ContextObservation::SummaryCompleted` adds completed summary refs to session context state.
4. deterministic unit tests assert budget drops and compaction recommendations.

### [~] 5) Add inspection and tests

Required surfaces:

1. last context plan/report per run,
2. deterministic unit tests for planning behavior,
3. an `aos-harness-py` workflow fixture proving source-agnostic inputs,
4. a fixture proving prompt-ref compatibility through the default engine.

The Python fixture should script LLM and blob receipts rather than depending on live provider
output. See `roadmap/v0.30-agent/p10-agent-sdk-testing.md` for the harness direction.

Done:

1. pure context-engine tests cover prompt-ref ordering, domain cause refs, budget drops, and compaction recommendations.
2. reducer tests cover prompt-ref compatibility through the default context engine.
3. run-level context plans and session-level last reports are inspectable in state.
4. a reducer-level custom-engine proof covers source-agnostic pinned inputs and report inspection through normal LLM dispatch.

Remaining:

1. deferred: add an `aos-harness-py` workflow fixture for prompt-ref compatibility and report inspection.
2. deferred: add an E2E source-agnostic fixture that scripts LLM/blob receipts end to end.

### [x] 6) Prove override

Prove that a consumer can:

1. link `aos-agent`,
2. provide a custom context selection implementation,
3. add implementation-specific inputs such as repo bootstrap files and domain-state summary refs,
4. still reuse base session/run and tool orchestration helpers.

This can be Demiurge or a focused linked-library fixture.

Done:

1. added `dispatch_pending_llm_turn_with_engine` so direct library consumers can reuse pending-turn LLM/tool orchestration with a custom `ContextEngine`.
2. added `build_context_for_turn_with_engine` so wrapper workflows can build/apply a custom context plan while preserving session/run state updates.
3. added a focused `RepoBootstrapFirstEngine` reducer test that selects a pinned repo bootstrap input, drops transcript refs, records a custom context report, and emits a normal `sys/llm.generate@1` request with the selected refs.
4. the proof uses generic context input metadata only; no Demiurge or software-factory-specific SDK variants were added.

## Non-Goals

P6 does **not** attempt:

1. final skill selection,
2. subagent context sharing,
3. semantic search or embeddings infrastructure,
4. hidden automatic LLM summarization,
5. policy/capability gating or approval semantics,
6. factory work-item or scheduler workflows,
7. final run trace UI.

## Acceptance Criteria

1. [x] Context inputs are source-agnostic.
2. [x] The default engine preserves legacy prompt-ref behavior.
3. [x] Context state is session-scoped and context plans are run-scoped.
4. [x] Each run records an inspectable context report.
5. [x] Budgeted selection and dropped-input reasoning are deterministic and testable.
6. [x] Run cause and domain/artifact refs can be supplied as normalized inputs without adding product-specific SDK variants.
7. [ ] Deferred: a deterministic `aos-harness-py` fixture proves prompt-ref compatibility and report inspection.
8. [x] A direct-wrapper fixture or Demiurge proves custom context selection without forking tool/session control flow.
