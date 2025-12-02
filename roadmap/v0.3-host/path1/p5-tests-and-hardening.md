# P5: Tests, Determinism, and Recording

**Goal:** Re-base tests on the new host runtime, add deterministic record/replay for adapters, and harden operational safeguards. This phase can be scheduled after P1–P4 land.

## Focus Areas

- **Host-backed test harness**: expose a lightweight test facade over `aos-host::runtime` so integration tests use the same code paths as the daemon and batch modes.
- **Deterministic adapters**: in-memory timer, HTTP, and LLM shims that can record receipts to fixtures and replay them byte-for-byte.
- **Replay-or-die checks**: ensure worlds replay from genesis to the latest snapshot with identical state; add a CI job that runs `cargo test -- --nocapture` plus a replay verification.
- **Policy/allowlist enforcement**: validate adapter allowlists (HTTP hosts, model names) in tests; add negative cases.
- **Load/snapshot safety**: fuzz-ish test that alternates drain/execute/snapshot/reopen to catch snapshot boundary bugs.

## Work Items

1. Add `aos-testkit` helpers that spin up `WorldRuntime` with in-memory store and deterministic adapters.
2. Add record/replay helpers (feature-gated) for HTTP and LLM adapters; fixtures under `tests/data/`.
3. Write integration tests for example worlds (`examples/00-counter`, timer demo, HTTP fetch, LLM summarizer) using the new helpers.
4. Add a replay check: open world → run step(s) → close → reopen → replay journal → assert state and receipts match.
5. Add allowlist/limit tests (HTTP host deny, response size limit, missing API key for LLM).
6. Add CI target `cargo test -p aos-host -p aos-testkit` plus replay job; document in CONTRIBUTING.

## Deliverables

- `aos-testkit` utilities that mirror CLI semantics (enqueue, drain, execute, apply receipts).
- Fixture-backed record/replay adapters for HTTP/LLM (opt-in feature flags to keep default builds lean).
- Integration tests covering happy-path and failure-path for timers, HTTP, LLM.
- CI docs/commands showing how to run replay verification locally and in CI.

## Out-of-Scope (ok to punt further)

- Full fuzzing or long-running soak tests.
- Real cryptographic signing for receipts (can stay stubbed until a later milestone).
