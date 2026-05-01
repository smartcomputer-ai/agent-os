# P6: Turn Planner

**Priority**: P1
**Effort**: High
**Risk if deferred**: High (`aos-agent` will keep splitting prompt/context/tool/skill decisions across unrelated helpers, making future context engineering harder to reason about)
**Status**: Implemented in `aos-agent`
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.30-agent/p5-session-run-model.md`

## Goal

Replace the narrow context engine with a deterministic turn planner.

The planner decides the next LLM request shape:

1. ordered typed `active_window_items`,
2. selected tool ids,
3. `tool_choice`,
4. optional response-format and provider-options refs,
5. blocking prerequisites such as tool-definition materialization or host-session opening,
6. durable planner state updates,
7. a bounded report with selection counts, token estimates, and unresolved prerequisites.

This is the context-engineering boundary for `aos-agent`: what the model can see and what the model
can do should be planned together.

Post-turn maintenance is not part of the pre-turn decision. Usage-triggered compaction,
summarization, memory writes, reflection, and skill learning are considered after the LLM output,
tool results, token usage, and run state are known. A later compaction phase may feed a blocking
pre-turn prerequisite back into the planner when session state already says the next generation must
compact first.

## Current Fit

The earlier P6 implementation added useful pieces: `ContextEngine`, `ContextRequest`,
`ContextPlan`, `ContextInput`, bounded reports, and session-scoped context state.

That moved prompt refs, transcript refs, run-cause refs, summaries, and pinned inputs behind a
deterministic selector. The problem is that the current LLM turn is still assembled from separate
decisions:

1. context planning selects only message refs,
2. tool selection is computed separately as `EffectiveToolSet`,
3. host-session readiness is encoded in generic `ToolAvailabilityRule` variants,
4. selected tool refs are materialized outside the planning decision,
5. `tool_choice` is chosen by workflow code,
6. steer refs are appended before planning rather than represented as candidates.

Do not build compatibility indirection around `ContextEngine`. Port the useful behavior and tests
into the new planner.

## Design Stance

### 1) Planning is pre-turn only

`TurnPlanner` runs before `sys/llm.generate@1`.

It may request blocking prerequisites, but it must not perform hidden work. The workflow remains
responsible for admitting events, tracking state, emitting effects/blob puts, materializing selected
tool definitions, dispatching the LLM request, settling tool batches, and recording traces.

The planner consumes already-materialized refs and deterministic runtime state. It does not read
files, call embeddings, search memory, summarize text, load skill packages, or open host sessions.

### 2) Post-turn maintenance is separate

Do not decide compaction or summarization before the model turn.

A later `TurnFinalizer`-style hook may request compaction, summarization, memory refresh/write,
skill-resolution refresh, or custom maintenance after the turn is complete. P6 should leave this
hook explicit and replayable, but it does not implement those systems.

P11 extends this split: the pre-turn planner may return a blocking `CompactContext` prerequisite
when existing session state already says the next generation must compact first, but usage
high-water marks, provider-returned compaction artifacts, and context-limit failures are observed by
post-turn workflow/finalizer logic. The planner requests work; it does not run compaction or apply
active-window mutations.

### 3) Active window items are the primitive

Do not add a system-message or prompt-template subsystem.

`active_window_items` are the ordered model-visible context. Normal message refs remain valid window
items, and their blobs carry roles such as `system`, `developer`, `user`, `assistant`, or `tool`.
The active window can also contain AOS summary refs and provider-native compaction artifacts once
P11 lands. Role semantics stay in the blob or provider-native artifact. Planner metadata is only
for ordering, budgeting, token estimates, compatibility checks, and reporting.

### 4) Candidates are normalized

The planner should not receive separate `prompt_refs`, `transcript_refs`, `turn_refs`, `steer_refs`,
`memory_refs`, or `skill_refs` fields.

Workflow helpers convert all available material into `TurnInput` candidates: prompt refs,
transcript refs, current turn refs, steer refs, run-cause refs, pinned inputs, summaries, retrieved
memory refs, domain/workspace/runtime hint refs, and resolved skill contributions.

This keeps the planner API stable as new sources appear.

### 5) Lanes are metadata, not LLM roles

Initial lanes:

1. `System`,
2. `Developer`,
3. `Conversation`,
4. `ToolResult`,
5. `Steer`,
6. `Summary`,
7. `Memory`,
8. `Skill`,
9. `Domain`,
10. `RuntimeHint`,
11. `Custom { kind }`.

Lanes guide ordering, budgets, and source-specific decisions. They do not replace message roles.

### 6) Tools are planned with context

Tool schemas are part of what the model sees. Tool selection belongs in the same plan as message
selection.

The planner receives tool candidates plus registry/profile/run config state. It outputs selected
tool ids. The workflow materializes those definitions and passes their refs to `sys/llm.generate@1`.

Host/Fabric readiness should not be a base tool-spec primitive. If no host session exists, the
default planner can expose only `open_session` or request host-session materialization. If host
sessions are ready, host mappers decide whether a session can be implicit or must be explicit.

### 7) Skills are contributions, not core language

P6 should not add a full skill model to `aos-agent` core.

P9 or embedding worlds resolve skills into ordinary planner candidates:

1. instruction/message refs,
2. compact skill-card refs,
3. memory/query hint refs,
4. tool candidates,
5. response-format or provider-option refs,
6. source metadata such as `source_kind = "skill"` and `source_id = "<skill id>"`.

Active/discoverable/inactive skill semantics live above the core planner and are expressed through
candidates and source metadata.

### 8) Token estimates are deterministic inputs

`TurnInput` and `TurnToolInput` may carry estimated token counts. Unknown estimates are allowed and
reported.

The default implementation can start with a crude deterministic estimator supplied by the workflow
or caller. Actual provider token usage is recorded from receipts after the turn.

### 9) Stable ordering should help prompt caches

The default planner should keep stable, reusable material before volatile material:

1. system/developer/session instructions,
2. stable summaries,
3. stable skill cards or instruction refs,
4. memory/domain/runtime hints,
5. older selected transcript,
6. recent transcript,
7. steer/current turn refs.

Tool ids should also be selected in stable order. Provider-specific cache controls can later be
passed through `provider_options_ref`; P6 only needs stable ordering and minimal churn.

### 10) Planner state is durable and small

Planner state is part of deterministic workflow state. It must not live in an in-memory planner.

Core state should cover pinned inputs, durable inputs observed from explicit workflows, the last
bounded report, and opaque custom state refs. Custom planners persist extra state through
namespaced refs, not new top-level SDK fields.

### 11) Turn planning is the pre-LLM trace boundary

After the workflow accepts a `TurnPlan` and before it emits `sys/llm.generate@1`, it should record
the canonical pre-LLM trace entry.

P6 supersedes the current P7 `ContextPlanned` trace point with `TurnPlanned`. The trace entry should
stay compact: planner id, turn plan hash/ref when available, selected/dropped message counts,
selected/dropped tool counts, token estimate summary, prerequisite count, and unresolved count.
Large plan details stay behind refs.

Later `LlmRequested`, `LlmReceived`, tool, stream, and receipt trace entries should be able to
correlate back to the accepted turn plan. Updating those trace contracts belongs in P7.

## Proposed Contracts

Illustrative shape:

```rust
pub trait TurnPlanner {
    fn build_turn(&self, request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError>;
}

pub struct TurnRequest<'a> {
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub run_cause: Option<&'a RunCause>,
    pub run_config: &'a RunConfig,
    pub budget: TurnBudget,
    pub state: &'a SessionTurnState,
    pub inputs: &'a [TurnInput],
    pub tools: &'a [TurnToolInput],
    pub registry: &'a BTreeMap<String, ToolSpec>,
    pub profiles: &'a BTreeMap<String, Vec<String>>,
    pub runtime: &'a ToolRuntimeContext,
}

