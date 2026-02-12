# P2 Addendum: Agent SDK Concerns and Vision

**Priority**: P2  
**Status**: Proposed

## Why This Document Exists

`p2-agent-sdk.md` defines the architectural direction and AIR wiring shape. This addendum defines the critical product/runtime concerns that must be explicit in P2 so the SDK is not only well-structured, but also usable, predictable, and scalable across agent applications.

The goal is a standalone vision for what makes an Agent SDK reliable in practice, while preserving AOS boundaries (reducers interpret state/logic, plans execute effects, adapters perform external calls).

## Core Thesis

If the SDK stops at naming/event normalization and helper crates, every app will still rebuild the hard parts differently: session control, tool-loop safety, provider behavior, context bounding, failure semantics, and observability contracts.

P2 should converge these concerns into first-class contracts so new agent apps compose from stable primitives rather than custom loop implementations.

## Main Concerns

## 1) Host-Control Contract Is Underspecified

### Concern
The SDK currently describes reusable primitives, but not a precise host-facing control model for live runs.

### Why it matters
Without a clear host control contract, each app invents incompatible behavior for:
- steering an in-flight run,
- queuing follow-up inputs,
- changing model/runtime knobs between steps,
- deciding when a run is complete versus waiting for more input.

This fragmentation directly hurts reuse and makes headless operations brittle.

### Required outcomes
- Define explicit session lifecycle states and transitions.
- Define mid-run control surfaces (`steer`, `follow_up`, `cancel`, optional `pause/resume`).
- Define exactly when control inputs are observed (immediate, next step boundary, or next turn).
- Define whether and how mutable config (reasoning effort, limits, model overrides) can change mid-session.

## 2) Provider Strategy Needs Explicit Philosophy

### Concern
P2 names tool contracts but does not yet lock the provider strategy at the SDK level.

### Why it matters
Model quality is strongly coupled to tool shape and instruction shape. If SDK contracts force an overly uniform surface, quality regresses and adapters become distortion layers.

### Required outcomes
- Establish provider profiles as first-class SDK assets.
- Preserve provider-native tool semantics where needed, while still mapping results into common SDK envelopes.
- Define a profile capability surface (parallel tool calls, reasoning controls, streaming support, context size hints).
- Define a compatibility policy for adding/changing profiles without destabilizing app-level agent logic.

## 3) Context Bounding and Output Discipline Are Not Yet a Contract

### Concern
P2 identifies CAS-first IO and deterministic envelopes, but does not yet define strict runtime policies for large outputs and context pressure.

### Why it matters
Unbounded tool output is the fastest path to runaway cost, degraded model behavior, and opaque failures. Without common truncation and context signals, each app will rediscover the same failure modes.

### Required outcomes
- Standardize tool output bounding policy (size caps, ordering of truncation steps, visible truncation markers).
- Standardize what is sent to model context versus what is retained for operators/audit.
- Add context pressure telemetry (for example threshold events) so hosts can decide summarization/compaction strategy.
- Keep compaction policy out of core P2 if needed, but make pressure signals and extension points explicit now.

## 4) Loop Safety and Termination Semantics Need Hard Guarantees

### Concern
P2 loop shape is correct at a high level, but stop/abort/loop-protection behavior is not fully specified.

### Why it matters
Agent loops fail in subtle ways: repeated ineffective tool calls, infinite recovery cycles, and ambiguous end states. If stop semantics are ambiguous, automation becomes unsafe.

### Required outcomes
- Define canonical stop conditions (natural completion, hard limits, cancellation, unrecoverable error).
- Define max-round/max-turn semantics and precedence.
- Define loop-detection behavior (signature model, thresholds, reaction policy).
- Define graceful cancellation semantics and event ordering.

## 5) Event Contract Must Be a First-Class API, Not Just Debug Data

### Concern
P2 requests better observability, but does not yet define event guarantees strongly enough for UIs/automation.

