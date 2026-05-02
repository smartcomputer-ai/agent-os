# P2: Agent Chat CLI Internals

**Priority**: P2
**Effort**: Large
**Risk if deferred**: High (the TUI will either couple directly to HTTP/world internals or invent UI-specific session semantics that are hard to reuse for WhatsApp/email later)
**Status**: Implemented
**Depends on**: `roadmap/v0.24-claw/p1-world-journal-sse.md`

## Goal

Build a UI-independent agent chat engine inside `aos-cli`.

The P2 slice owns the transport, session selection, submission, world observation, history reconstruction, blob loading, and projection logic needed by the terminal TUI. P3 should be able to focus on terminal rendering and input handling instead of AOS protocol details.

The chat internals must support a full Codex-like TUI from the beginning:

- durable session selection and resume,
- user turns and follow-up turns,
- current run status,
- assistant output extraction,
- tool chain progress,
- compaction and context-pressure progress,
- intervention signals such as steer, interrupt, pause, and resume,
- reconnect-safe live observation over world journal SSE,
- a plain/non-TTY fallback for scripts and debugging.

Most code should live in `aos-cli`. Do not add a new product crate for this slice.

## Implementation Snapshot

Implemented in `aos-cli`:

- `aos chat`, `aos chat sessions`, `aos chat history`, and `aos chat send`.
- Chat modules under `crates/aos-cli/src/chat/` for config, typed protocol/view models, control client, SSE parsing, session key handling, blob cache, projection, engine orchestration, and plain rendering.
- Typed `aos.agent/SessionInput@1` submission using `aos-agent` contracts.
- CAS upload/download for chat-authored user messages and assistant output blobs.
- Journal SSE parsing and follow/reconnect cursor handling over `/journal/stream`.
- Session listing/selection with per-world selected-session persistence in CLI config.
- Projection of runs, transcript turns, active parallel tool batches, and compaction trace entries.
- Plain diagnostic renderer shared by `history`, `send --plain/--follow`, and `chat --plain`.

The Codex-style Ratatui/Crossterm TUI remains P3. In this implementation, `aos chat` without
`--plain` returns an explicit P3-not-implemented error rather than silently using a temporary UI.

P3 compatibility check:

- The core boundary is aligned: P3 can render `ChatEvent`s and send `ChatCommand`s without knowing `ApiClient`, session input encoding, CAS fetching, or journal SSE details.
- The snapshot-style events (`ReplaceTurns`, `RunChanged`, `ToolChainsChanged`, `CompactionsChanged`) fit AOS's authoritative-state model. P3 can keep Codex-style active mutable cells by diffing these stable view models rather than treating every refresh as a new cell.
- The current projection is enough for the first live TUI: session selection, messages, active run status, active tool batch grouping, compaction lifecycle, reconnect/gap notices, and settings editability.
- Before polishing P3 detail overlays, extend P2 view models with raw refs where the TUI needs drilldown: tool argument/result refs, effect/receipt refs, stream-frame refs, reasoning refs, and trace refs.
- Before treating completed runs as rich transcript history, teach P2 to preserve completed tool/compaction summaries from `RunRecord.trace_summary` and any durable materialized refs available after `current_run` moves into `run_history`. Today the richest tool/compaction projection is active-run oriented.

## Current Fit

The existing stack already has the right primitives:

- `aos.agent/SessionWorkflow@1` is keyed by `SessionId`.
- `aos.agent/SessionInput@1` accepts `RunRequested`, `FollowUpInputAppended`, `RunSteerRequested`, `RunInterruptRequested`, and lifecycle/config inputs.
- `SessionState` records current run, run history, active tool batch, pending effects, pending blob work, context state, run traces, and `last_output_ref`.
- `RunTraceEntryKind` already has progress classes for LLM, tools, effects, stream frames, receipts, context pressure, compaction, token counts, active-window updates, and run finish.
- `ToolParallelismHint`, `ToolBatchPlan.execution_plan.groups`, `ActiveToolBatch.call_status`, and `ActiveToolBatch.pending_effects` already model parallel-safe tool groups, sequential barriers, per-call status, and in-flight receipts.
- `SessionConfig` and `RunConfig` already carry `provider`, `model`, `reasoning_effort`, `max_tokens`, prompt refs, tool profile/overrides, and host-session config.
- LLM outputs are stored as CAS blobs using `LlmOutputEnvelope`, including `assistant_text`, `tool_calls_ref`, and `reasoning_ref`.
- P1 added durable journal wait/stream endpoints. The chat client should use journal SSE as a wakeup/progress feed and existing state/CAS reads for authoritative detail.