pub struct TurnPlan {
    pub active_window_items: Vec<ActiveWindowItem>,
    pub selected_tool_ids: Vec<String>,
    pub tool_choice: Option<LlmToolChoice>,
    pub response_format_ref: Option<String>,
    pub provider_options_ref: Option<String>,
    pub prerequisites: Vec<TurnPrerequisite>,
    pub state_updates: Vec<TurnStateUpdate>,
    pub report: TurnReport,
}

pub struct ActiveWindowItem {
    pub item_id: String,
    pub kind: ActiveWindowItemKind,
    pub content_ref: String,
    pub lane: Option<TurnInputLane>,
    pub source_refs: Vec<String>,
    pub estimated_tokens: Option<u64>,
}
```

If prerequisites are returned, the workflow satisfies them explicitly and retries dispatch when
state changes or materialization receipts arrive.

Candidate inputs:

```rust
pub struct TurnInput {
    pub input_id: String,
    pub lane: TurnInputLane,
    pub kind: TurnInputKind,
    pub priority: TurnPriority,
    pub content_ref: String,
    pub estimated_tokens: Option<u64>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Vec<String>,
}

pub enum TurnInputKind {
    MessageRef,
    ResponseFormatRef,
    ProviderOptionsRef,
    ArtifactRef,
    Custom { kind: String },
}

