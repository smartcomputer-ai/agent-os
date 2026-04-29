# aos-agent Background

This document describes how `aos-agent` works today, before the v0.30 agent roadmap changes.

The goal is to give reviewers and implementers enough context to understand what the roadmap is changing on top of. It is intentionally descriptive rather than aspirational. The roadmap docs describe the target refactors.

It is okay to focus only on aos-agent for now and assume we'll break downstream users, we can fix them later once the agent sdk reaches its desired shape. This allows us to refactor more agressively, without worrying about backward compatibility.

## High-Level Shape

`aos-agent` is the reusable AOS agent SDK layer.

It currently provides:

1. public `aos.agent/*` contract types,
2. a reusable evented workflow named `aos.agent/SessionWorkflow@1`,
3. reducer helpers used by the workflow and by embedding worlds,
4. built-in tool definitions and mappers,
5. generated AIR v2 package output for downstream worlds,
6. a WASM binary for the reusable session workflow.

The crate is intentionally close to AOS primitives:

1. sessions are keyed workflow state,
2. LLM calls are emitted as `sys/llm.generate@1` effects,
3. tools are represented as LLM tool definitions plus effect/domain-event mappers,
4. external work returns through receipts and stream frames,
5. large payloads are stored by blob refs,
6. generated AIR is derived from Rust source.

The current implementation is functional for one-shot coding-agent style runs, but several concepts are still blurred:

1. session status and run lifecycle are one enum,
2. the default tool registry implies a broad coding-agent surface,
3. context is just prompt refs plus conversation refs,
4. run traces are not first-class,
5. steer/follow-up/cancel semantics are shallow,
6. host target policy defaults to local host assumptions.

Those are the main reasons for the v0.30 roadmap.

## Main Crates and Consumers

### `crates/aos-agent`

`aos-agent` is a `no_std` library with `alloc`.

Important source areas:

1. `src/contracts/`
   - public schema-bearing contract types,
   - IDs, session config, state, events, lifecycle, tools, batches, host commands.
2. `src/helpers/`
   - reducer helpers, LLM parameter mapping, lifecycle transitions, queue helpers,
   - exported from the crate but marked `#[doc(hidden)]`.
3. `src/tools/`
   - default registry/profiles,
   - tool mappers,
   - supported host, inspect, and workspace tools.
4. `src/workflow.rs`
   - the reusable `SessionWorkflow` reducer and Rust-authored AIR workflow metadata.
5. `src/world.rs`
   - generated AIR package declaration through `aos_wasm_sdk::aos_air_world!`.
6. `src/bin/session_workflow.rs`
   - WASM binary entrypoint for `SessionWorkflow`.
7. `air/generated/`
   - checked-in generated AIR, validated by tests.

### `worlds/demiurge`

Demiurge is the main current world-level consumer.

It is a task-ingress orchestrator around `aos.agent/SessionWorkflow@1`:

1. accepts `demiurge/TaskSubmitted@1`,
2. writes the task text as a user message blob,
3. opens a host session for the submitted workdir,
4. emits `aos.agent/SessionInput@1` events to configure and start the session workflow,
5. watches `aos.agent/SessionLifecycleChanged@1`,
6. emits `demiurge/TaskFinished@1` when the agent run reaches a terminal task outcome.

Demiurge is also Rust-authored AIR. It imports `aos-agent` contract and workflow types through package metadata, not manual AIR JSON imports.

### `crates/aos-agent-eval`

`aos-agent-eval` is a live prompt/tool eval harness.

It:

1. creates a temporary world,
2. imports the generated `aos-agent` AIR,
3. compiles and patches the `session_workflow` WASM module,
4. seeds files into a per-attempt host workdir,
5. sends `SessionInput` events,
6. dispatches real host tools and live `llm.generate` calls,
7. asserts tool use, assistant output, and filesystem outcomes.

It is currently live-LLM oriented, so pass-rate thresholds account for model variance.

### `crates/aos-effect-adapters`

`aos-effect-adapters` executes external effects.

For agent work, the important adapters are:

1. blob put/get,
2. LLM provider adapters,
3. local host session/exec/fs adapters,
4. Fabric-backed host session/exec/fs adapters,
5. introspection and workspace internal handling through kernel/internal effects where applicable.

The adapter registry can route canonical host effects to local adapters or Fabric adapters depending on config.

Fabric is not part of `aos-agent` core. It is an execution backend below the canonical host effect API.

## Rust-Authored AIR Model

`aos-agent` is now AIR v2 and Rust-authored.

Contract types derive `AirSchema`, and the workflow is declared with:

```rust
#[aos_wasm_sdk::air_workflow(
    name = "aos.agent/SessionWorkflow@1",
    module = "aos.agent/SessionWorkflow_wasm@1",
    state = SessionState,
    event = SessionWorkflowEvent,
    context = aos_wasm_sdk::WorkflowContext,
    key_schema = SessionId,
    effects = [...]
)]
pub struct SessionWorkflow;
```

The generated package includes:

1. schemas for IDs, config, state, tool specs, batch state, events, etc.,
2. the workflow definition,
3. the workflow module definition,
4. routing for `aos.agent/SessionInput@1` into `SessionWorkflow`,
5. routing for `sys/WorkspaceCommit@1` into `sys/Workspace@1`.

The generated output is checked into `crates/aos-agent/air/generated/`.

The crate test regenerates AIR into a temp directory and asserts that the checked-in files match. Any contract or workflow metadata change needs regenerated AIR.

## Public Contract Surface

The crate root exports `contracts::*`, `SessionWorkflow`, and `aos_air_nodes`.

The important public contracts are:

1. `SessionId`
   - UUID-like session key wrapper.
2. `RunId`
   - `{ session_id, run_seq }`.
3. `ToolBatchId`
   - `{ run_id, batch_seq }`.
4. `SessionConfig`
   - provider/model/default prompt refs/tool defaults.
5. `RunConfig`
   - active run provider/model/prompt refs/tool overrides.
6. `SessionLifecycle`
   - current combined session/run lifecycle enum.
7. `SessionState`
   - keyed workflow state for one session.
8. `SessionInput` and `SessionInputKind`
   - external/control input into the session workflow.
9. `SessionWorkflowEvent`
   - reducer event enum covering input, receipts, receipt rejections, stream frames, noop.
10. `SessionLifecycleChanged`
   - domain event emitted when lifecycle changes.
11. Tool contracts
   - `ToolSpec`, `ToolMapper`, `ToolExecutor`, `EffectiveToolSet`, `ToolBatchPlan`, `ActiveToolBatch`, etc.
12. Host command contracts
   - `HostCommand` and `HostCommandKind`.

## Current Session State

`SessionState` is the main state record.

It currently contains all of these in one structure:

1. durable identity and timestamps,
2. combined lifecycle,
3. run sequence and tool batch sequence counters,
4. session config,
5. active run id/config,
6. active tool batch,
7. pending top-level effects,
8. pending shared blob gets/puts,
9. pending follow-up turn state,
10. queued LLM message refs,
11. conversation message refs,
12. last output ref,
13. tool registry and profiles,
14. selected profile and runtime context,
15. effective tool set,
16. last tool plan hash,
17. pending steer/follow-up queues,
18. in-flight effect count.

This is one of the main design issues the roadmap addresses.

The current state record mixes:

1. durable session state,
2. active run state,
3. tool execution state,
4. context/transcript state,
5. host runtime attachment state,
6. operator intervention state.

## Current Lifecycle Model

`SessionLifecycle` currently has:

1. `Idle`,
2. `Running`,
3. `WaitingInput`,
4. `Paused`,
5. `Cancelling`,
6. `Completed`,
7. `Failed`,
8. `Cancelled`.

This enum acts like both session status and run lifecycle.

For example:

1. `RunRequested` transitions `Idle` or terminal states to `Running`.
2. tool/LLM completion can transition `Running` to `WaitingInput`.
3. `RunCompleted` transitions to `Completed` and clears active run fields.
4. `RunFailed` transitions to `Failed`.
5. a later `RunRequested` may transition `Completed`, `Failed`, or `Cancelled` back to `Running`.