The missing piece is a client-side projection layer that turns generic AOS world activity into stable chat-facing events.

## Design Stance

1. Keep chat semantics outside the TUI.

   The terminal UI should render `ChatEvent`s and send `ChatCommand`s. It should not know how to encode `SessionInput`, parse journal records, debounce state reads, or load CAS blobs.

2. Treat world state as authoritative.

   Journal SSE is the live observation mechanism, but reconstructed chat history comes from `SessionState`, domain events, run traces, and CAS blobs. The client should tolerate missed wakeups, reconnects, duplicate events, and partial blob availability.

3. Preserve AOS generic boundaries.

   Do not add an agent-specific backend stream in this slice. The chat engine observes `/journal/stream`, reads `/state`, reads `/trace` where useful, and fetches `/v1/cas/blobs/{hash}`.

4. Make projections deterministic and replayable.

   Given the same initial session snapshot, journal records, and blob values, the projection should produce the same ordered `ChatEvent`s. UI animation, timestamps for local display, and terminal ticks stay outside the projection.

5. Prefer typed contracts at the CLI boundary.

   `aos-cli` should depend on `aos-agent` for `SessionInput`, `SessionState`, `SessionId`, lifecycle types, trace types, and config types unless that creates an actual build problem. Local duplicate wire structs should be a fallback, not the first design.

6. Keep a scriptable mode.

   The full-screen TUI is the default for an interactive terminal, but the same engine should expose a plain event stream for `--plain`, tests, and CI diagnostics.

## CLI Surface

Add a top-level command:

```text
aos chat [--session <uuid>] [--new] [--plain] [--world <uuid>] [--from <seq>]
aos chat sessions [--world <uuid>]
aos chat history --session <uuid> [--world <uuid>] [--json]
aos chat send --session <uuid> --message <text> [--follow] [--plain]
```

Initial behavior:

- `aos chat` opens the TUI against the selected profile/world once P3 lands; P2 returns an explicit not-yet-implemented error unless `--plain` is used.
- `--session` resumes one session.
- `--new` creates a new UUID and opens it on first submitted turn.
- If neither is provided, use the last CLI-selected session for that world when present; otherwise open the session picker.
- `sessions` lists `aos.agent/SessionWorkflow@1` state cells and basic lifecycle metadata.
- `history` renders reconstructed history without entering raw terminal mode.
- `send --follow` is a non-TTY-friendly path that submits one message and streams progress until the run reaches a terminal lifecycle.

Future channel integrations should be able to reuse the session id, message blob conventions, and projection model without reusing the terminal UI.

## Module Layout

Keep modules under `crates/aos-cli/src`:

```text
commands/chat.rs              # clap args, command dispatch, output mode selection
chat/mod.rs                   # public chat module surface within aos-cli
chat/config.rs                # per-world selected session and UI preferences in CLI config
chat/protocol.rs              # typed wire helpers around SessionInput, state, CAS, journal SSE
chat/client.rs                # ChatControlClient wrapping ApiClient with chat-specific methods
chat/sse.rs                   # SSE parser and reconnect cursor handling
chat/session.rs               # session ids, listing, selection, state key encoding
chat/blob_cache.rs            # CAS blob fetch/cache/decode helpers
chat/projection.rs            # SessionState + journal + blobs -> ChatEvent
chat/engine.rs                # async orchestration and command/event channels
chat/plain.rs                 # line-oriented renderer using the same engine
chat/tui/...                  # P3-owned terminal UI modules
```

`commands/chat.rs` should stay thin. The long-lived logic belongs under `chat/`.

## Core Types

The engine exposes a narrow API:

```rust
pub(crate) struct ChatEngine;

pub(crate) enum ChatCommand {
    SubmitUserMessage { text: String },
    SetDraftProvider { provider: String },
    SetDraftModel { model: String },
    SetDraftReasoningEffort { effort: Option<ReasoningEffort> },
    SetDraftMaxTokens { max_tokens: Option<u64> },
    SteerRun { text: String },
    InterruptRun { reason: Option<String> },
    PauseSession,
    ResumeSession,
    SwitchSession { session_id: String },
    Refresh,
    Shutdown,
}

pub(crate) enum ChatEvent {
    Connected(ChatConnectionInfo),
    SessionSelected(ChatSessionSummary),
    HistoryReset { session_id: String },
    TranscriptDelta(ChatDelta),
    RunChanged(ChatRunView),
    ToolChainChanged(ChatToolChainView),
    CompactionChanged(ChatCompactionView),
    StatusChanged(ChatStatus),
    GapObserved { requested_from: u64, retained_from: u64 },
    Reconnecting { from: u64, reason: String },
    Error(ChatErrorView),
}
```

