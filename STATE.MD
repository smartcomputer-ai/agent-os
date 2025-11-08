# AgentOS Dev State – November 2025

## Current Focus: Kernel Testing & Remaining Work

### Tests in place (unit level)
- `crates/aos-kernel/src/plan.rs` now has unit tests for:
  - Assign steps updating the environment.
  - Emit effect steps enqueueing intents and storing effect handles.
  - Await receipt waiting/resuming behavior (hash mapping, deliver_receipt handshake).
  - Await event waiting on schemas and resuming on DomainEvents.
  - Guards blocking downstream steps when false.
  - Raise event producing DomainEvents.

### Tests in place (integration level)
- `crates/aos-kernel/src/world.rs` now has:
  - Smoke test for reducer → plan → effect happy path.
  - Plan receipt routing coverage ensuring receipts resume waiting plans and populate bindings.
  - Plan event wake-up coverage ensuring only matching schemas resume queued plans.
  - Guarded branching coverage verifying guarded effects complete only on the true path and remain pending otherwise.
  - Raised event propagation coverage validating plan-raised events reach reducers through the scheduler.

### Tests to add next (integration level)
1. **Reducer receipt delivery**: (once reducer micro-effect receipt flow is implemented) convert receipts into DomainEvents for reducers and assert reducer/event routing.

Plan is to create helper builders (reducer module, manifest with plan, plan inputs) so tests stay concise. Tests should live either alongside `world.rs` (current module) or under `crates/aos-kernel/tests/` if they grow.

### After integration tests
1. **Reducer receipt handling** – convert micro-effect receipts into reducer DomainEvents.
2. **Capability/policy enforcement** – integrate Cap/Policy gates and budget tracking. We can wait on this, though, until later, the allow all approach works for now.
3. **Journal/outbox** – persist events/effects for replay.
4. **Governance loop & CLI integration** – once runtime is stable.

Keep this file in sync as each milestone lands.
