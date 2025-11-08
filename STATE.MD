# AgentOS Dev State – November 2025

## Recently Completed
- **Core primitives**: `aos-cbor`, `aos-air-types`, `aos-store` fully working (canonical CBOR, AIR schema models/validation, CAS backends, manifest loader).
- **Expression runtime**: `aos-air-exec` evaluator/value model covers all Expr ops with good tests.
- **Effects layer**: `aos-effects` now has effect kinds/intents/receipts, capability grants/budgets, adapter/policy traits, and typed structs for built-ins.
- **WASM stack**: `aos-wasm-abi` canonical IO envelopes; `aos-wasm` deterministic Wasmtime runner with tests.
- **Kernel**:
  - Reducer loop wired to manifest loader, reducer registry/cache, scheduler, effect queue, reducer state tracking.
  - Plan runtime v1: manifest triggers start plan instances; steps support `assign`, `emit_effect`, `await_receipt`, `await_event`, `raise_event`, `end`. Plans track effect handles, park on receipts/events, resume on delivery, and raise events back to reducers.
  - Scheduler multiplexes reducer events and plan ticks; effect manager returns intent hashes for receipt routing; kernel handles receipts for both reducers (future) and plans.
  - Tests cover reducer → plan → effect pipeline with receipt handling.

## Current Gaps / TODOs
- **Plan DAG semantics**: edges/guards are not evaluated; we currently execute steps sequentially. Need a per-step readiness model honoring predecessors + `edges[].when` expressions.
- **Capability/policy/budget enforcement**: still stubbed; effect emission always allowed.
- **Reducer receipt delivery**: plan side implemented, reducer micro-effects (timer/blob) still need receipt-to-event plumbing.
- **Journal/outbox**: no persistent journal yet; world state/effects are in-memory only.

## Immediate Next Steps
1. **Plan edges/guards**
   - Track predecessor completion and evaluate guard expressions before scheduling steps, so branching matches AIR spec.
   - Honor `end.result` schema (validate output expression).
2. **Reducer receipt events**
   - Convert micro-effect receipts into DomainEvents and deliver to reducers, mirroring the plan receipt flow.
3. **Capability/policy enforcement** (after above)
   - Integrate Cap/Policy gates + budget tracking across reducers/plans.

## Medium-Term Plan
- Add journaling/snapshots for deterministic replay and persistence.
- Expand adapter integration + CLI for running worlds, dispatching effects, ingesting receipts.
- Flesh out governance loop once runtime spine is stable.
- Iterate towards plan v1.1 features once v1.0 semantics are fully covered.

Keep this file updated as each milestone lands.
