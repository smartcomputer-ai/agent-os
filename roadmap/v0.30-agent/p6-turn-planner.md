# P6: Turn Planner

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (`aos-agent` will keep splitting prompt/context/tool/skill decisions across unrelated helpers, making future context engineering harder to reason about)  
**Status**: Target shape reset; previous `ContextEngine` work is useful foundation but should be replaced, not slowly layered around  
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.30-agent/p5-session-run-model.md`

## Goal

Replace the narrow context-engine seam with a first-class, deterministic turn planner.

The planner should decide the complete LLM turn shape:

1. which system/developer/user/assistant/tool message refs are included,
2. which transcript, summary, memory, domain, runtime, and skill refs are included,
3. which tools are exposed for this turn,
4. which tools are only discoverable or intentionally omitted,
5. which turn controls are applied,
6. which follow-up actions such as compaction or materialization are requested,
7. why each inclusion, exclusion, and action happened.

This is the core context-engineering boundary for `aos-agent`.

Most serious agents are built around two decisions repeated every turn:

1. what the model can see,
2. what the model can do.

Those decisions should be planned together.

## Current Fit

The earlier P6 implementation added a useful but too-narrow context engine:

1. `ContextEngine::build_plan`,
2. `ContextRequest`,
3. `ContextPlan`,
4. `ContextInput`,
5. context reports and compaction recommendations,
6. session-scoped context state.

That moved prompt refs, transcript refs, run cause refs, summaries, and pinned inputs behind a
deterministic selector.

However, the current model still assembles an LLM turn from separate decisions:

1. context planning selects only `message_refs`,
2. tool selection is computed separately as `EffectiveToolSet`,
3. host-session readiness is encoded in generic `ToolAvailabilityRule` variants,
4. tool refs are materialized outside the planning decision,
5. `tool_choice` is chosen by workflow code,
6. skills are still imagined as later context contributions,
7. system/developer instruction layering is only implicit in the selected message refs.

That creates the wrong foundation. Tool availability, skill activation, memory selection, runtime
hints, and system/developer prompt layering are all part of the same turn-planning problem.

Because this is still experimental SDK development, do not add compatibility indirections around
the old `ContextEngine`. Replace it with the better abstraction.

## Design Stance

### 1) A turn is the unit of planning

The workflow should ask one deterministic planner for the complete next LLM turn.

The workflow should remain responsible for orchestration:

1. accepting inputs, receipts, rejections, and stream frames,
2. tracking run/session state,
3. materializing selected tool definitions,
4. emitting `sys/llm.generate@1`,
5. settling tool batches,
6. recording trace entries.

The workflow should not own context policy, skill activation, host/Fabric tool availability, prompt
layering, or memory selection.

### 2) Message refs remain the primitive

Do not add a separate system-message management subsystem.

`message_refs` can already point to JSON messages with roles such as `system`, `developer`, `user`,
`assistant`, and `tool`. The planner should output an ordered `message_refs` list. Role semantics
stay in the referenced message blobs.

The planner needs metadata about candidate refs for deterministic selection and reporting, but that
metadata should not replace the message blob format.

### 3) Planning metadata uses lanes

The planner should receive normalized candidate refs with a lane that explains how the candidate
should be considered.

Illustrative lanes:

1. `System`,
2. `Developer`,
3. `Conversation`,
4. `ToolResult`,
5. `Summary`,
6. `Memory`,
7. `Skill`,
8. `Domain`,
9. `RuntimeHint`,
10. `Custom`.

Lanes are not LLM roles. They are planner metadata for ordering, budgeting, diagnostics, and
selection.

### 4) Tools are planned with context

Tools are not just runtime wiring. Tool schemas are part of what the model sees, and selected tools
change how the model interprets the task.

The planner should select tools from explicit tool candidates and registry/profile state.

Host/Fabric-specific availability should not be a base SDK primitive. For example:

1. if no host session exists, expose only `open_session` or request host-session materialization,
2. if exactly one host session is ready, host tool mappers may use it as the implicit target,
3. if multiple host sessions are ready, selected tools may require explicit `session_id`,
4. Fabric-vs-local behavior is selected by host target config and effect routing, not a separate
   LLM tool family.

Those are planner and mapper concerns, not generic `ToolAvailabilityRule` concerns.

### 5) Skills are planner inputs

Skills should not be hidden prompt magic.

A resolved skill may contribute:

1. instruction refs,
2. short skill-card refs,
3. memory/query hints,
4. tool candidates,
5. response-format or provider-option refs,
6. activation metadata.

The planner decides whether a skill is:

1. active: full instruction refs and relevant tools are loaded,
2. discoverable: a compact skill card or lookup tool is available,
3. inactive: not included this turn.

This prevents both extremes:

1. loading every known skill into every turn,
2. hiding all capabilities behind tools the model may not know to call.

### 6) Planning remains deterministic and effect-free

The planner may consume already-materialized refs and request explicit actions.

It must not perform hidden I/O such as:

1. LLM summarization,
2. embedding updates,
3. remote search,
4. filesystem reads,
5. skill package loading.

Those are AOS workflows/effects that produce refs or state updates. The planner consumes their
results.

### 7) Planner state is durable session state

The planner needs durable state across turns. Examples:

1. previously active skills,
2. recently discoverable skills,
3. pinned instruction refs,
4. completed summary refs,
5. memory refs selected or suppressed recently,
6. unresolved prerequisites,
7. planner-specific cursors or indexes.

That state must be part of deterministic workflow state. It must not live in an in-memory planner
instance, because replay and custom workflow composition must produce the same turn plan.

`aos-agent` should provide a structured `SessionTurnState` for common needs and an open extension
slot for custom planner state.

The planner trait should receive immutable state and return state updates through the plan. The
workflow applies those updates after the plan is accepted, records the plan/report, then emits
effects. This keeps planning pure and replayable.

Custom planners should be able to track their own state without changing the SDK schema every time.
Use refs or opaque namespaced records for planner-specific state, not new top-level SDK fields for
every product.

## Proposed Contracts

Illustrative target shape:

```rust
pub trait TurnPlanner {
    fn build_turn(&self, request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError>;
}

pub struct TurnRequest<'a> {
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub run_cause: Option<&'a RunCause>,
    pub budget: TurnBudget,
    pub session_turn_state: &'a SessionTurnState,
    pub run_config: &'a RunConfig,
    pub transcript_refs: &'a [String],
    pub turn_refs: &'a [String],
    pub steer_refs: &'a [String],
    pub registry: &'a BTreeMap<String, ToolSpec>,
    pub profiles: &'a BTreeMap<String, Vec<String>>,
    pub runtime: &'a ToolRuntimeContext,
}

