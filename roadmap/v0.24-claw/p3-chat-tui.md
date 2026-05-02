# P3: Codex-Like Chat TUI

**Priority**: P3
**Effort**: Large
**Risk if deferred**: Medium (the chat engine can work in plain mode, but users will not get the rich live interaction needed for complex tools, compaction, and intervention)
**Status**: Planned
**Depends on**: `roadmap/v0.24-claw/p2-chat-cli-internals.md`

## Goal

Build a Codex-like terminal chat UI in `aos-cli` using Ratatui and Crossterm.

The TUI should feel closer to Codex than to a log tailer: persistent transcript, multiline composer, live run status, collapsible tool chains, compaction cards, session switching, slash commands, scrollback, pickers, overlays, and graceful terminal lifecycle handling.

This is still an AOS UI. It must render the P2 `ChatEvent` stream and send P2 `ChatCommand`s. It must not bypass the chat engine to talk directly to node control APIs.

## Inspiration From Codex-RS

Use Codex-RS as architectural inspiration, not as a code copy target:

- Ratatui + Crossterm for terminal rendering/input.
- A top-level `App` owns terminal state, routing, input, draw scheduling, and shutdown.
- An internal `AppEvent` bus coordinates actions across widgets.
- A chat widget owns transcript state and maps protocol/chat events into renderable cells.
- A bottom pane owns the composer, status/footer, and transient views.
- Renderable history cells keep transcript layout modular and width-aware.
- Streaming/in-progress cells are consolidated into stable transcript cells when a turn completes.

Important difference: Codex owns the model/tool runtime directly; AOS observes durable world state and journal progress. The AOS TUI should never be the source of truth for agent state.

Before implementing this slice, consult the current Codex Rust project:

- local checkout: `/Users/lukas/dev/tmp/codex`
- `https://github.com/openai/codex/tree/main/codex-rs`
- `codex-rs/tui/src/tui.rs`
- `codex-rs/tui/src/app.rs`
- `codex-rs/tui/src/app_event.rs`
- `codex-rs/tui/src/app_event_sender.rs`
- `codex-rs/tui/src/chatwidget.rs`
- `codex-rs/tui/src/bottom_pane/`
- `codex-rs/tui/src/history_cell.rs`
- `codex-rs/tui/src/transcript_reflow.rs`
- `codex-rs/tui/src/streaming/`
- `codex-rs/tui/src/slash_command.rs`

The goal is to reuse the shape of the design: app loop, event bus, bottom pane, composer, transcript cells, overlays, and terminal safety patterns. Do not mechanically port Codex-specific model/session/runtime assumptions into AOS.

Codex-RS details checked for this plan:

- `tui.rs` is an inline viewport terminal, not a conventional full-screen alternate-screen app by default. Committed history is inserted into normal terminal scrollback, while the active Ratatui viewport stays anchored at the bottom. Alternate screen exists, but should be reserved for focused detail surfaces where it is actually useful.
- `FrameRequester` coalesces redraw requests through a broadcast draw channel and a frame-rate limiter. Widgets request frames; they do not redraw directly.
- `App::run` selects over app events, terminal events, active-thread/runtime events, and server events. The AOS equivalent should select over `UiEvent`s, `TuiEvent`s, `ChatEvent`s, and shutdown.
- `ChatWidget` keeps committed history cells plus an in-flight `active_cell` with a revision counter. Streaming/tool cells mutate in place while active, then flush into committed history.
- `HistoryCell` is source-backed and width-aware. Most cells expose `display_lines(width)` and measure with Ratatui wrapping; final assistant markdown stores source so resize reflow can render again.
- `BottomPane` owns the composer and a stack of `BottomPaneView`s. Pickers and focused forms are bottom-pane views first, not global app modals.
- `ListSelectionView` is the reusable picker primitive for model, reasoning, settings, and other choices. It supports current/default/disabled rows, filtering, descriptions, and stable selection.
- `SlashCommand` is a typed enum in presentation order with feature gating and "available during task" rules. AOS should follow that shape for `/model`, `/effort`, `/session`, and tool/session commands.

## Design Stance