That means terminal states are terminal for one run, not for the durable session. The enum name hides that distinction.

## Session Ingress Events

`SessionInputKind` currently includes:

1. `RunRequested { input_ref, run_overrides }`,
2. `HostCommandReceived(HostCommand)`,
3. `ToolRegistrySet { registry, profiles, default_profile }`,
4. `ToolProfileSelected { profile_id }`,
5. `ToolOverridesSet { scope, enable, disable, force }`,
6. `HostSessionUpdated { host_session_id, host_session_status }`,
7. `RunCompleted`,
8. `RunFailed { code, detail }`,
9. `RunCancelled { reason }`,
10. `Noop`.

The workflow routes direct `SessionInput` events by `session_id`.

Receipts and stream frames are not ordinary domain ingress in the same sense. They re-enter through effect continuation handling and are represented in `SessionWorkflowEvent` so the reducer can settle pending effects.

## Run Start Flow

The current run start path is:

1. An embedding world or client stores a user message blob.
2. It sends `SessionInputKind::RunRequested { input_ref, run_overrides }`.
3. The reducer validates provider/model config.
4. It transitions lifecycle to `Running`.
5. It allocates a `RunId` by incrementing `next_run_seq`.
6. It stores `active_run_id` and `active_run_config`.
7. It clears active tool batch, pending blob state, queued LLM refs, conversation refs, and last output.
8. It recomputes effective tools.
9. It pushes `input_ref` into `conversation_message_refs`.
10. It queues an LLM turn with those refs.

The important current limitation is step 7: starting a run clears conversation history. That makes sense for current one-shot tasks, but not for a durable multi-run session.

## LLM Turn Dispatch

LLM turn dispatch is staged rather than emitted immediately in all cases.

`set_pending_llm_turn()` stores message refs in `pending_llm_turn_refs`, then calls `dispatch_pending_llm_turn()`.

`dispatch_pending_llm_turn()` waits until:

1. no top-level pending effects exist,
2. no pending blob gets exist,
3. no pending blob puts exist,
4. no pending follow-up turn is waiting.

Then it may do two setup steps before emitting `sys/llm.generate@1`:

1. auto-open a host session if the effective profile requires host tools and no host session is ready,
2. materialize tool definitions as blob refs if they have not been materialized.

After setup, it emits `sys/llm.generate@1` with:

1. provider,
2. model,
3. message refs,
4. tool refs,
5. automatic tool choice,
6. max tokens/reasoning effort from config,
7. provider secret ref when available.

Prompt refs are handled by prepending `RunConfig.prompt_refs` to the message refs during LLM parameter materialization. There is no context planner yet.

## LLM Receipt Flow

When a `sys/llm.generate@1` receipt arrives:

1. the pending effect is settled,
2. non-ok or decode failure fails the run,
3. the receipt payload gives an LLM output blob ref,
4. the workflow emits `sys/blob.get@1` for that output blob,
5. the output blob is decoded as `LlmOutputEnvelope`.

If the output envelope contains a `tool_calls_ref`:

1. the workflow emits `sys/blob.get@1` for tool calls,
2. the tool call blob is decoded as `LlmToolCallList`,
3. calls are converted into `ToolCallObserved`,
4. a tool batch starts.

If there is no `tool_calls_ref`, the run transitions from `Running` to `WaitingInput`.

The workflow also stores `last_output_ref` so wrappers such as Demiurge can fetch the final assistant output.

## Tool Registry and Profiles

`default_tool_registry()` currently builds one broad registry.

It includes:

1. host session tools,
2. host exec,
3. host filesystem tools,
4. introspection tools,
5. workspace tools.

`default_tool_profiles()` currently builds provider-shaped profiles:

1. `openai`,
2. `default`,
3. `anthropic`,
4. `gemini`.

The profiles share a large common set:

1. inspect world/workflow,
2. workspace inspect/list/read/apply/diff/commit,
3. host exec,
4. host read/write/grep/glob/stat/exists/list-dir.

The provider distinction mostly controls mutation style:

