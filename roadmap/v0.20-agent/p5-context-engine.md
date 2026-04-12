# P5: Overridable Context Engine

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (session semantics, skills, and world-specific agent behavior will keep accreting around ad hoc prompt refs instead of a coherent context model)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.16-factory/p4-tool-bundle-refactoring.md`, `roadmap/v0.16-factory/factory.md`

## Goal

Introduce a first-class, overridable context engine API for `aos-agent`.

Primary outcome:

1. context is assembled explicitly and deterministically for each run,
2. worlds that link `aos-agent` can replace or extend the context engine,
3. source-specific concerns such as workspaces, repo files, memories, and future skills feed the engine through generic inputs rather than hardcoded branches,
4. context planning becomes inspectable and testable instead of being implicit in `prompt_refs`.

## Problem Statement

The current prompt surface is too thin for the next phase.

Today we mostly have:

1. `default_prompt_refs`,
2. `prompt_refs`,
3. transcript message refs,
4. world-specific code that decides what to stuff into those lists.

That is enough for:

1. basic static prompts,
2. simple transcript continuation,
3. narrow smoke flows.

It is not enough for:

1. budgeted context assembly,
2. context-source inspection,
3. deterministic compaction decisions,
4. reusable bootstrap-file loading,
5. skill and memory integration,
6. world-specific policy over what is visible to the model.

## Design Stance

### 1) Context is built per run, not stored as one giant session blob

The engine should build a run-scoped context plan from multiple inputs:

1. world-level pinned inputs,
2. session-level durable state,
3. current turn input,
4. optional implementation-level sources such as workspace or repo files.

The assembled context plan is per-run.
The backing state that informs it may live much longer.

### 2) The engine API must be overridable by embedding worlds

The main requirement is not a clever built-in heuristic.
It is a stable seam.

That means:

1. `aos-agent` should define the engine trait and shared request/plan/report types,
2. the default engine should be small and conservative,
3. a world that links `aos-agent` should be able to provide its own engine implementation without forking the session kernel.

### 3) Separate deterministic planning from effectful synthesis

Context planning should stay deterministic.

Effectful work such as:

1. generating a summary,
2. compacting a transcript with an LLM,
3. fetching remote knowledge,
4. building an embedding index,

should not be hidden inside the engine boundary.

Instead:

1. the engine should be able to request or consume already-materialized refs,
2. compaction/summarization tasks should be explicit runtime work driven by the embedding world or a follow-on session workflow step.

### 4) Source-specific loading should stay outside core

The engine should accept normalized context inputs such as:

1. pinned refs,
2. transcript segments,
3. summary refs,
4. extracted facts,
5. repo/bootstrap file refs,
6. skill-provided refs.

It should not care whether those came from:

1. workspaces,
2. local files,
3. blobs uploaded by a wrapper,
4. static code constants.

### 5) Context reports are first-class

The engine must explain what it did.

At minimum it should report:

1. selected inputs,
2. dropped inputs,
3. budget reasoning,
4. compaction decisions,
5. unresolved prerequisites.

Without that report, context behavior will remain hard to debug and hard to validate.

## Proposed Model

### Context scopes

The context engine should operate across three scopes:

1. world scope
   - durable shared inputs for an agent implementation,
   - examples: system prompt, policy prompt, repo bootstrap refs, skill catalogs.
2. session scope
   - durable conversational state,
   - examples: transcript summaries, accepted facts, open threads, user preferences.
3. run scope
   - the exact set of inputs chosen for one LLM turn or tool-follow-up turn.

### Core types

Illustrative shapes:

```rust
pub struct ContextRequest<'a, S> {
    pub session_id: &'a SessionId,
    pub run_id: Option<&'a RunId>,
    pub budget: ContextBudget,
    pub world_inputs: &'a [ContextInput],
    pub session_state: &'a S,
    pub turn_input_refs: &'a [String],
}

pub struct ContextPlan {
    pub selected_refs: Vec<String>,
    pub selected_inputs: Vec<ContextSelection>,
    pub pending_actions: Vec<ContextAction>,
    pub report: ContextReport,
}

pub trait ContextEngine {
    type State;
    type Error;

    fn observe(&self, state: &mut Self::State, event: ContextEvent) -> Result<(), Self::Error>;
    fn build_plan(
        &self,
        request: ContextRequest<'_, Self::State>,
    ) -> Result<ContextPlan, Self::Error>;
}
```

The exact names can change.
The important part is the seam.

## Scope

### [ ] 1) Define the shared context-engine contracts

Add core types for:

1. context inputs,
2. scope and priority metadata,
3. budget hints,
4. deterministic planning result,
5. inspection/report payloads,
6. observation events.

These should live in the core library surface rather than in a world-specific wrapper.

### [ ] 2) Add the engine hook to the session kernel

The session runtime should stop materializing `prompt_refs` directly as the entire context story.

Required outcome:

1. run start and follow-up turns call the engine,
2. the engine returns the refs used for `llm.generate`,
3. the session kernel records the resulting context report for inspection,
4. the old prompt-ref path becomes the default engine behavior rather than the only behavior.

### [ ] 3) Provide a small default engine

The built-in engine should intentionally be minimal:

1. include pinned world refs,
2. include recent session transcript refs,
3. include current turn refs,
4. respect simple budget bounds,
5. surface dropped inputs in the report.

This is a reference implementation, not the final factory brain.

### [ ] 4) Add explicit compaction hooks

The engine contract should support compaction without hiding effectful work.

Required outcome:

1. engine can signal that a session summary is desirable or required,
2. summary generation remains explicit runtime work,
3. completed summary refs can later be observed back into the engine state.

### [ ] 5) Add inspection and test surfaces

Context planning must be visible to both harness tests and operators.

Required surfaces:

1. last context plan/report in session state or equivalent inspection output,
2. deterministic unit tests for planning behavior,
3. world-level tests that assert context selection and compaction behavior.

### [ ] 6) Prove world override with Demiurge or a focused fixture

We should prove that a consumer world can:

1. link `aos-agent`,
2. supply a custom engine,
3. add implementation-specific inputs such as repo bootstrap files,
4. still reuse the base session runtime.

## Non-Goals

P5 does **not** attempt:

1. the final skill-selection model,
2. the final durable session-management model,
3. subagent context sharing,
4. semantic search or embeddings infrastructure,
5. hidden automatic LLM summarization inside the context engine.

## Deliverables

1. Shared `ContextEngine` API in `aos-agent`.
2. Core context request/plan/report contracts.
3. Session-kernel integration at run-planning time.
4. Minimal default engine.
5. Explicit compaction/summarization seam.
6. Proof that an embedding world can override the engine.

## Acceptance Criteria

1. A world that links `aos-agent` can provide its own context engine without forking session control flow.
2. Context inputs are source-agnostic; workspace-backed inputs are optional rather than privileged.
3. The session kernel records inspectable context reports for each run.
4. Budgeted context selection and dropped-input reasoning are deterministic and testable.
5. The legacy prompt-ref behavior still works through the default engine.

## Recommended Implementation Order

1. define the core context contracts and engine trait,
2. integrate the engine hook into run planning,
3. ship the minimal default engine,
4. add context reports and harness coverage,
5. add explicit compaction hooks,
6. prove override from Demiurge or a dedicated linked-library fixture.