1. Codex-style TUI from the first interactive release.

   The terminal channel is the primary product surface for v0.24. It should be rich enough to handle long tool-heavy runs and context management without a later rewrite.

   Default to Codex's inline viewport model: committed transcript lives in normal terminal scrollback, the live active cell and bottom pane are redrawn by Ratatui, and retained source-backed cells allow transcript/detail views to be rebuilt. This gives users shell-native scrollback while still allowing rich live controls.

2. Keep terminal code inside `aos-cli`.

   Do not add a new `aos-tui` crate in this slice. Use modules under `crates/aos-cli/src/chat/tui/` and keep boundaries clean enough that extraction remains possible later.

3. Event-driven UI.

   TUI widgets emit `UiEvent` or `UiIntent`. The `App` translates those into `ChatCommand`s, overlay changes, or local UI updates. Widgets do not own async tasks.

4. Renderer state is separate from chat projection.

   P2 decides what happened. P3 decides how to show it, which cells are collapsed, what is selected, which retained overlay scroll offset is active, and which focused view is open.

5. Terminal lifecycle must be robust.

   Enter raw mode/bracketed paste only after command setup succeeds. Use alternate screen only for focused detail surfaces if needed. Always restore terminal state on normal exit, error exit, panic hook, and Ctrl-C/Ctrl-D paths.

6. Progressive disclosure for complexity.

   Tool chains, compaction, reasoning, and raw JSON refs should be visible but collapsed by default. The primary transcript should stay readable.

## Module Layout

Keep P3 under the P2 chat module:

```text
crates/aos-cli/src/chat/tui/mod.rs
crates/aos-cli/src/chat/tui/app.rs              # top-level UI state and event loop
crates/aos-cli/src/chat/tui/app_event.rs        # UI-local event bus and typed intents
crates/aos-cli/src/chat/tui/app_event_sender.rs # lightweight sender helpers
crates/aos-cli/src/chat/tui/tui.rs              # terminal wrapper, event stream, frame requester
crates/aos-cli/src/chat/tui/event_stream.rs     # Crossterm EventStream -> TuiEvent
crates/aos-cli/src/chat/tui/frame.rs            # coalesced redraw scheduling
crates/aos-cli/src/chat/tui/terminal.rs         # raw mode, alternate screen, panic restore
crates/aos-cli/src/chat/tui/viewport.rs         # inline viewport + scrollback insertion helpers
crates/aos-cli/src/chat/tui/layout.rs           # viewport/bottom-pane split calculations
crates/aos-cli/src/chat/tui/style.rs            # palette, status styles, symbols
crates/aos-cli/src/chat/tui/transcript.rs       # committed cells, active cell, reflow state
crates/aos-cli/src/chat/tui/cell.rs             # renderable transcript cell trait/types
crates/aos-cli/src/chat/tui/cells/message.rs
crates/aos-cli/src/chat/tui/cells/run.rs
crates/aos-cli/src/chat/tui/cells/tool.rs
crates/aos-cli/src/chat/tui/cells/compaction.rs
crates/aos-cli/src/chat/tui/cells/error.rs
crates/aos-cli/src/chat/tui/bottom_pane/mod.rs  # composer + status/footer + view stack
crates/aos-cli/src/chat/tui/bottom_pane/view.rs # BottomPaneView trait
crates/aos-cli/src/chat/tui/bottom_pane/composer.rs
crates/aos-cli/src/chat/tui/bottom_pane/list_selection.rs
crates/aos-cli/src/chat/tui/bottom_pane/footer.rs
crates/aos-cli/src/chat/tui/slash.rs            # typed slash commands + availability
crates/aos-cli/src/chat/tui/overlay.rs          # transcript/detail/help overlays
crates/aos-cli/src/chat/tui/markdown.rs         # markdown/code block rendering helpers
crates/aos-cli/src/chat/tui/snapshot_tests.rs   # test backend rendering fixtures
```

Do not put terminal code in `commands/chat.rs`.

## Dependencies

Add to `aos-cli`:

- `ratatui`
- `crossterm` with `bracketed-paste` and `event-stream` features
- `tokio-stream` if the existing async event loop needs a stream wrapper for terminal events
- `unicode-width`
- `textwrap` or a small local wrapper if workspace dependency policy prefers it

Optional after the first pass:

