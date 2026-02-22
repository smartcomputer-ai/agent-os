# 11-plan-runtime-hardening

Single-scenario P3 fixture covering high-value runtime hardening paths with current AIR v1 features:

- correlation-safe gating (`triggers[].correlate_by` + `await_event.where`),
- subplan composition (`spawn_plan`/`await_plan`, `spawn_for_each`/`await_plans_all`),
- crash/resume while child plans are parked on `await_receipt`,
- deterministic replay parity,
- journal-derived plan summary artifact generation.

The scenario runs two concurrent requests, approves one first to verify cross-talk isolation, restarts during in-flight worker receipts, then completes both requests and verifies replay + summary invariants.