pub struct TurnPlan {
    pub message_refs: Vec<String>,
    pub selected_tool_ids: Vec<String>,
    pub tool_choice: Option<LlmToolChoice>,
    pub response_format_ref: Option<String>,
    pub provider_options_ref: Option<String>,
    pub state_updates: Vec<TurnStateUpdate>,
    pub actions: Vec<TurnAction>,
    pub report: TurnReport,
}
```

The exact fields can change during implementation, but the important property is that one plan owns
the complete next LLM turn.

Candidate inputs:

```rust
pub struct TurnInput {
    pub input_id: String,
    pub lane: TurnInputLane,
    pub kind: TurnInputKind,
    pub priority: TurnPriority,
    pub content_ref: String,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Vec<String>,
}

pub struct TurnToolInput {
    pub tool_id: String,
    pub priority: TurnPriority,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub tags: Vec<String>,
}
```

Session-scoped planner state:

```rust
pub struct SessionTurnState {
    pub pinned_inputs: Vec<TurnInput>,
    pub summary_refs: Vec<String>,
    pub active_skills: Vec<SkillActivationState>,
    pub discoverable_skills: Vec<SkillDiscoveryState>,
    pub memory_refs: Vec<TurnInput>,
    pub runtime_hints: Vec<TurnInput>,
    pub last_report: Option<TurnReport>,
    pub custom_state_refs: Vec<PlannerStateRef>,
}