- Ratatui `scrolling-regions` if we use `scroll_region_up` for Codex-style inline scrollback insertion. If the workspace version exposes this behind a feature, enable it on `ratatui`.
- `pulldown-cmark` for markdown parsing.
- `syntect` only if syntax highlighting is needed immediately. Plain fenced code blocks are enough for P3 initial acceptance.
- `insta` for snapshot tests if the repo accepts it. Codex-RS relies heavily on `insta` plus Ratatui `TestBackend`; otherwise use checked-in string snapshots with `TestBackend`.

## Screen Layout

Use a Codex-style inline viewport by default. The terminal's normal scrollback contains committed transcript history; Ratatui owns the live viewport at the bottom.

```text
normal terminal scrollback

  user message
  assistant message
  completed tool chain
  compaction notice

live Ratatui viewport

  active assistant/tool/run cell
  status/footer: tools 2 running | context 82% | journal #12345 | connected
  > multiline composer
```

Responsive behavior:

- Header/status is one line unless the terminal is extremely narrow. Prefer footer/status over a permanent top header in the first pass; Codex keeps the bottom pane as the main persistent chrome.
- Bottom pane grows up to a bounded height for multiline input.
- Active cells own remaining live viewport space; committed cells are inserted into terminal scrollback.
- If width is too small, shorten identifiers and hide secondary metadata before wrapping important content.
- Do not rely on color alone; status markers need text or symbols.
- Keep enough source in memory to rebuild a transcript/detail overlay and to reflow committed history after resize. Do not treat terminal scrollback as the authoritative transcript.

## Transcript Cells

Represent each transcript item as a width-aware source-backed cell. Codex's strongest pattern here is `HistoryCell::display_lines(width)` plus a default render path that measures wrapped line count. Use that unless a cell needs custom drawing.

```rust
trait ChatCell {
    fn id(&self) -> &str;
    fn kind(&self) -> ChatCellKind;
    fn display_lines(&self, width: u16, state: &CellRenderState) -> Vec<Line<'static>>;
    fn transcript_lines(&self, width: u16, state: &CellRenderState) -> Vec<Line<'static>>;
    fn desired_height(&self, width: u16, state: &CellRenderState) -> u16;
    fn is_stream_continuation(&self) -> bool;
}
```

Initial cell types:

- `UserMessageCell`: submitted user text and optional source/ref metadata.
- `AssistantMessageCell`: final or streaming assistant text, markdown-aware.
- `RunStatusCell`: run start/finish/failure/cancel/waiting-input.
- `ToolChainCell`: grouped tool calls and effect progress.
- `ToolCallCell`: expanded view for one tool call, args, streams, result, errors.
- `CompactionCell`: context pressure, compaction request/receipt, token counts, active-window update.
- `ReasoningCell`: reasoning refs or summaries when available.
- `ReconnectCell`: SSE reconnects, gaps, stale projection warnings.
- `ErrorCell`: actionable errors from P2.
- `SystemNoticeCell`: selected session, config changes, pause/resume.

Cells should be cheap to re-render on resize. Store raw source where possible and re-wrap during render.

Keep one active mutable cell per live run/tool region when possible:

- Assistant text may stream/update in an active message cell, then consolidate into a source-backed markdown cell.
- Tool chains should update in place while calls progress, then flush into committed history once the batch/run is stable.
- Active cells need a revision counter so transcript/detail overlays can invalidate cached render output when the cell mutates without changing identity.
- For AOS, "streaming" usually means P2 projection updates from durable state rather than raw model-token deltas. The UI should still use Codex's active-cell pattern to avoid appending a new cell on every journal refresh.

## Tool Chain Rendering

Tool chains must be first-class, not dumped as JSON.

Collapsed chain:

```text
tools 3 calls  group 1/2 running
  group 1 parallel
    grep "SessionInput"        ok
    read crates/aos-agent/...  running
  group 2 waiting
    edit roadmap/...           queued
```

Expanded call:

```text
tool edit roadmap/v0.24-claw/p2-chat-cli-internals.md
status running
args
  {"path":"...","operation":"apply_patch"}
progress
  patch staged
result
  pending
```

Rules:

- Group by run id and tool batch when available.
- Render execution groups from `ToolBatchPlan.execution_plan.groups`; calls in the same group are parallel siblings, while later groups are sequential barriers.
- Preserve order from trace sequence, not local completion time.
- Show pending/running/succeeded/failed distinctly.
- Show group-level status: waiting, running, blocked, succeeded, failed, or partial.
- Collapse long args/results with an explicit "show more" action.
- Show stdout/stderr or stream-frame text in a bounded subview.
- Provide a raw detail overlay for refs and decoded JSON.

