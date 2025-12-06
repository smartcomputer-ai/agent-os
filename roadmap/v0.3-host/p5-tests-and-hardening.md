# P5: Tests & Hardening

**Goal (trimmed):** Finish a practical host-first test story and add replay tooling, without overbuilding. Focus on TestHost parity, example coverage, a replay command, and a small snapshot safety check.

## Focus Areas

- TestHost parity for near-term needs (matches daemon/batch loops).
- Examples + CLI smoke use TestHost with mocked effects (no network).
- Replay command for manual/CI experiments (no doc/CI wiring yet).
- Snapshot boundary sanity check at the WorldHost layer (small, high-signal).

## Tasks

1) **TestHost affordances (right-now scope).** Add helpers for: `run_to_idle`, `run_cycle_with_timers` (daemon-style), `drain_and_dispatch` (drain + adapter exec), `apply_receipt`, and `state_json`. Keep API tight; skip pending_effects/record/replay plumbing.
2) **Examples through TestHost.** Port counter + hello-timer (and optionally fetch-notify/llm-summarizer) to integration tests that use TestHost + mock adapters. Keep `crates/aos-examples/tests/cli.rs` as a smoke test but ensure it routes through host/mocks (no real HTTP/LLM).
3) **Replay command.** Add `aos world replay` to `aos-cli`: open world, replay to head, assert success, print heights/hash. Mark experimental; no CONTRIBUTING/CI hook yet.
4) **Snapshot safety (lightweight).** Add one WorldHost-level test: drain → snapshot → reopen → continue (ensures queued intents/awaits persist). Keep if effort stays low; otherwise defer explicitly.

## Explicitly deferred for now

- Recording/replay fixtures or adapters.
- Host-level allowlists/size/model limits (handled by caps/policies instead).
- CI wiring or docs for replay beyond the new command.

## Success Criteria

- TestHost helpers in place and exercised by new integration tests.
- Example flows run under TestHost + mock adapters; CLI smoke remains network-free.
- `aos world replay` works locally for manual runs.
- Snapshot safety test passes (or is consciously deferred). 