pub struct PlannerStateRef {
    pub planner_id: String,
    pub key: String,
    pub state_ref: String,
}
```

The structured fields cover the SDK's common needs. `custom_state_refs` lets a custom planner store
its own durable state as content-addressed data without adding schema variants to `aos-agent`.

State updates:

```rust
pub enum TurnStateUpdate {
    PinInput(TurnInput),
    RemoveInput { input_id: String },
    AddSummaryRef { summary_ref: String, input_refs: Vec<String> },
    ActivateSkill(SkillActivationState),
    DeactivateSkill { skill_id: String, reason: String },
    MarkSkillDiscoverable(SkillDiscoveryState),
    AddMemoryInput(TurnInput),
    RemoveMemoryInput { input_id: String },
    UpsertRuntimeHint(TurnInput),
    UpsertCustomStateRef(PlannerStateRef),
    RemoveCustomStateRef { planner_id: String, key: String },
}
```

The workflow applies these updates deterministically after validating the plan. External workflows
may also update `SessionTurnState` through explicit observations.

Observations:

```rust
pub enum TurnObservation {
    SummaryCompleted { summary_ref: String, input_refs: Vec<String> },
    InputPinned(TurnInput),
    InputRemoved { input_id: String },
    SkillResolved(SkillDiscoveryState),
    SkillActivated(SkillActivationState),
    SkillDeactivated { skill_id: String, reason: String },
    MemoryInputAdded(TurnInput),
    RuntimeHintUpdated(TurnInput),
    CustomStateRefUpdated(PlannerStateRef),
}
```

This replaces the old narrow `ContextObservation` with a turn-planning observation stream.

Report shape:

```rust
pub struct TurnReport {
    pub planner: String,
    pub selected_message_count: u64,
    pub dropped_message_count: u64,
    pub selected_tool_count: u64,
    pub dropped_tool_count: u64,
    pub budget: TurnBudget,
    pub decisions: Vec<String>,
    pub unresolved: Vec<String>,
    pub compaction_recommended: bool,
    pub compaction_required: bool,
}
```

Actions remain explicit requests, not hidden work:

```rust
pub enum TurnActionKind {
    Compact,
    Summarize,
    Materialize,
    ResolveSkill,
    RefreshMemory,
    OpenHostSession,
    Custom { kind: String },
}
```

`OpenHostSession` is a request to workflow code to emit the relevant explicit effect when config
allows it. The planner does not open anything directly.

## Scope

### [ ] 1) Replace context contracts with turn-planning contracts

Remove or supersede:

1. `ContextEngine`,
2. `ContextRequest`,
3. `ContextPlan`,
4. `ContextInput`,
5. `ContextSelection`,
6. `ContextReport`,
7. `SessionContextState`.

Add:

1. `TurnPlanner`,
2. `TurnRequest`,
3. `TurnPlan`,
4. `TurnInput`,
5. `TurnToolInput`,
6. `TurnSelection`,
7. `TurnReport`,
8. `SessionTurnState`,
9. `TurnObservation`,
10. `TurnStateUpdate`,
11. `PlannerStateRef`.

The old context implementation is not a compatibility layer. It is a source of test cases and
selection behavior to port.

### [ ] 2) Make LLM dispatch consume `TurnPlan`

Required outcome:

1. run start and tool follow-up turns call the planner,
2. planner-selected `message_refs` feed `sys/llm.generate@1`,
3. planner-selected tool ids determine which tool definitions are materialized,
4. planner-selected tool refs feed `sys/llm.generate@1`,
5. `tool_choice`, `provider_options_ref`, and `response_format_ref` come from the plan,
6. the run stores the turn plan for inspection,
7. the run trace records the turn-planning decision before LLM dispatch.

### [ ] 3) Move generic tool selection into the planner

Required outcome:

1. profile/default/run overrides are planner inputs,
2. selected tools are planner output,
3. `EffectiveToolSet` becomes either derived state or disappears,
4. `ToolAvailabilityRule::HostSessionReady` and `HostSessionNotReady` are removed from the generic
   SDK contract,
5. `profile_requires_host_session` is removed from `EffectiveToolSet` or replaced by explicit
   planner actions,
6. host/Fabric availability decisions live in default planner logic and host mappers, not in base
   tool specs.

### [ ] 4) Keep system/developer instructions as message refs

Required outcome:

1. `SessionConfig.default_prompt_refs` and `RunConfig.prompt_refs` are ported into `TurnInput`
   candidates,
2. those refs may point to `system` or `developer` message blobs,
3. the planner orders them deterministically ahead of conversational refs when selected,
4. no separate system-message template subsystem is added to the SDK core,
5. templating or instruction rendering happens in external workflows that materialize refs.

### [ ] 5) Add skill-aware planning inputs

Required outcome:

1. skills can contribute instruction refs,
2. skills can contribute compact discoverability/card refs,
3. skills can contribute tool candidates,
4. skills can contribute response-format/provider-option refs,
5. planner reports distinguish active, discoverable, and inactive skills,
6. P9 can build skill resolution above this without changing the core turn dispatch path.

### [ ] 6) Preserve compaction and memory hooks

Required outcome:

1. budgeted message selection remains deterministic,
2. compaction/summarization remain explicit action requests,
3. completed summary refs can be observed into session turn state,
4. memory refs are normalized turn inputs,
5. memory writes remain later explicit workflows and are not hidden inside the planner.

### [ ] 7) Define planner state update semantics

Required outcome:

1. planner state is stored on `SessionState` as `SessionTurnState`,
2. planners receive immutable state and return deterministic `TurnStateUpdate` values,
3. workflow code applies state updates after accepting a plan,
4. `TurnObservation` lets external workflows feed materialized summaries, resolved skills, memory
   refs, runtime hints, and custom planner state into the session,
5. custom planners can persist opaque state through namespaced `PlannerStateRef` entries,
6. no planner state depends on process memory or hidden I/O.

### [ ] 8) Prove custom planner override

Required outcome:

1. direct library consumers can call LLM dispatch with a custom `TurnPlanner`,
2. wrapper workflows can reuse base session/run and tool orchestration,
3. a focused test proves a custom planner can select a repo/bootstrap ref, activate a tool subset,
   update planner state, and drop transcript refs deterministically,
4. a custom planner can round-trip `PlannerStateRef` state across multiple turns,
5. no Demiurge or software-factory-specific SDK variants are added.

### [ ] 9) Update inspection and tests

Required outcome:

1. run state exposes the latest turn plan/report,
2. trace entries record turn planning before LLM dispatch,
3. deterministic unit tests cover prompt-ref compatibility, lane ordering, budget drops, skill
   activation states, and tool selection,
4. reducer tests cover selected tool materialization,
5. reducer tests cover planner state updates across multiple turns,
6. the deferred `aos-harness-py` fixture asserts turn plans rather than old context plans.

## Non-Goals

P6 does **not** add:

1. hidden LLM summarization,
2. embedding/search infrastructure,
3. policy/capability gating or approval UX,
4. host/Fabric execution adapters,
5. a complete skill registry,
6. memory write governance,
7. factory work-item workflows,
8. scheduler or heartbeat workflows.

Those systems feed materialized refs, tool candidates, observations, or actions into the planner.

## Acceptance Criteria

1. [ ] One planner call produces the complete next LLM turn.
2. [ ] System/developer/user/assistant/tool messages remain ordered refs in `message_refs`.
3. [ ] Tool selection is planner output, not a separate workflow-global `EffectiveToolSet` decision.
4. [ ] Host-specific availability rules are removed from generic tool specs.
5. [ ] Skills can be active, discoverable, or inactive per turn.
6. [ ] Turn reports explain selected/dropped messages, selected/dropped tools, actions, and unresolved prerequisites.
7. [ ] Compaction and materialization are explicit requested actions, not hidden planner I/O.
8. [ ] A custom planner can reuse base session/run/tool orchestration without forking the workflow loop.
9. [ ] Planner state is durable, replayable, and extensible for custom planners.
10. [ ] P7 traces record turn planning as the canonical pre-LLM decision point.
11. [ ] P10 deterministic fixtures assert turn plans end to end.