### Why it matters
For headless systems, the event stream is the integration surface. Missing or inconsistent events block reliable orchestration, incident diagnosis, and auditability.

### Required outcomes
- Define canonical event kinds and required fields.
- Define ordering guarantees and correlation rules across turns and tool calls.
- Define which payloads are full fidelity (operator channel) versus bounded (model channel).
- Define compatibility/versioning policy for event schema evolution.

## 6) Failure Model Needs a Shared Recovery Taxonomy

### Concern
P2 asks for error categories but does not yet define recovery semantics strongly enough.

### Why it matters
Without shared taxonomy and recovery rules, identical failures produce divergent behavior across apps (retry storms, silent drops, wrong terminal states).

### Required outcomes
- Separate tool-level recoverable failures from session-level terminal failures.
- Define retry ownership boundaries (LLM adapter retry vs plan retry vs reducer decision).
- Define canonical terminal states (`Completed`, `Failed`, `Cancelled`) and transition rules.
- Ensure failure envelopes include structured cause and correlation identifiers.

## 7) Effect Contract Pressure: LLM Parameters May Be Too Narrow

### Concern
P2 expects dynamic runtime control and profile behavior, but current built-in LLM generate contract appears minimal.

### Why it matters
If SDK semantics rely on knobs that effect schemas do not expose, implementations either fork semantics into app-specific blobs or bypass typed contracts.

### Required outcomes
- Evaluate whether `sys/LlmGenerateParams@1` should gain optional fields for:
  - reasoning control,
  - provider-specific options,
  - additional response shaping needed by agent loops.
- Keep normalized output in CAS as the reducer-facing contract, with provider-native output retained for audit/debug.
- Document which parts are stable SDK contract vs provider escape hatch.

## 8) Determinism Boundary Must Be Explicitly Documented for Agent Workloads

### Concern
P2 inherits AOS determinism principles but does not yet spell out deterministic vs non-deterministic surfaces for agent operations.

### Why it matters
Agent systems mix deterministic state transitions with non-deterministic external calls. If boundaries are unclear, replay expectations and debugging practices drift.

### Required outcomes
- Explicitly define what is replay-relevant (events, receipts, normalized envelopes) versus telemetry-only.
- Define how streaming/non-deterministic runtime signals are excluded from reducer state decisions.
- Add replay-oriented assertions for end-to-end agent flows, not only unit-level helpers.

## 9) Definition of Done Is Too High-Level for Cross-App Reuse

### Concern
Current DoD confirms existence and one working flow, but not behavioral consistency across providers and failure paths.

### Why it matters
An SDK that passes happy-path demos can still fail under real workload variance. Reuse requires behavioral conformance, not just compile-time integration.

### Required outcomes
- Add conformance matrix across provider profiles and key scenarios.
- Add deterministic integration tests for:
  - parallel tool fan-out/fan-in,
  - truncation and context-bound behavior,
  - cancellation and loop detection,
  - delegation/subagent lifecycle.
- Require at least one non-trivial smoke flow per profile before P2 closure.

## Recommended P2 Scope Tightening

To keep P2 focused while still solving the hard problems:

1. Prioritize contracts for host control, provider profiles, bounded context handling, events, and failure semantics.
2. Treat these contracts as mandatory SDK core, not optional helpers.
3. Gate P2 completion on behavioral conformance tests, not only API availability.

## Proposed Exit Criteria Additions for P2

P2 should not be marked complete until all are true:

- Session lifecycle and host-control APIs are specified and implemented.
- Provider profile model and capability flags are specified and implemented.
- Tool output bounding + context pressure signaling are implemented with deterministic tests.
- Event contract includes ordering/correlation guarantees and is validated by integration tests.
- Failure taxonomy and retry ownership are implemented and documented.
- Agent flow replay tests verify byte-identical outcomes where expected.
- At least one reusable sample world demonstrates parent/child session orchestration under these contracts.