## Compaction Rendering

Compaction should be visible because it explains pauses and context changes.

Collapsed card:

```text
context compacted  124k -> 38k tokens  active window updated
```

Expanded card:

```text
context pressure observed
reason: active window exceeded target
token count: 124k input, 4k reserve
compaction: requested -> received
artifact: sha256:...
active window: 41 items selected, 18 dropped
```

Rules:

- Render context pressure before compaction when both are present.
- Render requested/received as one evolving card when possible.
- Link compaction artifact refs to the detail overlay.
- If details are not present in trace metadata, show the lifecycle without inventing numbers.

## Composer

The composer should support:

- multiline editing,
- bracketed paste,
- Enter to submit in single-line mode,
- Shift-Enter or Alt-Enter for newline where supported,
- Ctrl-J as a reliable newline fallback,
- Ctrl-C to interrupt active run or exit when idle,
- Ctrl-D to exit when composer is empty,
- Up/Down history navigation when cursor is at the first/last line,
- slash command completion,
- draft preservation across overlays,
- visible disabled state while session switching.

P3 should implement a normal insert-mode editor first. Vim-style bindings can be added later if needed.

## Slash Commands

Initial slash commands:

```text
/help
/new
/sessions
/resume <session-id>
/status
/model [model]
/provider [provider]
/effort [low|medium|high|none]
/max-tokens [n|none]
/tools
/trace
/compact
/interrupt [reason]
/steer <instruction>
/pause
/resume-session
/copy
/clear
/quit
```

Command behavior:

- Commands that mutate world/session become `ChatCommand`s.
- UI-only commands stay as `UiEvent`s.
- Slash commands should be a typed enum in presentation order, with metadata for description, inline-argument support, feature availability, and availability during an active run.
- Slash commands without arguments should open option pickers, in the style of Claude Code/Codex. The common flow is type `/model`, press Enter, then select from a list.
- `/model` opens a model picker; `/model <model>` may remain as a power-user shortcut.
- `/provider` opens a provider picker; `/provider <provider>` may remain as a power-user shortcut.
- `/effort` opens a reasoning-effort picker; `/effort <value>` may remain as a power-user shortcut.
- `/max-tokens` opens an input/picker; `/max-tokens <n|none>` may remain as a power-user shortcut.
- In the first version, `/model` and `/provider` may be unsupported once the selected session has accepted its first run. The TUI should render this as a clear status/error cell and suggest `/new`. This should not imply that AOS sessions fundamentally forbid later model changes.
- `/effort` is disabled while a run is active. When idle, it may apply to the next run even in an existing session.
- Header/status should show the active run model/effort when a run is active and draft model/effort otherwise.
- `/compact` is only enabled if the agent workflow exposes a compaction input or command path. Otherwise show a disabled explanation.
- `/trace` opens a detail overlay using current `ChatRunView`.

Model picker flow:

- Use a reusable `ListSelectionView`-style bottom-pane view.
- Show current, default, and disabled rows distinctly.
- If selecting a model implies a default reasoning effort, apply both together.
- If a model has multiple supported efforts, either open a second reasoning picker immediately or keep `/effort` as the next visible action. Codex uses a second picker; AOS should prefer that once model metadata is available.
- If model metadata is not yet available from config/provider, show configured models first and leave dynamic discovery as an enhancement.

## Overlays

Use two levels of focused UI, matching Codex's split:

- Bottom-pane views for common selection and form flows.
- Full transcript/detail overlays only when the surface needs more room than the bottom pane.

Bottom-pane view stack:

- Help/keybindings.
- Session picker.
- Model picker.
- Provider picker.
- Reasoning effort picker.
- Max-token input/picker.
- Confirm interrupt/quit when a run is active.

Detail overlays:

- Tool call detail.
- Compaction detail.
- Raw JSON/ref viewer.
- Error detail.
- Transcript/search view.

Overlays should pause composer input but not pause the chat engine. Live events continue updating the transcript behind the overlay.

Model/provider/effort pickers are bottom-pane selection views. They should:

