# P6 (Optional): Deterministic Execution Bounding (Fuel-Based Compute Limits)

**Priority**: P3 (optional hardening)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`

## Goal

Harden workflow execution with deterministic compute limits beyond effect/event/output caps.

Removing the single-effect constraint and adding max effects/events/bytes per tick is necessary, but not sufficient once orchestration lives in module code. The kernel must also bound WASM compute deterministically.

## Why This Exists

Without deterministic compute bounding, a workflow can:

1. spin indefinitely,
2. create runaway compute denial-of-service,
3. make replay-or-die impractical at scale.

This is a kernel invariant-level requirement, not optional runtime polish.

## Scope

### 1) Add deterministic WASM fuel budgets

1. Add `max_wasm_fuel_per_tick`.
2. Optionally add `max_wasm_fuel_per_world_cycle` for host-cycle fairness.
3. Enforce fuel accounting in the kernel/wasm boundary in a replay-identical way.

### 2) Explicitly avoid nondeterministic wall-clock guards

1. Do not enforce `max_wasm_time_per_tick` using wall clock.
2. If time-based telemetry is emitted, keep it diagnostic-only and out of state-transition decisions.

### 3) Add deterministic faulting semantics

1. Define a deterministic workflow/module fault record when fuel is exceeded.
2. Record fault as a journaled event, for example:
   - `WorkflowFault { reason: FuelExceeded, module, instance_key, seq }`.
3. Ensure fault handling is traceable in debug/tail/trace outputs.

### 4) Preserve replay determinism

1. Fuel depletion and resulting faults must replay identically from genesis.
2. Snapshot/load/replay must preserve behavior around near-limit and over-limit executions.

## Out of Scope

1. Wall-clock deadline enforcement in kernel control flow.
2. Non-deterministic host preemption policies.
3. Distributed scheduling redesign.

## Work Items by Crate

### `crates/aos-wasm`

1. Expose deterministic fuel metering controls required by kernel.
2. Return structured trap/fault details for fuel exhaustion.

### `crates/aos-kernel`

1. Add configurable fuel limits to runtime config.
2. Charge/debit fuel per module tick and optional per world cycle.
3. Map fuel exhaustion to deterministic workflow fault record and state transition.
4. Ensure governance/quiescence and trace surfaces include fuel-fault visibility.

### `crates/aos-host` / `crates/aos-cli`

1. Surface fuel fault reason in trace/tail/control outputs.
2. Add operator-facing knobs/documentation for deterministic fuel limits.

### `crates/aos-kernel/tests` and `crates/aos-host/tests`

1. Add tests for fuel exhaustion faults.
2. Add replay-or-die tests proving byte-identical outcomes across fuel boundary cases.
3. Add fairness tests (if per-world-cycle budget is enabled).

## Acceptance Criteria

1. Workflow/module execution is bounded by deterministic fuel per tick.
2. No wall-clock time limit influences state transitions or fault decisions.
3. Fuel exhaustion produces deterministic journaled faults (`FuelExceeded`) and traceable diagnostics.
4. Replay from genesis reproduces identical fuel-fault behavior and snapshots.

