# P5: Tests & Hardening

**Goal (trimmed):** Finish a practical host-first test story and add replay tooling, without overbuilding. Focus on TestHost parity, example coverage, a replay command, and a small snapshot safety check.

## Focus Areas

- TestHost parity for near-term needs (matches daemon/batch loops).
- Examples + CLI smoke use TestHost with mocked effects (no network).
- Replay command for manual/CI experiments (no doc/CI wiring yet).
- Snapshot boundary sanity check at the WorldHost layer (small, high-signal).

## Tasks

1) **TestHost affordances (right-now scope).** ✅ Added helpers: `run_to_idle`, `run_cycle_with_timers` (daemon-style), `drain_and_dispatch`, `apply_receipt`, `state_json`; kept API tight and skipped pending_effects/record/replay plumbing.
2) **Examples through TestHost.** ✅ Counter, hello-timer, blob-echo, fetch-notify, aggregator, chain-comp, retry-backoff, safe-upgrade, llm-summarizer now run through `ExampleHost` (TestHost-based) with mock adapters; CLI smoke remains opt-in and still mock-only.
3) **Replay command.** ✅ `aos world replay` added to `aos-cli` (experimental; no CONTRIBUTING/CI wiring).
4) **Snapshot safety (lightweight).** ✅ WorldHost-level test added (`crates/aos-host/tests/host_snapshot.rs`) to ensure queued intents persist across snapshot/reopen.

## Explicitly deferred for now

- Recording/replay fixtures or adapters.
- Host-level allowlists/size/model limits (handled by caps/policies instead).
- CI wiring or docs for replay beyond the new command.

## Success Criteria

- TestHost helpers in place and exercised by new integration tests.
- Example flows run under TestHost + mock adapters; CLI smoke remains network-free.
- `aos world replay` works locally for manual runs.
- Snapshot safety test passes (or is consciously deferred). 
