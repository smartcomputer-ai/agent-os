# P2: Kernel Plan Runtime Cutover (Execution Path Removal)

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`

## Goal

Remove plan execution from the kernel runtime so orchestration is exclusively module-driven.

After this phase, plans may still exist in data models temporarily, but the runtime will not execute plan instances.

## Hard-Break Assumptions

1. Existing plan-based worlds may fail at runtime.
2. Journal replay compatibility with old plan runs is not required.
3. Debug APIs can be renamed or deleted even if callers break.

## Scope

### 1) Remove plan scheduling and ticking

1. Remove `Task::Plan` and plan queue from scheduler.
2. Remove `start_plans_for_event` and all plan task dispatch in event loop.
3. Remove plan wait/spawn/await runtime machinery.

### 2) Remove in-memory plan runtime state

1. Remove plan instance maps, waiters, pending plan receipts, completion caches.
2. Remove plan-specific replay identity reconciliation.
3. Replace manifest apply quiescence checks with module/effect queue-only checks.

### 3) Replace runtime plan debug surfaces

1. Remove or replace:
   - `pending_plan_receipts`,
   - `debug_plan_waits`,
   - `debug_plan_waiting_events`,
   - `plan_name_for_instance`,
   - `recent_plan_results`.
2. Introduce module-workflow equivalent diagnostics where required.

### 4) Keep control-plane behavior deterministic

1. Event routing determinism unchanged.
2. Effect enqueue/receipt handling determinism unchanged.
3. Snapshot and replay continue to work for non-plan flows.

## Out of Scope

1. AIR schema removal (`DefPlan`, manifest plan refs/triggers).
2. CLI/host command cleanup beyond what is required to compile.
3. Spec cleanup.

## Work Items by Crate

### `crates/aos-kernel`

1. Remove/retire `plan/*` and `world/plan_runtime.rs` execution wiring.
2. Update `world/mod.rs`, `world/event_flow.rs`, `scheduler.rs`, `world/governance_runtime.rs`.
3. Remove plan-origin `IntentOriginRecord` production from runtime paths.

### `crates/aos-host`

1. Remove runtime calls that assume active plan instances.
2. Keep host cycle behavior intact for module-origin effects.

### `crates/aos-kernel/tests` and `crates/aos-host/tests`

1. Delete/replace plan-runtime integration tests.
2. Add equivalent module-workflow runtime tests.

## Acceptance Criteria

1. Kernel has no active plan scheduling/ticking path.
2. New workflows continue to execute end-to-end under host cycle.
3. All remaining tests pass without relying on plan runtime state.
4. Manifest apply quiescence checks do not reference plan internals.