1. OpenAI gets `host.fs.apply_patch`.
2. Anthropic/Gemini get `host.fs.edit_file`.

`SessionState::default()` installs this default registry, default profiles, and `openai` as the selected profile. That is why the roadmap calls the current default an accidental coding-agent surface.

## Tool Specs

Each `ToolSpec` includes:

1. canonical `tool_id`,
2. LLM-facing `tool_name`,
3. `tool_ref`,
4. description,
5. JSON argument schema string,
6. `ToolMapper`,
7. `ToolExecutor`,
8. availability rules,
9. parallelism hint.

The canonical `tool_id` is used by profiles and config.

The LLM-facing `tool_name` is what the model calls. It must be unique and provider-valid.

The `tool_ref` initially uses a hash of the JSON tool definition bytes. Before an LLM call, the workflow writes actual tool definitions to CAS with `sys/blob.put@1` and updates `tool_ref` to the resulting blob ref. This makes LLM tool refs real blob refs.

## Effective Tool Selection

`refresh_effective_tools()` computes the current `EffectiveToolSet`.

Inputs:

1. selected profile,
2. session-level enable/disable/force lists,
3. run-level enable/disable/force lists,
4. current `ToolRuntimeContext`.

The tool runtime context currently contains:

1. `host_session_id`,
2. `host_session_status`.

Availability rules can require:

1. always available,
2. host session ready,
3. host session not ready.

If any selected tool requires a ready host session, the profile is marked `profile_requires_host_session`. That flag drives host session auto-open.

## Host Session Auto-Open

If a queued LLM turn needs host tools and no host session is ready, the workflow emits `sys/host.session.open@1`.

The current default mapper for `open_session` uses:

```json
{ "local": { "network_mode": "none" } }
```

unless a tool call explicitly supplies a `target`.

Demiurge does not rely on this default for its main path. It opens a host session itself for the submitted `workdir`, then sends `HostSessionUpdated` before `RunRequested`.

The roadmap changes this area because local host, Fabric sandbox host, workspace-only, and no-host agents need explicit target policy rather than one implicit local default.

## Tool Batch Planning

When tool calls are observed:

1. the workflow matches each LLM `tool_name` to an effective tool,
2. accepted calls become `PlannedToolCall` entries,
3. unknown/disabled calls are marked ignored,
4. a deterministic execution plan groups calls by parallelism hints and resource keys,
5. an `ActiveToolBatch` is stored.

Parallel-safe calls can run in the same group unless they share a resource key.

Non-parallel-safe calls get their own group.

The batch then advances group by group.

## Tool Execution Paths

The current tool system supports several execution styles.

### Effect Tools

Most host and inspect tools map to canonical effects.

The flow is:

1. parse the tool arguments JSON,
2. map arguments into effect params,
3. begin a pending effect keyed by call id,
4. emit the effect,
5. wait for receipt,
6. map receipt payload into an LLM tool result,
7. settle the call.

Host examples:

1. `host.exec` -> `sys/host.exec@1`,
2. `host.fs.read_file` -> `sys/host.fs.read_file@1`,
3. `host.fs.apply_patch` -> `sys/host.fs.apply_patch@1`.

Inspect examples:

1. `introspect.manifest` -> `sys/introspect.manifest@1`,
2. `introspect.workflow_state` -> `sys/introspect.workflow_state@1`.

### Workspace Composite Tools

Workspace tools are currently special-cased in the generic tool runner.

They can require multiple internal effects or blob puts before producing a single LLM tool result.

Examples:

1. `workspace.inspect`,
2. `workspace.list`,
3. `workspace.read`,
4. `workspace.apply`,
5. `workspace.diff`,
6. `workspace.commit`.

The workspace runner maintains a JSON-serialized internal phase state and emits `WorkspaceAction` values:

1. emit an internal workspace effect,
2. emit a domain event,
3. put a blob,
4. complete with a mapped receipt.

`workspace.commit` emits `sys/WorkspaceCommit@1` as a domain event and relies on routing to `sys/Workspace@1`.

