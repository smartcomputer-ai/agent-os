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
1. **Reducer receipt delivery** – covered via the new `reducer_receipt_is_converted_into_event` test; follow-up scenarios can exercise blob receipts once adapters land.

Plan is to create helper builders (reducer module, manifest with plan, plan inputs) so tests stay concise. Tests should live either alongside `world.rs` (current module) or under `crates/aos-kernel/tests/` if they grow.

### After integration tests
1. **Reducer receipt handling** – kernel now tracks reducer-origin effects, converts `timer.set` and `blob.{put,get}` receipts into `sys/*` events, and logs late receipts. Next step: wire updated schemas/routing plus budget settlement off the new structured payload hook.
2. **Capability/policy enforcement** – capability resolution + grant plumbing are now wired (plans and reducer slots must reference real grants, and effect intents resolve them before enqueue). Policy is still `AllowAll`; We will implementing real rule matching + budget enforcement later.
3. **Journal/outbox** – persist events/effects for replay.
4. **Governance loop & CLI integration** – once runtime is stable.

Keep this file in sync as each milestone lands.