- show the current value,
- show whether the field is currently editable,
- apply immediately to draft settings when editable,
- support keyboard navigation, filtering where the option list can be long, and a visible disabled reason for unavailable options,
- avoid entering raw JSON/config editing in the first version.

## Event Loop

Top-level app loop inputs:

- Crossterm key/mouse/paste/resize events.
- `ChatEvent`s from P2 engine.
- Frame draw events from a coalescing `FrameRequester`.
- Optional tick events for animations and cursor blink, scheduled through the frame requester.
- Shutdown signals.

Outputs:

- `ChatCommand`s to P2 engine.
- redraw requests.
- local UI state changes.

Basic loop:

1. Start terminal guard.
2. Start `ChatEngine`.
3. Load initial session/projection.
4. Schedule the first frame.
5. Select over terminal events, app events, chat events, draw events, and shutdown.
6. Route input to overlay, composer, or transcript depending on focus.
7. Apply chat events to transcript model.
8. Request redraws from widgets/tasks and draw only when the frame requester emits.
9. On exit, request engine shutdown and restore terminal.

Do not redraw on every async event if several events arrive in one poll. Follow Codex's `FrameRequester` approach: many components can request frames, but the scheduler coalesces and rate-limits actual draw notifications.

## App Event Model

P3 should define a UI-local event enum similar in spirit to Codex-RS:

```rust
enum UiEvent {
    SubmitComposer,
    ComposerChanged,
    ScrollUp,
    ScrollDown,
    ToggleSelectedCell,
    OpenOverlay(OverlayKind),
    CloseOverlay,
    Chat(ChatEvent),
    Tick,
    Resize { cols: u16, rows: u16 },
    ExitRequested,
}
```

`UiEvent::Chat` carries the P2 engine stream into the UI reducer. UI reducers should be unit-testable without a terminal.

Also add a small sender wrapper, similar to Codex's `AppEventSender`, so widgets can emit typed intents without holding references to `ChatTuiApp` internals. This wrapper should be thin and should not know about node control APIs.

## State Model

```rust
struct ChatTuiApp {
    connection: ChatConnectionInfo,
    transcript: TranscriptState,
    bottom_pane: BottomPaneState,
    detail_overlays: Vec<DetailOverlay>,
    status: StatusLineState,
    focus: Focus,
    pending_exit: Option<ExitMode>,
}
```

`TranscriptState` owns:

- ordered cells,
- active cell,
- active cell revision,
- pending history lines waiting to be inserted into terminal scrollback,
- transcript resize/reflow state,
- selected cell id for overlays/detail navigation,
- retained source for transcript/detail views,
- collapsed/expanded map,
- search state,
- active run cell ids.

In the inline viewport model, terminal scrollback owns ordinary upward scrolling. For a retained transcript overlay, when the user is at bottom, new events keep the view pinned to bottom. When the user has scrolled up inside the overlay, new events should not yank the viewport; show an unread indicator instead.

`BottomPaneState` owns:

- composer state,
- view stack,
- status/footer lines,
- pending steers/queued input preview,
- whether input is enabled,
- paste burst state if we need delayed paste rendering.

## Styling

Use a restrained terminal palette:

- User messages: clear label, no heavy boxes.
- Assistant messages: readable markdown, code blocks visually distinct.
- Running tools: subtle accent.
- Success: green text/symbol only where useful.
- Failure/error: red label plus concise message.
- Compaction/context: neutral technical color.
- Reconnect/gap: warning style.

Avoid large bordered cards for every cell. Use spacing, labels, indentation, and one-line separators. Tool details and overlays can use borders.

## Keyboard Map

Initial bindings:

- `Enter`: submit or confirm overlay selection.
- `Ctrl-J`: newline.
- `Esc`: close overlay or clear composer completion.
- `Ctrl-C`: interrupt active run; if idle, exit.
- `Ctrl-D`: exit when composer empty.
- `PageUp/PageDown`: scroll the retained transcript/detail overlay when it is open.
- `Ctrl-U/Ctrl-D`: half-page scroll when an overlay is focused and the composer is not consuming it.
- `Home/End`: composer line navigation; with modifiers, transcript overlay top/bottom.
- `Tab`: complete slash command or focus next overlay control.
- `Space`: toggle selected transcript cell when transcript focus is active.
- `Ctrl-T`: open retained transcript/search overlay if that mode lands in P3.
- `?`: help overlay.

