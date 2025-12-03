# P4: REPL & Developer Experience

**Goal:** Provide a pleasant interactive loop (`aos dev`) that talks to a running world over the control channel. REPL never touches `WorldHost` directly; it uses the same ControlClient as the CLI.

## Principles
- Single interface: all REPL commands go through the control channel (Unix socket/stdio). No direct kernel/adapter access.
- Respect daemon semantics: `step` in daemon mode must invoke `run_cycle(RunMode::WithTimers)` (via control `step`). Batch-only paths are a fallback when no daemon is running.
- Auto-manage lifecycle: if no daemon is running, auto-start one for the session; on exit, shut it down only if we started it.
- Clear command semantics: distinguish enqueue-only vs enqueue+step; keep commands idempotent and versioned with the control protocol.
- Safety: treat the control socket as the single-authority endpoint. Before auto-starting a daemon, check for an existing socket and refuse to stomp a running instance. If the socket file exists but is unreachable, surface a clear error and ask the user to clean up or kill the stale process—never auto-remove a potentially-live socket.

## REPL Architecture

```
crates/aos-host/src/repl/
├── mod.rs          # ReplSession orchestrating ControlClient + UI loop
├── commands.rs     # Command handlers -> control requests
├── display.rs      # Formatting for results/errors/state
├── parser.rs       # Input parsing / shortcuts
└── client.rs       # Thin wrapper over ControlClient (framing, timeouts)
```

### Session Flow
- On start:
  1) Try to connect to control socket; if absent, spawn daemon (background) and retry.
  2) Create ControlClient (socket or stdio).
  3) Enter readline loop.
- On exit:
  - If REPL started the daemon, send `shutdown` and await ack; otherwise leave daemon running.

## Command Surface (maps to control verbs)
- `event <schema> <json>` → `send-event` (enqueue only).
- `event-step <schema> <json>` (or `event ... --step`) → enqueue + `step` (daemon uses `run_cycle(RunMode::WithTimers)`).
- `event @file.json` or `event @-` → allow file/stdin inputs (client-side convenience before sending `send-event`).
- `state <reducer> [--key <json>]` → `query-state` (returns raw bytes; REPL pretty-prints if decodes as JSON).
- `state` with no args: list reducers by reading manifest via control (once exposed).
- `step` → control `step` (daemon: `run_cycle(RunMode::WithTimers)`; batch fallback: local `WorldHost::run_cycle` if no daemon).
- `snapshot` → control `snapshot`.
- `shutdown` → control `shutdown` (only if we own the daemon).
- `manifest` → optional `query-manifest` control verb (or drop if not implemented).
- `effects` / `timers` → only if control protocol exposes pending effects/timers; otherwise omit to avoid special-casing.
- Optional log/tail: pretty-print recent journal entries if control exposes them.

## CLI `aos dev`
- Detect running daemon via control socket.
- If absent: scaffold (optional template), start daemon, then launch REPL.
- If present: connect and launch REPL without touching lifecycle.
- Flags:
  - `--stdio` to force stdio control mode (CI/tests).
  - `--template` for scaffold when path missing.

## Pretty Output
- Keep lightweight formatting (no color in core; let CLI decide).
- Helpers to render events/effects/receipts/state; avoid kernel types directly—use control responses only.

## Tasks
1) Implement ControlClient wrapper for REPL (Unix socket + stdio, JSON Lines; optional CBOR if enabled).
2) Rewrite REPL commands to call control verbs; add `event-step` helper.
3) Add auto-start/auto-shutdown logic for daemon ownership in `aos dev`.
4) Optional: add control verbs for `pending-effects` / `pending-timers`; otherwise drop those commands from REPL.
5) Add file/stdin helpers for `event` inputs on the client side.
6) Persist history under platform data dir; keep the UI responsive (async readline).

## Success Criteria
- `aos dev examples/00-counter` connects (or starts daemon), `event-step demo/Increment@1 {}` updates state via control `state`.
- Exiting REPL leaves pre-existing daemon running; stops auto-started daemon cleanly via control `shutdown`.
- REPL never instantiates `WorldHost`/adapters/timer heap directly; everything flows through the control channel.
