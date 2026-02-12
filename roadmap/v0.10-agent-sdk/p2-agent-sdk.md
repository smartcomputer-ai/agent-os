# P2: Agent SDK on AOS Primitives

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (agents remain app-specific and hard to scale)  
**Status**: Proposed

## Goal

Define and implement an Agent SDK that sits above core AOS and makes agent construction reusable:
- coding agent,
- Demiurge-native world operator,
- planner/worker/judge patterns for factory flows.

The SDK should be headless-first, policy-aware, and fully auditable through normal AOS event/plan/receipt rails.

**Very Important**: WE CAN MAKE BREAKING CHANGES. DO NOT, I REPEAT, DO NOT WORRY ABOUT BACKWARD COMPATIBILITY. Only focus on what the best new setup and contracts and schemas would be and agressively refactor towards that goal.

## Core Positioning

AOS itself remains agent-agnostic.  
Agent semantics are introduced in SDK-level schemas, reducers, plans, and helper libraries.

This preserves existing system boundaries:
- reducers: state + business/agent loop logic,
- plans: external orchestration and effect execution,
- adapters: provider/tool execution.

## Non-Goals (P2)

- No kernel-level “agent” primitive.
- No implicit hidden orchestration loops outside events/plans.
- No final multi-world universe protocol design in this phase.

## Decision Summary

1. Create `crates/aos-agent-sdk` (plus app-level AIR templates/examples).
2. Standardize agent run primitives as schemas/events rather than ad hoc app types.
3. Keep tool execution cap/policy gated through normal plan effects.
4. Support parallel tool execution via plan DAG fan-out/fan-in and/or multi-plan choreography.
5. Accept breaking schema changes during v0.10 to converge on a stable core model.

## Proposed SDK Primitives (v0)

### State model
- `AgentRun`: identity, objective, policy profile, budget, status.
- `AgentTurn`: input refs, model config, parent/child relation.
- `AgentAction`: requested tool call(s), delegation, plan request.
- `AgentObservation`: tool results/receipts/outcomes.
- `AgentDecision`: continue, retry, delegate, finish, fail.

### Event model
- `RunRequested`, `RunStarted`, `TurnRequested`, `TurnCompleted`
- `ActionRequested`, `ActionCompleted`, `ActionFailed`
- `DelegationRequested`, `DelegationResult`
- `RunCompleted`, `RunFailed`, `RunCancelled`

### Tooling model
- typed tool registry entries (schema + constraints + caps),
- deterministic tool result envelope,
- explicit error category taxonomy (policy/cap/adapter/timeout/validation).

## Architecture Shape

### Crate/API

`crates/aos-agent-sdk` should provide:
- reusable event/state types,
- reducer helpers (turn loop, correlation, idempotency fences),
- plan helper patterns (tool execution, wait handling, retries),
- test harness helpers for deterministic replay-based assertions.

### AIR assets

Provide reusable templates under app folders (or examples):
- minimal headless agent world,
- tool-enabled single-agent world,
- delegation/subagent pattern world.

## Phase Plan

### Phase 2.1: SDK schema and contracts
- Define base schemas for run/turn/action/observation/decision.
- Define compatibility rules and versioning policy.
- Add initial docs and examples.

### Phase 2.2: Reducer/plan helpers
- Add reducer utility patterns for agent loops.
- Add plan utility patterns for tool fan-out/fan-in.
- Provide standard error/result envelopes.

### Phase 2.3: Tool runtime contracts
- Define SDK-level tool descriptor conventions.
- Standardize tool call normalization and result events.
- Add harness tests across mock tools.

### Phase 2.4: Headless operations
- Add operational helpers for long-running headless runs:
  - checkpoint markers,
  - cancellation,
  - bounded retries/backoff policies.
- Integrate with debug trace surfaces from v0.9.

## Testing Strategy

- Unit tests for schema transforms and loop helpers.
- Integration tests that run full reducer+plan flows using `aos-host` test fixtures.
- Replay-or-die tests asserting byte-identical snapshots across agent runs.
- Targeted conformance tests for tool-call roundtrips and parallel action handling.

## Definition of Done

- `aos-agent-sdk` exists with reusable primitives and helper APIs.
- At least one non-trivial agent flow runs end-to-end using SDK primitives.
- Parallel tool-use and error handling paths are covered by deterministic integration tests.
- Docs are sufficient for building a new agent app without copying Demiurge internals.

## Open Questions

- Which primitives should be mandatory vs optional extension points?
- Do we standardize delegation now for same-world only, or reserve cross-world semantics for universe work?
- Where do budget/economics constraints live first: SDK schemas only, or immediately cap-policy integrated?
