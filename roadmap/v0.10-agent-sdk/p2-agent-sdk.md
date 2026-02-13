# P2: Agent SDK on AOS Primitives (Index + Staged Delivery)

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (agents remain app-specific and fragile under real workloads)  
**Status**: Proposed (staged)

## Goal

Define and implement a reusable Agent SDK above core AOS that is:
- headless-first,
- policy/capability-aware,
- deterministic where required,
- auditable across reducer/plan/effect boundaries.

This document is the umbrella index. Implementation is split into staged docs (`p2.1` ... `p2.6`) with hard exit gates.

## Repository Placement and Test Runner Decisions

1. SDK contracts and reusable runtime helpers live in `crates/aos-agent-sdk`.
2. For v0.10, SDK reducer/pure WASM modules stay in `crates/aos-agent-sdk` (`src/bin/*`) rather than a separate module crate.
3. Canonical reusable AIR assets for the SDK live under `crates/aos-agent-sdk/air/` (schemas, module defs, plan templates, capability/policy templates).
4. `apps/demiurge` is a consumer and migration target for SDK contracts, not the source of truth for them.
5. `crates/aos-smoke` is the single end-to-end runner for SDK flows; do not introduce a parallel e2e harness in `aos-agent-sdk`.
6. E2E execution has two lanes:
   - deterministic lane (default/CI, mock or stub adapters, replay parity required),
   - live lane (opt-in with real credentials/providers, validates wiring/interop, not replay-parity gating).

## Why Staged

The SDK has coupled contracts (session lifecycle, provider profiles, loop safety, event API, failure semantics).  
Staging prevents partial rollout drift and gives each phase a testable boundary.

## Stage Plan

1. `roadmap/v0.10-agent-sdk/p2.1-session-contracts.md`  
   Foundation schemas and lifecycle/control contracts.
2. `roadmap/v0.10-agent-sdk/p2.2-provider-profiles-llm-contract.md`  
   Provider profile model, dedicated profile-catalog reducer flow, and LLM effect contract evolution.
3. `roadmap/v0.10-agent-sdk/p2.3-tool-loop-safety-context-bounds.md`  
   Loop safety, limits, bounded tool output, context pressure signals.
4. `roadmap/v0.10-agent-sdk/p2.4-events-observability-contract.md`  
   Canonical event API, ordering/correlation guarantees, stream contract.
5. `roadmap/v0.10-agent-sdk/p2.5-failure-retry-cancel.md`  
   Failure taxonomy, retry ownership, cancellation semantics.
6. `roadmap/v0.10-agent-sdk/p2.6-conformance-and-demiurge-migration.md`  
   Conformance matrix and Demiurge migration onto `aos.agent/*`.

## Sequencing Rationale

1. Contracts first (`p2.1`), so all later work targets stable schemas/events.
2. Provider + LLM contract second (`p2.2`), because loop behavior depends on request/receipt semantics.
3. Loop safety + bounding third (`p2.3`), once request/response surfaces are fixed.
4. Event API fourth (`p2.4`), once core runtime flow is stable.
5. Failure/retry/cancel fifth (`p2.5`), to harden operational behavior.
6. Conformance + migration last (`p2.6`), to prove reuse in real app wiring.

## Completion Rule

P2 is complete only when all stage exit criteria are met and validated by deterministic integration tests and replay parity checks.
