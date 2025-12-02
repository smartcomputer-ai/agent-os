# P5: Tests & Hardening

**Goal:** Align testing with WorldHost, add deterministic record/replay, and enforce guardrails once P1–P4 land.

## Focus Areas

- Host-backed test harness: lightweight facade over `aos-host::WorldHost` so integration tests follow the same paths as daemon/batch.
- Deterministic adapters: in-memory timer/http/llm shims with record/replay fixtures.
- Replay-or-die: replay from genesis to latest snapshot and assert byte-identical state.
- Policy/allowlist enforcement: cover host/size/model limits with negative cases.
- Snapshot boundary safety: alternate drain/execute/snapshot/reopen to catch persistence bugs.

## Tasks

1) Add `TestHost` helper mirroring CLI semantics (enqueue, drain, dispatch, apply receipts).
2) Deterministic timer/http/llm shims; record/replay helpers (feature-gated) with fixtures in `tests/data/`.
3) Integration tests for example worlds (counter, timer, http fetch, llm summarizer) via `TestHost`.
4) Replay-or-die check in CI; document the command in CONTRIBUTING.
5) Policy validation tests for adapter configs (allowlists, size/token limits, missing API key).

### Naming / packaging options (decide after P1–P4 land)

- Keep crate name `aos-testkit` and tighten scope in README/docs to “tests via WorldHost.”
- Or rename to `aos-testhost` to make the WorldHost dependency explicit. If we rename:
  - Update workspace members, imports, and CI scripts.
  - Optionally keep a temporary `aos-testkit` re-export for transition, then remove.
- Either way, deprecate old bespoke harnesses once `TestHost` covers them.

Decision point: revisit post P4 (REPL/daemon) when the host API stabilizes; pick the name then to avoid churn if APIs shift.

## Success Criteria

- `cargo test -p aos-host -p aos-testkit` passes without network when replay fixtures are used.
- Replay check proves state equality after reopen.
- Negative cases (blocked host, oversize body, missing API key) return error receipts, not panics.
