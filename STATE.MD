# AgentOS Dev State – November 2025

## Upcoming Work Packages

Each package is scoped so an engineer can take it end-to-end. Tackle them roughly in order; later packages assume earlier infrastructure is in place.

### Test Ownership & Coverage Boundaries
- **Kernel crate (`crates/aos-kernel/`)** keeps fast unit tests that exercise local modules (plan scheduler, effect manager, capability resolver, reducer receipt conversion). These live next to the code they cover (`mod tests` or sibling `tests.rs` files) so refactors catch regressions quickly.
- **Testkit crate (`crates/aos-testkit/`)** now ships the reusable deterministic harness (world builder, fake adapters, manifest fixtures) plus higher-level scenarios under `crates/aos-testkit/tests/`. All kernel integration flows moved here as part of WP1 so downstream crates (CLI, governance) can reuse the same helpers.

### WP1 – Kernel Integration Harness & Scenario Coverage *(Completed)*
- **Goal**: Move from bespoke tests in `crates/aos-kernel/src/world.rs` to reusable builders that make it trivial to describe reducer/plan manifolds and scripted event flows.
- **Key Tasks**:
  - Extract helper constructors for reducers, plans, schemas, capabilities, and manifests (fixtures should allow overriding cap slots/bindings, trigger routing, and plan DAG definitions).
  - Relocate/port the existing happy-path, guard, and receipt-routing tests into `crates/aos-testkit/tests/` (driven by the new builders) while leaving lightweight unit coverage in the kernel crate.
  - Add scenario coverage for at least: single-plan orchestration, reducer-driven retries (timer receipt), and guarded multi-branch plans as outlined in `spec/07-workflow-patterns.md`.
  - Ensure tests assert on journal ordering (domain events → triggers → plan wakeups) and deterministic replay by re-running the scenario through the harness without redefinition.
- **Delivered**: `aos-testkit` fixture module, `TestWorld` harness, helpers for event-emitting reducers, and seven integration cases covering single-plan orchestration, reducer timer receipts, guarded branches, multi-stage plans, plan event wakeups, and plan-raised events. Kernel unit tests retain only local coverage.

### WP2 – Reducer Micro-Effect Receipts & Built-in Schemas *(Completed)*
- **Goal**: Wire the built-in schema catalog (`crates/aos-air-types/src/builtins.rs`) into the kernel so reducer-emitted micro-effects automatically produce typed `sys/*` events when receipts arrive.
- **Key Tasks**:
  - Load and cache `spec/defs/builtin-schemas.air.json` hashes inside the kernel’s router so manifests can reference `sys/TimerFired@1`, `sys/BlobPutResult@1`, etc. without bespoke plumbing.
  - Extend the `pending_reducer_receipts` path to record `adapter_id`, serialized params, and reducer name so the emitted event payload matches the structure defined in `spec/03-air.md §7`.
  - Implement conversion helpers that take `EffectReceipt` + pending context → typed `DomainEvent` bytes; cover timer and blob receipts, including error/timeout statuses.
  - Add integration tests (leveraging WP1 harness) that stub timer/blob adapters, enqueue micro-effects, deliver receipts (including duplicates), and assert reducers observe the canonical `sys/*` payloads.
- **Delivered**: Manifest loader now injects built-in defschemas, reducer receipt conversion covers timer + blob put/get payloads, and aos-testkit integration tests assert sys/TimerFired and sys/Blob* events reach reducers (including duplicate suppression/unknown receipt handling).

### WP3 – Capability & Policy Enforcement *(Completed – minimal scope)*
- **Goal**: Introduce manifest-defined policy rules and keep capability enforcement honest without over-designing budgets yet.
- **Delivered**:
  - Manifest loader now hydrates `defpolicy` nodes; kernels instantiate either `RulePolicy` (default deny, reducer/plan aware) or fallback `AllowAllPolicy` as needed.
  - `EffectManager` evaluates policy decisions before queueing intents; reducers/plans missing bindings still get descriptive `KernelError`s.
  - Receipt conversion covers timer + blob events; integration tests in `policy_integration.rs` show reducers being denied while plans are allowed under the same policy.
  - Budgets/host-method constraints remain TODO pending broader design, but the scaffolding (RulePolicy + CapabilityResolver) makes that incremental.