The exact Rust names can change, but the boundary should stay stable:

- TUI sends `ChatCommand`.
- Engine emits `ChatEvent`.
- Projection emits declarative view models.
- Widgets do not call `ApiClient` directly.

## Session Selection

Session source of truth:

- World state cells under `aos.agent/SessionWorkflow@1`.
- Each cell key is the CBOR-encoded `SessionId`.
- The selected session per world may be cached in the CLI config for convenience, but cached selection is never authoritative.

Selection flow:

1. Resolve the world from `--world`, global `--world`, profile, or env.
2. Load session cells from `/v1/worlds/{world}/state/aos.agent%2FSessionWorkflow%401/cells`.
3. Decode each key to `SessionId`.
4. Fetch state for likely candidates only. Avoid loading every blob for a large world.
5. Sort sessions by `SessionState.updated_at` descending.
6. If a requested session has no state yet, treat it as a new empty session that will materialize on first input.

P2 can list sessions as UUIDs plus lifecycle/status. P3 can add fuzzy search and richer previews.

## Message Blob Contract

The CLI-authored user message blob should be plain JSON compatible with the agent context planner:

```json
{
  "role": "user",
  "content": "hello",
  "source": {
    "kind": "aos-cli",
    "channel": "terminal",
    "session_id": "..."
  }
}
```

Rules:

- Hash and upload the UTF-8 JSON bytes to CAS before submitting the session input.
- Use the resulting `sha256:<hex>` as `input_ref`.
- Keep the role/content fields stable because existing workflows already use that shape.
- Extra source metadata is best-effort context and must not be required by the workflow.

First user turn:

```json
{
  "schema": "aos.agent/SessionInput@1",
  "value": {
    "session_id": "SESSION_UUID",
    "observed_at_ns": 0,
    "input": {
      "$tag": "RunRequested",
      "$value": {
        "input_ref": "sha256:...",
        "run_overrides": null
      }
    }
  }
}
```

Follow-up turn:

```json
{
  "input": {
    "$tag": "FollowUpInputAppended",
    "$value": {
      "input_ref": "sha256:...",
      "run_overrides": null
    }
  }
}
```

Use `RunRequested` when there is no active or queued run for the session. Use `FollowUpInputAppended` when a session exists or the engine is uncertain. The workflow already starts the queued follow-up immediately when idle and queues it when a run is active.

`observed_at_ns` should be deterministic from the client perspective only as a monotonic client stamp. It does not need wall-clock precision for correctness. If the node later exposes a server observed-time helper, use that instead.

## Model And Effort Settings

Chat settings should be slash-command driven in the TUI and represented in P2 as local draft settings plus explicit run/session config writes.

Settings model:

```rust
struct ChatDraftSettings {
    provider: String,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    max_tokens: Option<u64>,
}
```

Rules:

- The TUI changes draft settings through slash commands such as `/model`, `/provider`, `/effort`, and `/max-tokens`.
- Slash commands without arguments should emit picker-oriented commands/events rather than requiring users to remember values. For example, `/model` opens a model picker and the selected option then becomes `SetDraftModel`.
- `ReasoningEffort` maps to the existing `aos.agent/ReasoningEffort@1` values: `Low`, `Medium`, and `High`.
- The first `RunRequested` for a new session should include `run_overrides: Some(SessionConfig { ... })` built from the draft settings.
- If the selected session already has `SessionState.session_config`, use it to seed draft settings.
- If the selected session is empty, seed draft settings from CLI config/profile defaults or explicit CLI args.
- First-version P2 does not need to support changing `provider` or `model` after a session has accepted its first run. `/model` and `/provider` may return an unsupported/actionable `ChatEvent` telling the user to start `/new` for another model. This is an implementation limit, not a long-term session invariant.
- Reasoning effort can be changed for the next run while no run is active, using `run_overrides` on `RunRequested` or `FollowUpInputAppended`. It should be disabled while a run is active.
- If this proves confusing, the implementation may also leave late reasoning-effort changes unsupported, but the preferred P2 behavior is per-next-run effort with late model/provider switching unsupported.
- `SessionConfigUpdated` should be used only when intentionally changing session defaults. For first-version chat, prefer explicit `run_overrides` so a slash change has clear scope.

Display state:

- `ChatConnectionInfo` or `ChatStatus` should include current provider, model, effort, and whether each setting is currently editable.
- The projection should show the active run config from `SessionState.active_run_config` when present, not only the local draft.
- Plain mode should print model/effort changes and disabled-setting warnings.

## Observation Loop

The engine runs these async tasks:

1. **Session loader**

   Loads the selected session snapshot, recent state, known output refs, and enough blobs to render initial history.

2. **Journal follower**

   Opens `/v1/worlds/{world}/journal/stream?from=<cursor>` and parses `journal_record`, `world_head`, `gap`, `error`, and keepalive events.

3. **Refresh coordinator**

   Converts journal events into debounced reads. Multiple journal records in a burst should usually produce one state refresh.

4. **Blob loader**

   Fetches missing CAS refs concurrently with bounded parallelism. It caches successful decodes by hash and records failures as renderable errors.

5. **Submitter**

   Serializes chat commands that mutate the world: CAS upload first, session input event second. It should emit an optimistic local user-message delta after the event is accepted, but final history still comes from state/projection.

6. **Supervisor**

   Owns cancellation, reconnect backoff, shutdown, and error fan-out.

## Journal Handling

The chat engine should subscribe with a limited kind set when possible:

```text
kind=domain_event
kind=effect_intent
kind=effect_receipt
kind=stream_frame
```

It should still be correct if the server sends all kinds.

Rules:

- Maintain `next_from` exactly as supplied by SSE `journal_record` and `world_head`.
- On reconnect, pass explicit `from=<next_from>` instead of relying only on `Last-Event-ID`.
- On `gap`, emit a visible warning event and rebuild the session projection from current state.
- Treat journal records as hints plus optional detail. Do not assume every needed display field is present in every record.
- Use state reads after domain events, receipts, stream frames, and world-head advancement.

## Projection Model

`chat/projection.rs` owns a `ChatProjection`:

```rust
struct ChatProjection {
    world_id: String,
    session_id: String,
    journal_next_from: u64,
    session_state: Option<SessionState>,
    blob_cache: BlobCache,
    turns: Vec<ChatTurn>,
    active_run: Option<ChatRunView>,
    tool_chains: Vec<ChatToolChainView>,
    compactions: Vec<ChatCompactionView>,
    seen_trace_seq: BTreeSet<u64>,
}
```

Projection sources:

- `SessionState.turn_state` and `context_state` for durable message refs and active-window state.
- `SessionState.current_run.trace.entries` for live progress.
- `SessionState.run_history` for completed run summaries.
- `SessionState.active_tool_batch`, `pending_effects`, and pending blob maps for active tool/effect state.
- `SessionState.last_output_ref` and `RunLifecycleChanged.output_ref` for final assistant output.
- CAS blobs for user messages, assistant output envelopes, tool call argument refs, tool results, reasoning, and compaction artifacts.

The projection should emit deltas, not a full redraw payload, so the TUI can preserve scroll position and expanded/collapsed cell state.

## Rendering-Oriented View Models

P2 should define view models that are independent of Ratatui:

```rust
struct ChatTurn {
    turn_id: String,
    user: Option<ChatMessageView>,
    assistant: Option<ChatMessageView>,
    run: Option<ChatRunView>,
    tool_chains: Vec<ChatToolChainView>,
}

struct ChatToolChainView {
    id: String,
    title: String,
    status: ChatProgressStatus,
    calls: Vec<ChatToolCallView>,
    summary: Option<String>,
}

struct ChatCompactionView {
    id: String,
    status: ChatProgressStatus,
    reason: Option<String>,
    before_tokens: Option<u64>,
    after_tokens: Option<u64>,
    artifact_ref: Option<String>,
}
```

Status should be semantic (`queued`, `running`, `waiting`, `succeeded`, `failed`, `cancelled`, `stale`, `unknown`) rather than styled. Styling belongs to P3.

## Tool Chain Semantics

Tool rendering should be based on run trace entries and effect state:

- `ToolCallsObserved`: model requested one or more tool calls.
- `ToolBatchPlanned`: workflow planned the tool batch.
- `EffectEmitted`: an effect intent was emitted.
- `StreamFrameObserved`: a running effect produced progress.
- `ReceiptSettled`: an effect completed or failed.
- `RunFinished`: summarize unresolved tools and terminal result.

Parallel tool use should be represented directly:

- A tool batch is one LLM tool-call response.
- `ToolBatchPlan.execution_plan.groups` is the intended execution shape.
- Calls within one group may be in flight at the same time.
- Groups are sequential barriers; group N+1 should not render as runnable until group N has settled.
- `ToolParallelismHint.parallel_safe=false` forces a single-call group.
- `ToolParallelismHint.resource_key` prevents conflicting parallel-safe calls from sharing a group.
- Host-loop tools can be `Pending` without a normal effect receipt; the projection must still count them as in-flight through `ActiveToolBatch.call_status`.

