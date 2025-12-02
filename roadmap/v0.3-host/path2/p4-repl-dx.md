# P4: REPL & DX (path2)

**Goal:** Provide a pleasant interactive loop (`aos dev`) talking to a running world (auto-starts daemon if needed).

## REPL Shape

- Lives in `aos-host::repl`.
- Uses `rustyline` for input/history; colored output for events/effects/receipts.
- Commands (short aliases): `help`, `event <schema> <json>`, `receipt <json>`, `state [reducer]`, `effects`, `timers`, `step`, `run` (polling loop), `snapshot`, `manifest`, `quit`.
- REPL sends commands over the same control channel the daemon already exposes; if no daemon, start one in-process for the session.

## CLI

- `aos dev <path> [--template minimal|counter|chat]`
  - If path missing: scaffold template (manifest + sample reducer/plan).
  - Starts daemon with default adapters (timer/http, llm if available).
  - Drops user into REPL; Ctrl-C exits REPL then stops daemon cleanly.

## Tasks

1) Implement control-channel client in `aos-host` (Unix socket/stdin JSON), reused by CLI and REPL.
2) Implement REPL commands mapping to control ops; pretty printers for events/effects/receipts/state.
3) Add scaffolding helper for templates (minimal, counter, chat).
4) Wire `aos dev` in `aos-cli` to start daemon if absent, then launch REPL.
5) Persist history under `~/.local/share/aos/repl_history` (or platform equiv).

## Success Criteria

- `aos dev examples/00-counter` allows sending `event demo/Increment@1 {}` and seeing state update.
- REPL works whether daemon already running or not.
- Exiting REPL shuts down daemon cleanly when auto-started.