Mouse support can wait unless cheap through Ratatui/Crossterm defaults.

## Terminal Safety

`terminal.rs` should own:

- raw mode enter/exit,
- bracketed paste enter/exit,
- focus change enter/exit if useful,
- keyboard enhancement enter/exit where supported,
- alternate screen enter/exit for detail surfaces that need it,
- cursor show/hide,
- panic hook restore,
- signal-aware shutdown,
- stdin flush after temporarily dropping terminal input,
- fallback message after restoration if a fatal error occurred.

Never print ordinary logs while in alternate screen. Route diagnostics into transcript cells or write them after terminal restoration.

Codex-specific safety pattern to copy:

- Check stdin/stdout are terminals before entering TUI mode.
- Enable bracketed paste and raw mode during init, and restore them in reverse on exit.
- Use a stronger "restore after exit" path for keyboard reporting state.
- Do not keep a Crossterm `EventStream` alive while running an external interactive program.
- Use synchronized terminal updates for draw paths where possible.

## Phasing

P3a: Shell

- Ratatui/Crossterm setup.
- App loop.
- Inline viewport, committed-history insertion, active cell, bottom pane.
- Frame requester/redraw coalescing.
- Basic composer.
- Bottom-pane view stack and generic list selection view.
- Render user/assistant/run/error cells from fake `ChatEvent`s.
- Plain terminal restore tests.

P3b: Live AOS

- Wire to real P2 `ChatEngine`.
- Session picker.
- Model/provider/effort/max-token pickers.
- Submit user turns.
- Committed terminal scrollback and retained transcript overlay.
- Assistant output rendering.
- Reconnect/gap notices.

P3c: Complex Runs

- Tool chain cells.
- Tool detail overlay.
- Compaction cells.
- Trace overlay.
- Interrupt/steer flows.
- Slash command completion.

P3d: Polish

- Markdown/code wrapping.
- Copy/export.
- Search in transcript.
- Better narrow-terminal behavior.
- Snapshot tests for representative full frames.

## Scope

- Add TUI modules under `crates/aos-cli/src/chat/tui/`.
- Add Ratatui/Crossterm dependencies to `aos-cli`.
- Make `aos chat` launch the Codex-style TUI by default on TTY.
- Keep `--plain` fallback using P2.
- Implement transcript cells for messages, runs, tools, compaction, reconnects, and errors.
- Implement bottom composer and slash command parser.
- Implement bottom-pane selection views for session/model/provider/effort choices and detail overlays for large views.
- Add UI reducer tests and render snapshots.

## Non-Goals

- New TUI crate.
- Browser/desktop UI.
- Image/file attachment UI.
- Mouse-first interaction.
- Multi-world dashboard.
- Backend API changes unless P2 proves trace metadata is insufficient.
- Perfect parity with Codex-RS.

## Test Plan

Unit tests:

- Key binding reducer.
- Composer editing and paste handling.
- Slash command parse/completion.
- Transcript append/update/reflow behavior.
- Retained transcript overlay scroll, sticky-bottom, and unread indicator behavior.
- Cell height calculations at narrow, normal, and wide widths.
- Tool chain collapsed/expanded rendering model.
- Compaction card rendering model.

Render tests:

- Initial empty session.
- Active run with pending LLM.
- Tool-heavy run with nested calls.
- Failed tool call.
- Compaction requested/received.
- SSE reconnect and retained-history gap.
- Narrow terminal layout.

Integration/manual tests:

- `aos chat --new` against a local node.
- Resume existing session.
- Submit follow-up while previous run is active.
- Interrupt a running session.
- Disconnect/reconnect node.
- Resize terminal during streaming output.
- Panic/fatal error restores terminal state.

## Open Questions

- Should the TUI have a separate transcript search mode in P3, or defer until after tool/compaction rendering lands?
- Should `/compact` be a direct session input, a workflow-specific command, or only a display of automatic compaction for now?
- Which trace metadata should become required for excellent tool grouping?
- Should the selected session picker load full states eagerly or show summaries first and lazy-load previews?
- Should P3 implement transcript/search overlay immediately, or ship shell-native scrollback first and add the retained overlay in P3d?
