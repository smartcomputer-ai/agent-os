# P2: Kernel Plan Runtime Cutover (Execution Path Removal)

**Priority**: P1  
**Status**: In Progress  
**Depends on**: `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`

## Implementation Status

### Scope
- [ ] Scope 1.1: Remove `Task::Plan` and plan queue from scheduler.
- [x] Scope 1.2: Remove `start_plans_for_event` and all plan task dispatch in event loop.
- [ ] Scope 1.3: Remove plan wait/spawn/await runtime machinery.
- [ ] Scope 2.1: Remove plan instance maps, waiters, pending plan receipts, completion caches.
- [x] Scope 2.2: Remove plan-specific replay identity reconciliation.
- [x] Scope 2.3: Manifest apply quiescence now uses workflow strict-quiescence checks.
- [x] Scope 2.4: Module-workflow pending receipt state retained and keyed by intent/origin tuple.
- [x] Scope 2.5: Kernel-recognized workflow instance records retained (`status`, `inflight_intents`, `last_processed_event_seq`).
- [ ] Scope 3.1: Remove/replace plan debug surfaces.
- [ ] Scope 3.2: Introduce workflow-equivalent diagnostics where required.
- [x] Scope 4.1: Event routing determinism unchanged.
- [x] Scope 4.2: Effect enqueue/receipt handling determinism unchanged.
- [x] Scope 4.3: Snapshot/replay continues to work for non-plan flows.
- [x] Scope 4.4: Receipt routing remains manifest-independent (origin identity only).
- [x] Scope 4.5: Workflow instance state transitions remain deterministic/replay-identical.
- [x] Scope 4.6: Structural module authority guardrails remain enforced.
- [x] Scope 4.7: Manifest apply decisions remain deterministic under strict-quiescence rules.

### Work Items by Crate
- [x] `crates/aos-kernel/src/world/event_flow.rs`: removed plan-event dispatch from active domain event flow.
- [x] `crates/aos-kernel/src/world/mod.rs`: removed plan replay intent reconciliation from active replay path.
- [x] `crates/aos-kernel/src/world/plan_runtime.rs`: removed active plan-receipt wakeup handling path in kernel receipt ingress.
- [x] `crates/aos-kernel/src/world/governance_runtime.rs`: switched apply-quiescence checks to workflow instance + inflight intent model.
- [x] `crates/aos-host/tests/journal_integration.rs`: retired plan-runtime assertions (ignored) and kept workflow no-plan coverage active.
- [x] `crates/aos-host/tests/{cap_enforcer_e2e,demiurge_introspect_manifest_e2e,governance_plan_integration}.rs`: marked plan-trigger fixtures retired (ignored pending workflow-native replacements).
- [x] `crates/aos-host/src/control.rs`: fixed `workspace-read-bytes` internal effect receipt decoding for canonical byte payloads.
- [ ] `crates/aos-host/tests/policy_integration.rs`: migrate plan-policy assertions to workflow-policy fixtures.
- [ ] `crates/aos-kernel/src/scheduler.rs`: remove `Task::Plan`/plan queue.
- [ ] `crates/aos-kernel/src/world/plan_runtime.rs` + `crates/aos-kernel/src/plan/*`: retire remaining plan execution machinery.
- [ ] `crates/aos-host/src/trace.rs` + related APIs: replace plan debug surfaces with workflow diagnostics.

### Acceptance Criteria
- [ ] AC1: Kernel has no active plan scheduling/ticking path.
- [x] AC2: New workflows continue to execute end-to-end under host cycle.
- [ ] AC3: All remaining tests pass without relying on plan runtime state.
- [x] AC4: Manifest apply quiescence checks do not depend on plan internals.
- [x] AC5: Receipt wakeups remain deterministic under concurrent in-flight module instances.
- [x] AC6: Pending/waiting workflow instances survive snapshot-load-replay with identical inflight intent sets (non-plan fixtures).
- [x] AC7: Runtime has no plan-or-reducer-specific authority dependency for effect emission.
- [x] AC8: Manifest apply fails while any in-flight workflow instances/intents exist, with deterministic block reasons.

## Goal

Remove plan execution from the kernel runtime so orchestration is exclusively workflow-module-driven.

After this phase, plans may still exist in data models temporarily, but the runtime will not execute plan instances.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

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
3. Replace manifest apply quiescence checks with workflow strict-quiescence checks:
   - no non-terminal workflow instances,
   - no in-flight intents,
   - no queued effects/scheduler work.
4. Introduce/retain module-workflow pending receipt state keyed by `intent_id` and resolved to `(origin_module_id, origin_instance_key)`.
5. Introduce/retain kernel-recognized workflow instance records carrying `status`, `inflight_intents`, and `last_processed_event_seq`.

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
4. Receipt routing is manifest-independent and uses recorded origin identity only.
5. Workflow instance state transitions are deterministic and replay-identical.
6. Structural module authority guardrails remain enforced during runtime cutover.
7. Manifest apply decisions are deterministic under strict-quiescence rules.

## Out of Scope

1. AIR schema removal (`DefPlan`, manifest plan refs/triggers).
2. CLI/host command cleanup beyond what is required to compile.
3. Spec cleanup.

## Work Items by Crate

### `crates/aos-kernel`

1. Remove/retire `plan/*` and `world/plan_runtime.rs` execution wiring.
2. Update `world/mod.rs`, `world/event_flow.rs`, `scheduler.rs`, `world/governance_runtime.rs`.
3. Remove plan-origin `IntentOriginRecord` production from runtime paths.
4. Ensure receipt wakeup path targets module instances by recorded origin tuple, not router subscriptions.
5. Ensure manifest apply quiescence uses workflow instance/inflight state instead of plan-only counters.
6. Ensure effect emission path resolves module authority as workflow (not reducer) for post-plan semantics.
7. Replace apply-path state clearing assumptions with strict precondition checks that fail closed when in-flight workflow state exists.

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
5. Receipt wakeups remain deterministic under concurrent in-flight module instances.
6. Pending/waiting workflow instances survive snapshot-load-replay with identical inflight intent sets.
7. Runtime has no plan-or-reducer-specific authority dependency for effect emission.
8. Manifest apply fails while any in-flight workflow instance/intents exist, with deterministic block reasons.
