# P2.5: Control Channel

**Goal:** Ship a minimal, versioned control server around `WorldHost`/`WorldDaemon` so CLI/REPL/tests talk to a running world via one interface. Keep timers/adapter loop unchanged; keep batch mode untouched.

## Scope and Boundaries
- Provide **IO + translation only**: JSON/CBOR messages → `WorldHost` calls → responses. No adapter logic, no timer logic.
- Transport: local-only (Unix socket in world dir by default); optional stdio mode for embedding/debug. TCP/auth deferred.
- Governance verbs (propose/shadow/approve/apply) stay out of scope for this slice; add hooks for them in P3+.

## Components

### ControlServer
- Runs inside `WorldDaemon`; owns a listener (Unix socket path under world dir, e.g., `.aos/control.sock`) and a request loop.
- Deserializes framed messages (JSON Lines first; CBOR optional), dispatches to `WorldHost`, sends a response envelope.
- Shares a channel with the daemon loop for wakeups; `shutdown` command triggers the same clean shutdown path as Ctrl-C.
- Versioned protocol: `version`, `id`, `method`, `payload`, `error?`. Reject mismatched versions.

### ControlClient (library)
- Small helper used by `aos world` subcommands and the REPL to speak the protocol.
- Handles framing, request ids, timeouts, and fallbacks:
  - If socket missing, can optionally start a one-shot batch step (for CI) or surface a friendly error (for run-mode).
  - Optional `--stdio` to talk to an embedded server (useful for tests).

### Command Surface (MVP)
- `send-event {schema, value_cbor, key_cbor?}` → enqueue domain event, drain optional? (server does not auto-drain; client can request `step` separately).
- `inject-receipt {receipt}` → enqueue receipt (same shape as `EffectReceipt`).
- `query-state {reducer, key?}` → returns raw CBOR bytes; higher-level decoding is client-side.
- `snapshot {}` → force snapshot.
- `journal-head {}` → return last seq/id for health checks.
- `shutdown {}` → graceful drain + snapshot + exit.
- Optional: `step {}` for a single `drain_and_execute` cycle so batch-mode CLI and REPL can reuse the same verb.

### Protocol Details
- **Envelope**: `{ "version": 1, "id": "<client-uuid>", "method": "<verb>", "payload": {...} }`
- **Response**: `{ "id": "<same>", "ok": true, "result": {...} }` or `{ "id": "<same>", "ok": false, "error": { "code": "...", "message": "..."} }`
- **Framing**: JSON Lines (`\n` delimited). CBOR framing is optional via `--cbor` flag negotiated at connect time.
- **Errors**: use stable codes (`invalid_request`, `unknown_method`, `decode_error`, `host_error`, `timeout`, `not_running`).
- **Security**: local-only; set `SO_PEERCRED`/uid check on Unix if available; socket perms `0600`.

### Integration with P1/P2/P4
- P2 (daemon+timers) remains unchanged; the daemon just spins ControlServer alongside its timer loop and wakes when commands arrive.
- P1 (batch) keeps direct `WorldHost` calls; batch CLI can optionally use ControlClient in `--stdio` mode for parity.
- P4 (REPL) must route all commands through ControlClient, not direct host access. CLI `aos world run/step` can reuse the same client.

## Tasks
1) Implement `ControlServer` (Unix socket + stdio mode) with request dispatch and version negotiation.
2) Implement `ControlClient` helper (framing, retries, timeouts, optional stdio).
3) Wire `WorldDaemon` to start ControlServer and honor `shutdown` commands.
4) Add CLI plumbing: `aos world run` connects to socket (or starts daemon), `aos world step` may use stdio mode for hermeticity.
5) Tests: unit tests for protocol framing/dispatch; integration test that round-trips `send-event`/`inject-receipt`/`query-state` through a running daemon.
6) Docs: protocol reference + socket location + versioning + error codes; note governance verbs are TBD.

## Success Criteria
- `aos world run <world>` starts a daemon with a listening control socket; `aos world step` and REPL operate via the control channel.
- Control commands (`send-event`, `inject-receipt`, `query-state`, `snapshot`, `shutdown`) succeed with responses; errors surface with stable codes.
- Duplicate socket/start behavior is graceful (detect running daemon and reuse it; refuse double-start).
- REPL code no longer calls `WorldHost` directly; it relies on `ControlClient`.
