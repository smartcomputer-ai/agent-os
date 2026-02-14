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
3. Canonical reusable AIR assets for the SDK live under `crates/aos-agent-sdk/air/` (schemas, module defs, plan templates, capability/policy templates), but only for `aos.agent/*` ownership.
4. `apps/demiurge` is a consumer and migration target for SDK contracts, not the source of truth for them.
5. `crates/aos-smoke` is the single end-to-end runner for SDK flows; do not introduce a parallel e2e harness in `aos-agent-sdk`.
6. E2E execution has two lanes:
   - deterministic lane (default/CI, mock or stub adapters, replay parity required),
   - live lane (opt-in with real credentials/providers, validates wiring/interop, not replay-parity gating).
7. Built-in `sys/*` ownership stays in core:
   - schemas/effects/caps in `spec/defs`,
   - Rust effect/receipt types in `aos-effects`,
   - adapter execution in `aos-host`,
   - cap enforcers in `aos-sys`.

## Why Staged

The SDK has coupled contracts (session lifecycle, LLM contract shape, loop safety, event API, failure semantics).
Staging prevents partial rollout drift and gives each phase a testable boundary.

## Stage Plan

1. `roadmap/v0.10-agent-sdk/p2.1-session-contracts.md`
   Foundation schemas and lifecycle/control contracts.
2. `roadmap/v0.10-agent-sdk/p2.2-llm-contract-direct-provider-model.md`
   Direct provider/model run config with LLM effect contract evolution (no profile registry).
3. `roadmap/v0.10-agent-sdk/p2.3-tool-loop-safety-context-bounds.md`
   Loop safety, limits, bounded tool output, context pressure signals.
4. `roadmap/v0.10-agent-sdk/p2.4-events-observability-contract.md`
   Canonical event API, ordering/correlation guarantees, stream contract.
5. `roadmap/v0.10-agent-sdk/p2.5-failure-retry-cancel.md`
   Failure taxonomy, retry ownership, cancellation semantics.
6. `roadmap/v0.10-agent-sdk/p2.6-sdk-conformance-live-smoke.md`
   SDK conformance matrix and opt-in live-provider smoke coverage.

## Sequencing Rationale

1. Contracts first (`p2.1`), so later work targets stable schemas/events.
2. LLM contract second (`p2.2`), split into:
   - core `sys/Llm*` evolution in core crates,
   - SDK mapper flow on top of that core contract.
3. Loop safety + bounding third (`p2.3`), once request/response surfaces are fixed.
4. Event API fourth (`p2.4`), once core runtime flow is stable.
5. Failure/retry/cancel fifth (`p2.5`), to harden operational behavior.
6. Conformance closure last (`p2.6`), with Demiurge migration deferred to `p3`.

## Completion Rule

P2 is complete only when all stage exit criteria are met and validated by deterministic integration tests and replay parity checks.
