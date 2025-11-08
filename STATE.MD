# AgentOS Dev State – November 2025

## Recently Completed
- **Core primitives**: `aos-cbor`, `aos-air-types`, and `aos-store` implemented with canonical CBOR, AIR schema models/validation, and CAS (fs/mem) + manifest loader.
- **Expression & value runtime**: `aos-air-exec` now evaluates the full Expr AST, enforces type errors, and has broad unit coverage.
- **Effects layer**: `aos-effects` defines effect kinds, intents, receipts, capability grants/budgets, adapter/policy traits, and typed structs for the v1 built-in effects.
- **WASM stack**: `aos-wasm-abi` provides canonical reducer envelopes; `aos-wasm` wraps Wasmtime in a deterministic runner with tests.
- **Kernel (current focus)**:
  - Reducer loop: manifest ingestion, reducer registry/cache, event scheduler, state tracking, effect queue.
  - Plan runtime v1: manifest triggers start plan instances; steps support `assign`, `emit_effect`, `await_receipt`, `raise_event`, and `end`. Plans now track effect handles, park on receipts, resume when `EffectReceipt` arrives, and can raise reducer events.
  - Scheduler now multiplexes reducer events and plan ticks; effect manager returns intent hashes so receipts can map back to plan instances.
  - tests exercise reducer → plan → effect flow end-to-end.

## Crate Status Snapshot
- **aos-cbor**: stable helpers, schema hash regression tests.
- **aos-air-types**: full AIR schema surface + semantic validators.
- **aos-air-exec**: evaluator + value model with tests for literals, arithmetic, collections, error cases.
- **aos-effects**: rich types for intents/receipts/caps, helper builders, effect registry stubs.
- **aos-store**: manifest loader + Catalog, FS and mem stores, manifest plan validation.
- **aos-wasm-abi**: versioned ABI; encode/decode tests.
- **aos-wasm**: runtime invoking `step` via Wasmtime; integration test with WAT stub.
- **aos-kernel**: reducers + plans + effect queue operating deterministically with receipt routing for plan steps.

## Immediate Next Steps
1. **Full plan feature parity**
   - Implement `await_event`, guards, invariants, and `end.result` validation.
   - Persist plan instance state/mailboxes for long-running flows (tests currently in-memory only).
2. **Receipt routing to reducers**
   - Reducer micro-effects (timer/blob) need receipt events delivered back; hook this into `Kernel::handle_receipt` alongside plans.
3. **Capability & policy enforcement**
   - Replace allow-all stubs with real gates using manifest grants/policies, budget tracking, and effect origin metadata.
4. **Journal/outbox plumbing**
   - Persist events/effects to a journal abstraction; expose deterministic replay path.
5. **Reducer state storage**
   - Move in-memory reducer state to CAS blobs (or structured snapshots) and hook into snapshot/restore story.

## Medium-Term Plan
- Expand adapter integration + CLI commands (effect dispatch loop, receipt ingestion, manual approvals).
- Flesh out governance loop (proposal/shadow/approve/apply) once runtime is stable.
- Enhance testkit/CLI to spin up in-memory worlds, run reducers/plans, and assert effect/receipt flows.
- Implement plan v1.1 features gradually (cells, spawn_for_each, etc.) once v1.0 is rock solid.

Keep this file updated as each milestone lands.
