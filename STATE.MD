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
- `crates/aos-kernel/src/world.rs` still only has the original smoke test covering reducer → plan → effect happy path. No integration tests exist yet for receipts, waiting events, or reducer receipt handling.

### Tests to add next (integration level)
1. **Plan receipt routing**: plan emits effect, waits, adapter receipt via `Kernel::handle_receipt` resumes the plan; verify bind var populated, pending_receipts cleared.
2. **Plan event wake-up**: multiple plan instances waiting on different schemas; deliver DomainEvent matching one schema and ensure only that plan is re-queued/run.
3. **Reducer receipt delivery**: (once reducer micro-effect receipt flow is implemented) convert receipts into DomainEvents for reducers.
4. **Guarded branching end-to-end**: manifest plan with guard false vs true; assert effect queue outcomes and plan completion.
5. **Raised event propagation**: plan raises an event that is routed to reducers via scheduler.

Plan is to create helper builders (reducer module, manifest with plan, plan inputs) so tests stay concise. Tests should live either alongside `world.rs` (current module) or under `crates/aos-kernel/tests/` if they grow.

### After integration tests
1. **Reducer receipt handling** – convert micro-effect receipts into reducer DomainEvents.
2. **Capability/policy enforcement** – integrate Cap/Policy gates and budget tracking. We can wait on this, though, until later, the allow all approach works for now.
3. **Journal/outbox** – persist events/effects for replay.
4. **Governance loop & CLI integration** – once runtime is stable.

Keep this file in sync as each milestone lands.