### WP4 – Journal & Outbox Persistence *(Completed)*
- **Goal**: Implement the append-only journal and effect/receipt outbox so kernel state can be snapshotted and replayed deterministically (“replay-or-die”).
- **Key Tasks**:
  - Define on-disk layout for journal segments, CAS blobs, and snapshots as described in `spec/02-architecture.md` (canonical CBOR entries, monotonic segment numbers, fenced receipts).
  - Implement serialization for events/effect intents/receipts, plus CAS-backed storage for large payloads (params, receipt bodies). Ensure `EffectIntent.intent_hash` acts as the idempotency key.
  - Build a replay loader that rehydrates manifests, reducer states, queued plans, and pending receipts; verify idempotent processing of duplicate/late receipts using the logical height fence.
  - Extend WP1 scenarios to run twice: first to generate journal + snapshot artifacts, then to replay from genesis and assert byte-identical reducer states and queues.
- **Deliverables**: Journal writer/reader modules, snapshot serializer, replay harness with automated tests proving deterministic replays and proper handling of truncated/corrupt segments.
- **Delivered**: FsJournal-backed persistence, snapshot serialization via CAS blobs, replay loader with plan/reducer queue recovery, and integration tests covering journal-only rehydration, snapshot resume (including FsStore/FsJournal cold starts), and intent semantics.
- **Notes**: Hardening (stress/fuzz) will be revisited later, but the core WP4 infrastructure is in place.

### WP5 – Governance Loop Runtime
- **Goal**: Implement the manifest governance pipeline (propose → shadow-run → approve → apply) inside the kernel so control-plane changes can be exercised and tested deterministically. CLI work will follow later once the runtime pieces are in place.
- **Key Tasks**:
  - Add governance state tracking (proposals, lifecycle states, metadata) plus journal events for Proposed, ShadowRunCompleted, ApprovalRecorded, and ManifestApplied.
  - Implement the shadow executor: clone current world state from the latest snapshot, drive representative test events through the proposed manifest delta via the deterministic harness with stubbed adapters, and record predicted intent/effect summaries (stop at the first await/receipt boundary rather than replaying the full journal).
  - Wire approval/application logic so approved proposals atomically swap the active manifest (routing, cap grants, policies) and emit auditable journal entries.
  - Provide a kernel API to submit manifest patches (new nodes or replacements) that reuses the existing AIR loader/validator before entering the governance pipeline.
  - Add an end-to-end integration test (in `aos-testkit`) that submits a simple manifest delta, runs propose → shadow → approve → apply, and asserts the new reducer/plan is live afterward.
- **Deliverables**: Kernel governance structures + journal events, shadow-run executor, manifest patch ingestion, and an integration test proving the loop works without any CLI.

## Dependencies & Suggested Order
1. **WP1** lays the groundwork for deterministic integration tests used by all later packages.
2. **WP2** relies on the harness from WP1 and unlocks structured reducer receipt handling needed for budget settlement in WP3.
3. **WP3** depends on WP2 events (for cost data) and provides enforcement guarantees required before persisting intent/receipt history in WP4.
4. **WP4** supplies durable replay infrastructure that WP5’s shadow runs and governance proofs depend on.
5. **WP5** requires deterministic runtime + persistence to safely simulate and apply control-plane changes.

Keep this document updated as each package lands or is reprioritized.

## Kernel Test Gaps To Close
- Reducer receipt error paths (decode failures, adapter error statuses) still lack coverage; extend WP2 harness to exercise those failure cases in kernel unit tests.
- Capability + policy enforcement currently lacks unit tests for duplicate grants, slot mismatches, or policy deny cases; these must be added as part of WP3.
- Effect manager budgeting, intent hashing, and audit logging are untested; motivators for WP3/WP4 work.
- Journal/replay invariants (idempotent receipt handling, ordering fences) have zero coverage; WP4 should add integration tests via `aos-testkit` harness.
- Governance loop behaviors (proposal/shadow/approval/apply) and CLI flows have no tests; WP5 must add both unit coverage in kernel components and end-to-end runs that reuse the shared testkit fixtures.