pub struct TurnToolInput {
    pub tool_id: String,
    pub priority: TurnPriority,
    pub estimated_tokens: Option<u64>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub tags: Vec<String>,
}
```

Budgeting:

```rust
pub struct TurnBudget {
    pub max_input_tokens: Option<u64>,
    pub reserve_output_tokens: Option<u64>,
    pub max_message_refs: Option<u64>,
    pub max_tool_refs: Option<u64>,
}

pub struct TurnTokenEstimate {
    pub message_tokens: u64,
    pub tool_tokens: u64,
    pub total_input_tokens: u64,
    pub unknown_message_count: u64,
    pub unknown_tool_count: u64,
}
```

Minimal state and reporting:

```rust
pub struct SessionTurnState {
    pub pinned_inputs: Vec<TurnInput>,
    pub durable_inputs: Vec<TurnInput>,
    pub last_report: Option<TurnReport>,
    pub custom_state_refs: Vec<PlannerStateRef>,
}

pub struct PlannerStateRef {
    pub planner_id: String,
    pub key: String,
    pub state_ref: String,
}

pub struct TurnReport {
    pub planner: String,
    pub selected_message_count: u64,
    pub dropped_message_count: u64,
    pub selected_tool_count: u64,
    pub dropped_tool_count: u64,
    pub token_estimate: TurnTokenEstimate,
    pub budget: TurnBudget,
    pub decision_codes: Vec<String>,
    pub unresolved: Vec<String>,
}
```

State updates and observations should stay generic:

```rust
pub enum TurnStateUpdate {
    UpsertPinnedInput(TurnInput),
    RemovePinnedInput { input_id: String },
    UpsertDurableInput(TurnInput),
    RemoveDurableInput { input_id: String },
    UpsertCustomStateRef(PlannerStateRef),
    RemoveCustomStateRef { planner_id: String, key: String },
}

pub enum TurnObservation {
    InputObserved(TurnInput),
    InputRemoved { input_id: String },
    CustomStateRefUpdated(PlannerStateRef),
    CustomStateRefRemoved { planner_id: String, key: String },
    Noop,
}
```

Prerequisites are explicit requests, not hidden planner work:

```rust
pub enum TurnPrerequisiteKind {
    MaterializeToolDefinitions,
    OpenHostSession,
    Custom { kind: String },
}

