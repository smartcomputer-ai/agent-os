# Example 08 — Retry with Exponential Backoff (Reducer-driven)

A runnable blueprint for reducer-driven retries: reducer owns attempt counting and timers; plan just tries the work and reports back.

## What it does
- `StartWork` event kicks the reducer.
- Reducer emits `WorkRequested` intent and tracks `attempt = 1` with config `max_attempts`, `base_delay_ms`, `anchor now_ns`.
- Trigger starts `WorkPlan`, which (in this minimal example) always reports a transient failure (`WorkErr transient=true`).
- Reducer schedules `timer.set` with exponential backoff (`base_delay_ms * 2^(attempt-1)`) until `max_attempts` is hit, then marks `Failed`. A `WorkOk` event would mark `Done` immediately.

## Layout
```
crates/aos-smoke/fixtures/08-retry-backoff/
  air/
    schemas.air.json      # StartWork, WorkRequested, WorkOk, WorkErr, RetryEvent, RetryState
    module.air.json       # defmodule demo/RetrySM@1 (reducer)
    plans.air.json        # defplan demo/WorkPlan@1 (raises WorkErr)
    capabilities.air.json # timer cap
    policies.air.json     # allow-all policy
    manifest.air.json     # wires routing + trigger + cap grant
  reducer/
    Cargo.toml
    src/lib.rs            # reducer state machine and backoff logic
```

## How backoff is computed
```
delay_ms = base_delay_ms * 2^(attempt-1)
delay_ns = delay_ms * 1_000_000
deliver_at_ns = anchor_ns + delay_ns
```
The timer key is set to `req_id` to ease correlation/diagnostics.

## To run/build the reducer
```
cargo build -p retry_sm --release --target wasm32-unknown-unknown
```
(You can swap the placeholder `wasm_hash` in `module.air.json` with the built artifact's hash.)

## To make the plan succeed
Replace the single `raise_event` in `plans.air.json` with your real effect/receipt handling that raises `WorkOk` when successful and `WorkErr` with `transient=false` on terminal failure.

## Why reducer-driven?
- Retry policy lives in deterministic state; journal shows every attempt and timer.
- Plans stay thin and audited; no retry state in external orchestration.
- Works with reducer micro-effect `timer.set`, avoiding heavy effects in reducers.