This hardcoded composite behavior is one of the reasons P4 adds bundle-specific execution seams.

### HostLoop Tools

`ToolExecutor::HostLoop` exists as a contract variant, but the current built-in default registry primarily uses effect/domain-event execution for shipped tools.

Host-loop pending calls are counted as in-flight but do not currently represent the main shipped tool path.

## Tool Results and Follow-Up Turns

After all accepted tool calls in a batch settle:

1. results are ordered according to observed call order,
2. a `results_ref` hash is computed over ordered results,
3. follow-up message blobs are built:
   - assistant tool call message,
   - one function-call-output message per result,
4. those messages are written with `sys/blob.put@1`,
5. once all follow-up message blobs are available, the workflow appends them to `conversation_message_refs`,
6. it queues the next LLM turn.

This creates the normal tool-call loop:

```text
LLM output -> tool calls -> tool receipts -> tool result messages -> next LLM turn
```

Tool outputs can contain blob refs. When a mapped tool result includes blob refs in its JSON output, the workflow may fetch those blobs and inject bounded inline text into the tool result JSON before completing the call.

## Pending Effects and Receipts

The workflow tracks several pending categories:

1. top-level pending effects such as `llm.generate` and auto-open host session,
2. pending blob gets,
3. pending blob puts,
4. pending tool-call effects inside an active batch,
5. host-loop pending calls.

`in_flight_effects` is recomputed from those categories.

Receipts are matched by the pending effect structures. Receipt rejections are converted into error-like envelopes and then settled through mostly the same paths.

The reducer is deterministic: external work only changes state after a receipt, rejection, stream frame, or domain ingress is admitted.

## Stream Frames

`SessionWorkflowEvent::StreamFrame` exists and passes stream frames to pending effect observation.

Currently stream frames are not exposed as a first-class run trace. They mainly update pending-effect state.

P7 changes this by making progress frames traceable.

## Host Commands and Ref-Based Intervention

Current host commands are:

1. `Pause`,
2. `Resume`,
3. `Cancel { reason }`,
4. `Noop`.

Host commands are session lifecycle controls only. LLM-level intervention is modeled as
ref-based `SessionInputKind` variants:

1. `FollowUpInputAppended { input_ref, run_overrides }`,
2. `RunSteerRequested { instruction_ref }`,
3. `RunInterruptRequested { reason_ref }`.

The old text-based steer/follow-up host commands were removed from the SDK contract. The SDK
queues refs, not raw operator strings.

This keeps a clear distinction between:

1. follow-up input for a later run,
2. steer text for the next model turn,
3. interrupting active execution,
4. cancelling a run while external effects are active,
5. pausing a durable session.

Host/Fabric cancellation semantics remain deferred to P8.

## Context Model Today

There is no context engine yet.

The current context equivalent is:

1. `SessionConfig.default_prompt_refs`,
2. `RunConfig.prompt_refs`,
3. `conversation_message_refs`,
4. current run input ref,
5. tool follow-up message refs.

When materializing `sys/llm.generate@1`, prompt refs are prepended to message refs.

There is no:

1. context budget model,
2. source metadata,
3. selected/dropped input report,
4. compaction recommendation,
5. session-scoped context state,
6. skill contribution model.

P6 introduces this.

## Demiurge Current Integration

Demiurge currently wraps `SessionWorkflow` through events rather than direct reducer composition.

On task submission:

1. validates task id and absolute workdir,
2. validates allowed tools against `default_tool_registry()`,
3. stores task text as a user message blob,
4. opens a local host session for the workdir,
5. emits a sequence of `SessionInput` events:
   - optional `ToolRegistrySet`,
   - `HostSessionUpdated`,
   - `RunRequested`.

Demiurge then watches lifecycle events:

1. `Running` marks task running,
2. `WaitingInput` causes Demiurge to emit `RunCompleted`,
3. `Completed` marks task succeeded,
4. `Failed` marks task failed,
5. `Cancelled` marks task cancelled.

This means Demiurge currently treats one task as one session/run story. P5 changes the base model so Demiurge can map tasks onto explicit sessions and runs more cleanly.

