# P2.5: Control Channel (Status: COMPLETE for socket path; stdio/CBOR/streaming/gov verbs deferred)

**Goal:** Ship a minimal, versioned control server around `WorldHost`/`WorldDaemon` so CLI/REPL/tests talk to a running world via one interface. Keep timers/adapter loop unchanged; keep batch mode untouched.

## Scope and Boundaries
- Provide **IO + translation only**: JSON/CBOR messages → `WorldHost` calls → responses. No adapter logic, no timer logic.
- Transport: local-only (Unix socket in world dir by default). Stdio mode deferred. Windows named pipe later; TCP/auth deferred.
- Governance verbs (propose/shadow/approve/apply) stay out of scope for this slice; add hooks for them in P3+.
 - Governance verbs (propose/shadow/approve/apply) must be first-class control methods later; do not route them through generic enqueue or domain events.

## Components

### ControlServer
- Runs inside `WorldDaemon`; owns a listener (Unix socket path under world dir, e.g., `.aos/control.sock`) and a request loop. On Windows, plan for a named pipe (e.g., `\\.\pipe\\aos-<world-id>`); stdio remains a fallback.
- Deserializes framed messages (JSON Lines first; CBOR optional), dispatches to `WorldHost`, sends a response envelope.
- Shares a channel with the daemon loop for wakeups; `shutdown` command triggers the same clean shutdown path as Ctrl-C; `step` should instruct the daemon to run `run_cycle(RunMode::WithTimers)` (not the batch cycle) so timer partitioning is preserved.
- Versioned protocol: `version`, `id`, `method`, `payload`, `error?`. Reject mismatched versions.

### ControlClient (library)
- Small helper used by `aos world` subcommands and the REPL to speak the protocol.
- Handles framing, request ids, timeouts, and fallbacks:
  - If socket missing, can optionally start a one-shot batch step (for CI) or surface a friendly error (for run-mode).
  - Optional `--stdio` to talk to an embedded server (useful for tests).

### Command Surface (MVP)
- `send-event {schema, value_cbor, key_cbor?}` → enqueue domain event; default is **no auto-drain**. Provide a convenience `send-event-and-step` (or `step {inject:[...]}`) for single-shot flows.
- `inject-receipt {receipt}` → enqueue receipt (same shape as `EffectReceipt`).
- `query-state {reducer, key?}` → returns raw CBOR bytes; higher-level decoding is client-side.
- `snapshot {}` → force snapshot.
- `journal-head {}` → return last seq/id for health checks.
- `shutdown {}` → graceful drain + snapshot + exit.
- `put-blob {data_b64}` → upload a blob to the world's CAS, return `HashRef`. Streaming/stdio upload deferred.
- Optional: `step {}` for a single `run_cycle(RunMode::WithTimers)` when daemon is running (batch mode continues to call `run_cycle` directly).
- Governance (P5+): add `propose/shadow/approve/apply` as explicit control verbs that call kernel governance APIs (not generic event enqueue); validate patches against `patch.schema.json` before submission and enforce sequencing on proposal_ids.

### Protocol Details
- **Envelope**: `{ "v": 1, "id": "<client-uuid>", "cmd": "<verb>", "payload": {...} }`
- **Response**: `{ "id": "<same>", "ok": true, "result": {...} }` or `{ "id": "<same>", "ok": false, "error": { "code": "...", "message": "..."} }`
- **Framing**: JSON Lines (`\n` delimited). CBOR framing deferred.
- **Errors**: use stable codes (`invalid_request`, `unknown_method`, `decode_error`, `host_error`, `timeout`, `not_running`).
- **Versioning**: reject requests without `v` or with `v != 1` using `invalid_request`; treat `v` as part of the public API so CLI/REPL can negotiate.
- **Request ids**: every response MUST echo the request `id`; responses for a given `id` are single-shot (no streaming) to keep pipelining deterministic.
- **Security**: local-only; set `SO_PEERCRED`/uid check on Unix if available; socket perms `0600`.
- **Optional event stream**: allow clients to subscribe to journal tail (`journal-appended` events) for live logs; NDJSON framing works well here.

### Integration with P1/P2/P4
- P2 (daemon+timers) remains unchanged; the daemon just spins ControlServer alongside its timer loop, wakes on control messages, and uses `run_cycle(RunMode::WithTimers)` for control-driven steps.
- P1 (batch) keeps direct `WorldHost` calls; batch CLI can optionally use ControlClient in `--stdio` mode for parity.
- P4 (REPL) must route all commands through ControlClient, not direct host access. CLI `aos world run/step` can reuse the same client.
- CLI behavior: commands check for a live control socket/pipe; if present, they attach and issue control verbs. If absent, `aos world run/dev` can start a daemon; `aos world step` falls back to batch-mode `WorldHost`.

## Tasks
1) Implement `ControlServer` (Unix socket) with request dispatch and version negotiation. *(done; stdio deferred)*
2) Implement `ControlClient` helper (framing, timeouts). *(done; retries/stdio deferred)*
3) Wire `WorldDaemon` to start ControlServer and honor `shutdown` commands. *(done for socket)*
4) Add CLI plumbing: `aos world run` connects to socket (or starts daemon), `aos world step` uses control when socket present. *(done for socket; stdio deferred)*
5) Add `put-blob` support in control server/client (base64 payload). *(done; streaming deferred)*
6) Tests: integration round-trips `send-event`/`inject-receipt`/`query-state`/`put-blob` through a running daemon. *(done)*
7) Docs: protocol reference + socket location + versioning + error codes; note governance verbs are TBD. *(spec/09-control.md drafted; stdio/streaming/CBOR/gov verbs remain deferred)*

## Success Criteria
- `aos world run <world>` starts a daemon with a listening control socket; `aos world step` and REPL operate via the control channel.
- Control commands (`send-event`, `inject-receipt`, `query-state`, `snapshot`, `shutdown`) succeed with responses; errors surface with stable codes.
- Version/ID invariants are enforced: missing/wrong `v` yields `invalid_request`; responses always echo `id`.
- Duplicate socket/start behavior is graceful (detect running daemon and reuse it; refuse double-start).
- REPL code no longer calls `WorldHost` directly; it relies on `ControlClient`.