The initial projection can group by run id, tool batch id, execution group, and effect/tool identifiers available in trace refs and metadata. If trace metadata is insufficient for high-quality grouping, P2 should add small, backward-compatible trace metadata improvements in `aos-agent` rather than making the TUI infer from free-form summaries.

## Compaction Semantics

Compaction should be first-class in the projection because the TUI needs to explain long-running context management:

- Show `ContextPressureObserved` when pressure is detected.
- Show `CompactionRequested` while waiting for the compaction effect.
- Show `CompactionReceived` when the compacted artifact is available.
- Show `ActiveWindowUpdated` after the planner switches to the compacted context.

If token counts are present through `TokenCountRequested` and `TokenCountReceived`, include before/after counts in the view model.

## Plain Mode

`chat/plain.rs` should subscribe to the same `ChatEvent` stream as the TUI and print stable lines:

```text
session <uuid> running
user: ...
run 3 running
tool search_step running
tool search_step ok
compaction requested
assistant: ...
run 3 completed
```

Plain mode is not a lesser implementation. It is the diagnostic view for projection correctness.

## Dependencies

Likely P2 dependencies in `aos-cli`:

- `aos-agent.workspace = true`
- `futures-util.workspace = true`
- `tokio-stream.workspace = true` if added to workspace
- an SSE parser crate, or a small local SSE parser with strict tests
- `reqwest` with streaming enabled; prefer aligning `aos-cli` with the workspace `reqwest` version instead of keeping a local older version

Ratatui/Crossterm dependencies belong to P3.

## Failure Handling

The engine should handle:

- selected world missing,
- session workflow not installed in the current manifest,
- selected session missing,
- journal SSE disconnect,
- retained-history gap,
- malformed SSE event,
- session state decode failure,
- missing CAS blob,
- LLM output blob without `assistant_text`,
- workflow rejected user input,
- terminal user interrupts while a run is active.

Every failure should produce a `ChatEvent::Error` with actionability. The TUI decides whether to render it as a transcript cell, status message, or modal.

## Scope

- Add `aos chat` command family.
- Add chat engine modules under `crates/aos-cli/src/chat/`.
- Add typed session input submission.
- Add CAS upload/download helpers for chat blobs.
- Add journal SSE client with reconnect cursor semantics.
- Add session listing and selection support.
- Add `SessionState` to chat projection.
- Add assistant output extraction from `LlmOutputEnvelope`.
- Add tool chain and compaction view models.
- Add plain mode over the same event stream.
- Add unit and integration tests for the engine/projection.

## Non-Goals

- Full-screen TUI rendering. That is P3.
- New `aos-chat` crate.
- Agent-specific backend SSE routes.
- WebSocket support.
- Multi-world fan-in.
- WhatsApp/email integration.
- Durable channel registry changes beyond session id usage.
- Perfect historical reconstruction for sessions created before this client exists if required blobs are absent.

## Test Plan

Unit tests:

- Encode `SessionInput` JSON for first turn, follow-up turn, steer, interrupt, pause, and resume.
- Encode/decode `SessionId` state keys.
- Parse SSE chunks with split lines, comments, ids, multi-line data, and reconnect ids.
- Advance journal cursors over `journal_record` and `world_head`.
- Handle `gap` by resetting projection state.
- Project `SessionState` with current run, completed run, active tool batch, pending effects, and compaction trace entries.
- Project a parallel tool batch with two same-group calls, a later sequential group, and mixed succeeded/running/failed call statuses.
- Decode user message blobs and `LlmOutputEnvelope` blobs.
- Deduplicate repeated journal/state refreshes.

Integration tests:

- Start an embedded node/control app.
- Create or load an agent world with `aos.agent/SessionWorkflow@1`.
- Submit a first user turn through the chat engine.
- Observe journal progress through P1 SSE.
- Fetch session state and render a plain transcript.
- Submit a follow-up turn and verify it becomes a second run or queued follow-up.

Manual smoke:

- `aos chat --plain --new`
- `aos chat sessions`
- `aos chat history --session <uuid>`
- Disconnect/restart the node during an active follow and verify reconnect/gap behavior.

## Open Questions

- Should `aos-cli` persist last selected session in the existing profile config or in a separate chat state file keyed by API/world?
- Should `observed_at_ns` be client monotonic, wall-clock, or server-assigned in a later route?
- Do current run trace refs contain enough stable identifiers to group complex tool chains without brittle inference?
- Should the workflow expose a dedicated session summary state cell to avoid loading large `SessionState` values for the session picker?
- Should chat-authored user message blobs eventually get an AIR schema, or is JSON role/content sufficient for the terminal channel?