## Eval Harness Today

There are two relevant harnesses today:

1. `aos-agent-eval`,
2. `aos-harness-py`.

They should not be treated as competing implementations of the same long-term role.

`aos-agent-eval` tests current live behavior.

Each attempt:

1. creates a fresh session id,
2. creates a fresh host workdir,
3. seeds files,
4. installs a tool registry/profile,
5. optionally bootstraps a host session,
6. stores the user prompt blob,
7. sends `RunRequested`,
8. drives effects until idle,
9. collects conversation observations from blob refs and active batch state,
10. asserts expected tools, output content, and file state.

This proves live provider/tool behavior, but it is not deterministic in the model-output sense.
It should remain available as a live provider/tool acceptance lane, not become the main SDK
correctness framework.

`aos-harness-py` is the intended direction for deterministic SDK and workflow integration tests.

It already provides:

1. `WorkflowHarness` for isolated workflow tests,
2. scripted effect choreography through `pull_effects()` and `apply_receipt_object()`,
3. helper receipts for LLM, blob, HTTP, and timer effects,
4. state, blob, trace, snapshot, and reopen inspection,
5. `WorldHarness` for realistic unified-node/SQLite world tests.

A deterministic scripted-LLM eval would replace the live LLM provider with a fake adapter whose responses are scripted by test case and turn. That would let tests verify reducer behavior, context planning, tool batching, traces, and replay without depending on model choices.

In practical terms, a scripted LLM eval drives the workflow until it emits `sys/llm.generate@1`,
inspects the request, admits a known `LlmGenerateReceipt`, answers the follow-up `sys/blob.get@1`
with a fixed `LlmOutputEnvelope`, scripts any tool-call argument blobs and tool receipts, then
asserts final state and trace output.

The migration direction is captured in `p10-agent-sdk-testing.md`:

1. keep Rust unit tests for low-level reducer invariants,
2. use `aos-harness-py` for deterministic SDK/workflow tests,
3. keep `aos-agent-eval` for live provider/tool acceptance during the transition.

## Effect Adapters and Fabric

The canonical effects used by `aos-agent` are backend-neutral.

For host tools, canonical effects include:

1. `sys/host.session.open@1`,
2. `sys/host.exec@1`,
3. `sys/host.session.signal@1`,
4. `sys/host.fs.read_file@1`,
5. `sys/host.fs.write_file@1`,
6. `sys/host.fs.edit_file@1`,
7. `sys/host.fs.apply_patch@1`,
8. `sys/host.fs.grep@1`,
9. `sys/host.fs.glob@1`,
10. `sys/host.fs.stat@1`,
11. `sys/host.fs.exists@1`,
12. `sys/host.fs.list_dir@1`.

`aos-effect-adapters` registers local host adapters by default.

If Fabric config is present, it also registers Fabric host adapters and route overrides so canonical host routes can execute through Fabric.

Fabric-backed adapters support:

1. sandbox session open,
2. exec,
3. session signal,
4. filesystem reads/writes/patch/grep/glob/stat/exists/list-dir,
5. exec progress frames.

The current `aos-agent` code does not depend on Fabric. P8 keeps that boundary and treats Fabric as a hosted execution backend selected through host target policy and adapter routing.

## Current Design Pressures

The roadmap is motivated by these concrete pressures in the current implementation:

1. The default registry and profile make all agents look like local coding agents.
2. Optional effects are all declared by the reusable workflow, so AIR surface and registry surface need clearer documentation.
3. The session lifecycle is actually a run lifecycle.
4. Starting a run clears conversation refs, making durable multi-run sessions awkward.
5. Context is not inspectable or budgeted.
6. Tool traces are fragmented across state fields and receipts.
7. Stream frames are not operator-visible.
8. Steer/follow-up/cancel are not precise enough for robust agent control.
9. Local host and Fabric-hosted execution need the same agent semantics but different target policy.
10. Skills need to feed context/tools explicitly rather than become hidden prompt magic.

The v0.30 roadmap should be read as a sequence of changes that untangles these current implementation boundaries without throwing away the working LLM/tool loop.
