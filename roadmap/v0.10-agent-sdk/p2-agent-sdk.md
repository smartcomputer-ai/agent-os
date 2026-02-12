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

## Namespace Convention

- SDK-owned schemas, plans, and modules use `aos.agent/*`.
- Do not use `sys/*` for SDK-level agent abstractions.
- App-specific layers (like Demiurge UI events) may map onto `aos.agent/*` but should not redefine core loop semantics.

## Demiurge Learnings to Preserve

These are required constraints for SDK design because they reflect hard-won behavior from the current Demiurge wiring:

1. CAS-first IO:
   - message/tool/result payloads are hash refs, not large inline state blobs.
2. Reducer interprets, plans execute:
   - reducer parses tool calls and emits typed tool intents,
   - plans perform `llm.generate` and tool effects.
3. Plan-only privileged effects:
   - `llm.generate`, `workspace.*`, `introspect.*` stay in plans under policy/cap gates.
4. Tool call normalization before execution:
   - reducer maps tool call payloads into typed params/variants before `ToolCallRequested`.
5. Tool registry caching pattern:
   - refresh/scan is version-aware and should avoid unnecessary re-resolution each turn.

## Reference Wiring to Carry Forward

SDK should explicitly preserve this shape (renamed into `aos.agent/*`):

- `demiurge/UserMessage@1` -> `aos.agent/UserInput@1`
- `demiurge/ChatRequest@1` -> `aos.agent/LlmStepRequested@1`
- `demiurge/ChatResult@1` -> `aos.agent/LlmStepCompleted@1`
- `demiurge/ToolCallRequested@1` -> `aos.agent/ToolCallRequested@1`
- `demiurge/ToolResult@1` -> `aos.agent/ToolResult@1`
- `demiurge/ToolRegistryScanRequested@1` -> `aos.agent/ToolsetRefreshRequested@1`

Plan equivalents:
- `demiurge/chat_plan@1` -> `aos.agent/llm_step_plan@1`
- `demiurge/tool_plan@1` -> `aos.agent/tool_call_plan@1`
- `demiurge/tool_registry_plan@1` -> `aos.agent/toolset_refresh_plan@1`

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

Concrete naming baseline:
- `aos.agent/UserInput@1`
- `aos.agent/LlmStepRequested@1`
- `aos.agent/LlmStepCompleted@1`
- `aos.agent/ToolCallRequested@1`
- `aos.agent/ToolResult@1`
- `aos.agent/Completed@1`

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

Baseline module/plan names:
- keyed reducer: `aos.agent/SessionReducer@1`
- plans: `aos.agent/llm_step_plan@1`, `aos.agent/tool_call_plan@1`, `aos.agent/toolset_refresh_plan@1`

## Phase Plan

### Phase 2.1: SDK schema and contracts
- Define base schemas for run/turn/action/observation/decision.
- Define keyed routing schema (`session_id`) and enforce `key_field` conventions.
- Define compatibility rules and versioning policy.
- Add initial docs and examples.

### Phase 2.2: Reducer/plan helpers
- Add reducer utility patterns for agent loops.
- Add plan utility patterns for tool fan-out/fan-in.
- Provide standard error/result envelopes.
- Encode current Demiurge loop shape as first-class helpers, not as optional examples.

### Phase 2.3: Tool runtime contracts
- Define SDK-level tool descriptor conventions.
- Standardize tool call normalization and result events.
- Add harness tests across mock tools.
- Include a policy template that mirrors current Demiurge allow/deny pattern by origin plan/module.

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
- The default SDK flow can be wired into current Demiurge plans/events with only namespace/event-shape adapters.

## Open Questions

- Which primitives should be mandatory vs optional extension points?
- Do we standardize delegation now for same-world only, or reserve cross-world semantics for universe work?
- Where do budget/economics constraints live first: SDK schemas only, or immediately cap-policy integrated?
