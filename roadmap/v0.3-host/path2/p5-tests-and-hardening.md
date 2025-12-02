# P5: Tests & Hardening (path2)

**Goal:** Align testing with the host runtime, add deterministic record/replay, and enforce guardrails.

## Priorities

- Host-backed test harness in `aos-testkit` that uses `WorldRuntime` + in-memory store and deterministic adapters.
- Record/replay for HTTP/LLM (feature-gated) with fixtures under `tests/data/`.
- Replay verification: open world → run steps → close → reopen and replay journal → byte-identical state/snapshots.
- Allowlist/limit tests (HTTP host block, body-size cap, missing LLM key).
- Snapshot boundary fuzz: alternate drain/execute/snapshot/reopen to catch persistence bugs.

## Tasks

1) Add `TestHost` helper mirroring CLI semantics (enqueue, drain, execute, apply receipts).
2) Deterministic timer/http/llm shims; record/replay helpers (opt-in features).
3) Integration tests for example worlds (counter, timer, http fetch, llm summarizer) via `TestHost`.
4) Replay-or-die check in CI + doc the command in CONTRIBUTING.
5) Basic policy validation tests for adapter configs (allowlists, size/token limits).

## Success Criteria

- `cargo test -p aos-host -p aos-testkit` passes without network when replay fixtures are used.
- Replay check proves state equality after reopen.
- Negative cases (blocked host, oversize body, missing API key) return error receipts, not panics.
