# P1.5: Minimal Host Test Harness (pulled forward)

**Goal:** Bring the smallest slice of `p5-tests-and-hardening` forward so P1/P2 can be validated with the same `WorldHost::run_cycle` surface (including `RunMode::WithTimers` once timers land).

## Scope (keep it tiny)
- `TestHost` wrapper around `WorldHost` with helpers: `send_event`, `inject_receipt`, `run_cycle_batch()`, `run_cycle_with_timers(&mut TimerScheduler)`, `state_json`.
- Deterministic stub adapters (no network) that return canned receipts; timer shim can immediately succeed but should exercise the partitioning path.
- Replay smoke check: open world → run one cycle → snapshot → reopen → replay tail → assert reducer state equality.

## Tasks
1) Add `TestHost` in `crates/aos-host` (feature-gated for tests) that constructs `WorldHost` + stub adapters and exposes the minimal helpers above.
2) Add fixtures for `examples/00-counter` using `run_cycle_batch` and `examples/01-hello-timer` using `RunMode::WithTimers` (timer fires immediately via shim).
3) Wire a replay smoke test that reopens from snapshot and asserts state equality.

## Success Criteria
- Integration tests can drive the host through `run_cycle` (both modes) without bespoke harnesses.
- Counter + timer examples pass with stub adapters and replay equality holds.