pub struct TurnPrerequisite {
    pub prerequisite_id: String,
    pub kind: TurnPrerequisiteKind,
    pub reason: String,
    pub input_ids: Vec<String>,
    pub tool_ids: Vec<String>,
}
```

P11 extends this prerequisite vocabulary with `CompactContext` and `CountTokens`; those are context
maintenance requests, not hidden planner work.

Post-turn maintenance can use a smaller later hook:

```rust
pub trait TurnFinalizer {
    fn finish_turn(&self, request: TurnFinalizerRequest<'_>) -> Result<PostTurnPlan, TurnPlanError>;
}

pub struct PostTurnPlan {
    pub state_updates: Vec<TurnStateUpdate>,
    pub actions: Vec<PostTurnActionKind>,
}

pub enum PostTurnActionKind {
    CompactContext,
    CountTokens,
    Summarize,
    RefreshMemory,
    WriteMemory,
    ResolveSkill,
    Custom { kind: String },
}
```

Avoid verbose full-prompt reports in core state. If a product needs rich diagnostics, store a
separate blob ref above the SDK.

## Scope

### [x] 1) Replace context contracts

Remove or supersede `ContextEngine`, `ContextRequest`, `ContextPlan`, `ContextInput`,
`ContextSelection`, `ContextReport`, and `SessionContextState`. Add the turn-planning contracts:
`TurnPlanner`, `TurnRequest`, `TurnPlan`, `TurnInput`, `TurnToolInput`, `TurnBudget`,
`TurnTokenEstimate`, `TurnReport`, `SessionTurnState`, `TurnObservation`, `TurnStateUpdate`,
`TurnPrerequisite`, and `PlannerStateRef`.

### [x] 2) Build normalized candidates before planning

Convert all message/source refs into `TurnInput` and tool registry/profile/run overrides into
`TurnToolInput`. Candidates must carry deterministic ids, source metadata, priority, lane, and
optional token estimates. The planner should receive candidates, not source-specific ref lists.

### [x] 3) Make LLM dispatch consume `TurnPlan`

Run start and tool follow-up turns call the planner. Planner-selected `active_window_items`, tool
ids, `tool_choice`, `provider_options_ref`, and `response_format_ref` feed
`sys/llm.generate@1.window_items`.
Blocking prerequisites delay dispatch until explicitly satisfied. Run state and trace expose the
latest turn plan/report.

### [x] 4) Move generic tool selection into the planner

Selected tools become planner output. `EffectiveToolSet` becomes derived state or disappears.
`ToolAvailabilityRule::HostSessionReady` and `HostSessionNotReady` are removed from generic tool
specs. Host/Fabric availability decisions live in default planner logic and host mappers.

### [x] 5) Keep instructions as normal active-window message refs

`SessionConfig.default_prompt_refs` and `RunConfig.prompt_refs` become `TurnInput` candidates. Those
refs may point to `system` or `developer` message blobs. Default ordering keeps stable instruction
refs ahead of volatile conversational refs. Templating or rendering happens in external workflows
that materialize refs.

### [x] 6) Keep skills source-agnostic

Skills feed planner inputs through `TurnInput` and `TurnToolInput`. Core P6 contracts do not define
skill storage or activation schemas. Selected/dropped skill contributions are reportable through
source metadata. P9 can add skill descriptors and resolvers without changing LLM dispatch.

### [x] 7) Preserve memory and compaction hooks

Memory refs and completed summary refs are normalized turn inputs. Pre-turn budget selection remains
deterministic. Pre-turn compaction is represented only as an explicit prerequisite when already
required by session state. Post-turn compaction/summarization/memory actions are explicit requests.
No memory retrieval, embedding update, active-window mutation, or summarization runs inside the
planner.

### [x] 8) Define durable planner state semantics

Planner state is stored on `SessionState` as `SessionTurnState`. Planners receive immutable state
and return deterministic updates. `TurnObservation` lets external workflows feed materialized refs
and custom state into the session. Custom planners persist opaque state through namespaced
`PlannerStateRef` entries.

### [x] 9) Prove custom planner override

Direct library consumers can dispatch with a custom `TurnPlanner`; wrapper workflows can reuse base
session/run/tool orchestration. Add a focused test for custom message selection, tool selection,
state update, and transcript dropping. Do not add Demiurge or software-factory-specific SDK variants.

### [x] 10) Update inspection and tests

Add deterministic unit tests for prompt-ref compatibility, lane ordering, budget drops, token
estimates, source metadata, and tool selection. Add reducer tests for selected tool materialization,
blocking prerequisites, and planner state updates. Update `aos-harness-py` fixtures to assert turn
plans rather than old context plans.

### [x] 11) Mark P7 trace follow-up

P6 should record the accepted turn plan as the pre-LLM trace point. P7 still needs follow-up work to
rename or supersede `ContextPlanned` with `TurnPlanned`, add compact turn-plan metadata, and thread
turn-plan correlation through `LlmRequested`, `LlmReceived`, tool, stream, and receipt trace entries.

## Non-Goals

P6 does **not** add:

1. hidden LLM summarization,
2. embedding/search infrastructure,
3. memory write governance,
4. policy/capability gating or approval UX,
5. host/Fabric execution adapters,
6. a complete skill registry,
7. skill package loading,
8. factory work-item workflows,
9. scheduler or heartbeat workflows,
10. extensive prompt/context reports in core state.

Those systems feed materialized refs, tool candidates, observations, post-turn actions, or custom
state refs into the planner.

## Acceptance Criteria

1. [x] One pre-turn planner call produces the complete next LLM request shape.
2. [x] System/developer/user/assistant/tool messages remain ordered normal message-ref items in
   `active_window_items`.
3. [x] Tool selection is planner output, not a separate workflow-global `EffectiveToolSet` decision.
4. [x] Host-specific availability rules are removed from generic tool specs.
5. [x] Skills participate as generic source-tagged candidates, not core SDK skill state.
6. [x] Token estimates and unknown-token counts are represented in the plan/report.
7. [x] Prompt-cache-friendly stable ordering is a default planner invariant.
8. [x] Usage-triggered compaction, summarization, and memory writes are post-turn actions, while pre-turn compaction is only an explicit prerequisite from existing session state.
9. [x] A custom planner can reuse base session/run/tool orchestration.
10. [x] Planner state is durable, replayable, and extensible through namespaced refs.
11. [x] P6 records `TurnPlanned` as the canonical pre-LLM trace point, with P7 follow-up called out
   for trace schema/correlation updates.
12. [x] P10 deterministic fixtures assert turn plans end to end.
