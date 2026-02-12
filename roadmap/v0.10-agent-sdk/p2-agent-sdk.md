# P2: Agent SDK on AOS Primitives (Contract-Complete)

**Priority**: P2  
**Effort**: High  
**Risk if deferred**: High (agents remain app-specific and fragile under real workloads)  
**Status**: Proposed (revised)

## Goal

Define and implement an Agent SDK above core AOS that makes agent behavior reusable and operationally safe across:
- coding agents,
- Demiurge-native world operators,
- planner/worker/judge and factory patterns.

The SDK must stay inside AOS boundaries:
- reducers own state and business/loop decisions,
- plans execute privileged effects,
- adapters isolate non-determinism behind receipts.

This revision resolves the concerns in `p2-agent-sdk-concerns.md` with concrete contracts.

**Very Important**: v0.10 allows breaking changes. Optimize for the best long-term contracts, not compatibility.

## What Demiurge Already Proves (and Where It Stops)

Current Demiurge in `apps/demiurge/` already validates important patterns:

1. CAS-first IO works:
   - reducer and plans exchange `message_ref`, `output_ref`, `result_ref` hashes.
2. Reducer/plan split is correct:
   - reducer interprets tool calls and emits intents (`ToolCallRequested`),
   - plans execute `llm.generate`, `workspace.*`, `introspect.*`.
3. Tool registry refresh is version-aware:
   - `tool_registry_plan` avoids rescanning when version is unchanged.
4. Normalized LLM output already exists in host:
   - `aos-host` LLM adapter stores normalized output in `output_ref` and provider-native raw output in `raw_output_ref`.

But it is still chat-app specific and under-specified for SDK reuse:

- no explicit host control/session lifecycle contract,
- no canonical terminal states on reducer state,
- no SDK-level event schema guarantees for automation,
- no shared failure/retry ownership contract,
- output truncation exists but is app-local (`MAX_TOOL_OUTPUT_BYTES = 64 * 1024` in reducer) instead of SDK policy.

## P2 Thesis

P2 must standardize runtime behavior, not only naming:

1. Session control contract.
2. Provider profile contract.
3. Context/output bounding contract.
4. Loop safety and stop semantics contract.
5. Event API contract (journal + telemetry).
6. Failure/recovery taxonomy contract.
7. LLM effect contract evolution.
8. Determinism boundary contract.
9. Conformance-based Definition of Done.

Without these, each app rebuilds the same loop differently.

## SDK Namespace and Core Defs

- Namespace: `aos.agent/*`.
- Keyed reducer baseline:
  - module: `aos.agent/SessionReducer@1`
  - key schema: `aos.agent/SessionId@1`
  - event schema: `aos.agent/SessionEvent@1`
  - state schema: `aos.agent/SessionState@1`
- Plans:
  - `aos.agent/llm_step_plan@1`
  - `aos.agent/tool_call_plan@1`
  - `aos.agent/toolset_refresh_plan@1`
- Optional (P2 if time, else P3): `aos.agent/session_control_plan@1` for host commands that require plan effects.

## Concern Resolutions

## 1) Host-Control Contract (resolved)

### Contract

Add explicit lifecycle and host command events:

- `aos.agent/SessionLifecycle@1` (variant):
  - `Idle`, `Running`, `WaitingInput`, `Paused`, `Cancelling`, `Completed`, `Failed`, `Cancelled`.
- `aos.agent/HostCommand@1` (variant):
  - `Steer { text }`
  - `FollowUp { text }`
  - `Reconfigure { patch }`
  - `Pause`
  - `Resume`
  - `Cancel { reason? }`

### Observation semantics

- `Steer`: observed at next step boundary before the next `llm.generate`.
- `FollowUp`: queued and consumed after current input/run reaches `Idle` or `WaitingInput`.
- `Reconfigure`: takes effect on next `LlmStepRequested`.
- `Cancel`: immediate state transition to `Cancelling`; no new intents emitted. Late receipts/results are ignored via epoch fence.

### Implementation shape

- Add `session_epoch` and `step_epoch` to state and emitted intents.
- Plans echo epochs in result events.
- Reducer ignores stale completions (`epoch mismatch`).

This avoids kernel plan-cancel primitives while remaining deterministic and compatible with AIR v1.

## 2) Provider Strategy (resolved)

### Contract

Introduce first-class provider profiles:

- `aos.agent/ProviderProfile@1`:
  - `profile_id`
  - `provider`
  - `model_default`
  - capability flags:
    - `supports_parallel_tool_calls`
    - `supports_reasoning_effort`
    - `supports_streaming`
    - `supports_tool_choice_named`
  - hints:
    - `context_window_hint`
  - `provider_options_default` (escape hatch)

### Rules

- Reducer and plans are profile-driven; app logic never branches on raw provider names.
- Preserve provider-native tool/request semantics in adapter/profile layer.
- Normalize outputs into common CAS envelopes for reducer consumption.

### Current-code alignment

- `aos-llm` already supports provider-specific options and normalized stream/event models.
- `aos-host` currently drops `provider_options`; P2 must thread profile options through `llm.generate`.

## 3) Context Bounding and Output Discipline (resolved)

### Contract

Define SDK-wide tool output policy and channels:

- `operator_output_ref`: full-fidelity tool output in CAS.
- `model_output_ref`: bounded text passed to LLM context.
- `truncation_meta`: `{ original_bytes, bounded_bytes, truncated, policy_id }`.

### Deterministic bounding policy (default)

1. Apply byte cap per tool family.
2. Decode into UTF-8 (lossy if needed).
3. If over limit, head/tail compaction with deterministic marker:
   - `...[truncated <N> bytes; sha256:<digest>]`
4. Store both refs.

### Pressure signals

Reducer emits `aos.agent/ContextPressure@1` when thresholds are crossed (for example 70/85/95% estimated window usage).
Compaction strategy remains host/app-specific, but pressure signals are standardized now.

## 4) Loop Safety and Termination (resolved)

### Contract

Canonical stop reasons:

- `Completed`
- `Cancelled`
- `Failed { code, retryable, stage }`
- `LimitsExceeded { kind }` where kind in:
  - `max_turns`
  - `max_tool_rounds`
  - `max_steps`
  - `max_tool_calls_per_step`

Loop detection contract:

- signature: hash of `(tool_name, normalized_args, tool_choice, last_assistant_ref?)`
- configurable window and repeat threshold
- policy:
  - `InjectSteeringThenContinue` (default once),
  - `FailImmediately`,
  - `CompleteWithWarning`

### Cancellation ordering

Ordered journal events:

1. `HostCommandAccepted(Cancel)`
2. `LifecycleChanged(Cancelling)`
3. optional late receipts/results (ignored by reducer via epoch fence)
4. `LifecycleChanged(Cancelled)`

## 5) Event Contract as API (resolved)

### Contract

Create canonical SDK event schema:

- `aos.agent/AgentEvent@1` (variant) with required correlation fields:
  - `session_id`
  - `run_id`
  - `turn_id`
  - `step_id`
  - `event_seq`
  - `correlation_id`
  - `causation` (event hash or intent hash)

Event families:

- lifecycle: started/paused/resumed/completed/failed/cancelled
- llm step: requested/completed/failed
- tool call: requested/completed/failed/bounded
- host control: accepted/rejected/applied
- diagnostics: loop detected/context pressure

### Ordering guarantees

- Durable ordering is journal order (DomainEvent/receipt order from kernel).
- Telemetry stream ordering is best-effort and must carry journal cursor when derived from journal.

### Channels

- `model channel`: bounded payload refs only.
- `operator channel`: full refs and diagnostics.

## 6) Failure Model and Recovery Taxonomy (resolved)

### Contract

Standard error kinds:

- `policy_denied`
- `cap_denied`
- `validation_error`
- `adapter_error`
- `adapter_timeout`
- `provider_error_retryable`
- `provider_error_terminal`
- `tool_not_found`
- `tool_args_invalid`
- `internal_invariant_violation`

### Retry ownership

- Adapter-owned retries: transient network/provider transport (`aos-llm` retry policy).
- Plan-owned retries: effect-level retry envelope only when deterministic and explicit.
- Reducer-owned retries: business retries/backoff decisions and escalation.

Terminal reducer states are always one of:
- `Completed`
- `Failed`
- `Cancelled`

No silent terminal ambiguity.

## 7) LLM Effect Contract Pressure (resolved)

### Contract change

Add `sys/LlmGenerateParams@2` and `sys/LlmGenerateReceipt@2` for SDK needs:

- params additions (optional):
  - `reasoning_effort`
  - `stop_sequences`
  - `metadata`
  - `provider_options`
  - `response_format`
- receipt additions:
  - `finish_reason`
  - `usage_details` (reasoning/cache tokens when available)
  - `warnings_ref` (optional CAS ref)

Keep:
- `output_ref` = normalized reducer-facing payload,
- `raw_output_ref` = provider-native payload for audit/debug.

This matches current host behavior and closes the schema gap for runtime controls.

## 8) Determinism Boundary for Agent Workloads (resolved)

### Contract

Replay-relevant:
- DomainEvents,
- EffectIntents/EffectReceipts,
- PlanResult,
- canonical SDK envelopes and bounded refs used for decisions.

Telemetry-only (never reducer-decision input):
- live stream deltas,
- UI progress ticks,
- non-journal host diagnostics.

### Rule

Any value that can affect reducer transitions must be journaled and schema-normalized once at ingress.

### Tests

Add replay-or-die e2e tests for agent flows:
- run flow,
- snapshot state/journal,
- replay from genesis,
- assert byte-identical state and terminal classification.

## 9) Definition of Done for Cross-App Reuse (resolved)

P2 closes only when behavior is conformant, not just APIs compiling.

Required conformance matrix:

- provider profiles: `openai-responses`, `anthropic-messages`, `openai-compatible`.
- scenarios:
  - no-tool completion,
  - single tool call,
  - multi-tool roundtrip,
  - bounded output with truncation markers,
  - host cancellation,
  - loop detection trigger,
  - policy deny,
  - cap deny,
  - adapter timeout/error,
  - parent/child session orchestration.

Minimum artifacts before closure:

1. `crates/aos-agent-sdk` with contracts/helpers/tests.
2. One headless sample world using `aos.agent/*`.
3. One parent/child session sample (`spawn/send_input/wait/close` event choreography).
4. Demiurge migrated to SDK core contracts (legacy adapters allowed short-term).

## Concrete AIR Mapping (Demiurge -> SDK)

- `demiurge/UserMessage@1` -> `aos.agent/UserInput@1`
- `demiurge/ChatRequest@1` -> `aos.agent/LlmStepRequested@1`
- `demiurge/ChatResult@1` -> `aos.agent/LlmStepCompleted@1`
- `demiurge/ToolCallRequested@1` -> `aos.agent/ToolCallRequested@1`
- `demiurge/ToolResult@1` -> `aos.agent/ToolResult@1`
- `demiurge/ToolRegistryScanRequested@1` -> `aos.agent/ToolsetRefreshRequested@1`

and plans:

- `demiurge/chat_plan@1` -> `aos.agent/llm_step_plan@1`
- `demiurge/tool_plan@1` -> `aos.agent/tool_call_plan@1`
- `demiurge/tool_registry_plan@1` -> `aos.agent/toolset_refresh_plan@1`

## Implementation Slices

### Phase 2.1: Contracts and schemas
- Define `SessionState`, `SessionEvent`, `HostCommand`, `ProviderProfile`, `FailureEnvelope`, `ContextPressure`.
- Define event ordering/correlation rules.

### Phase 2.2: Reducer helpers
- session lifecycle state machine helpers,
- epoch fence helpers,
- loop detection and limit evaluators.

### Phase 2.3: Plan templates
- llm step plan template,
- tool call plan template (including bounded output refs),
- toolset refresh template.

### Phase 2.4: LLM effect v2
- add `sys/LlmGenerateParams@2`/`Receipt@2`,
- host adapter wiring for `provider_options`, `reasoning_effort`, `finish_reason`.

### Phase 2.5: Event API and telemetry
- finalize `aos.agent/AgentEvent@1`,
- align HTTP/SSE surfaces with cursor/correlation semantics.

### Phase 2.6: Conformance harness
- profile x scenario matrix tests,
- replay parity for every terminal class.

### Phase 2.7: Demiurge migration path
- keep namespace adapters temporarily,
- move reducer state/lifecycle to SDK contracts,
- remove legacy-only paths by end of v0.10.

## Specs Alignment

This proposal intentionally aligns with:

- `spec/03-air.md`:
  - plans as `emit_effect`/`await_receipt`/`raise_event` orchestration,
  - policy/cap gating at effect boundary,
  - determinism and replay constraints.
- `spec/04-reducers.md`:
  - reducer-owned business logic and typestate,
  - micro-effect limits for reducers,
  - intent-driven pattern for risky effects.
- `spec/02-architecture.md`:
  - receipts as non-determinism boundary,
  - journaled control-plane/runtime evidence.

## Out of Scope for P2

- kernel-level agent primitive,
- new plan opcodes,
- universe/cross-world protocol,
- automatic compaction policy selection (only pressure signaling is standardized in P2).

## Open Questions (remaining)

1. Should `pause/resume` be mandatory in P2 core or optional extension?
2. Should `provider_options` be typed per profile in SDK schemas now, or remain partially opaque until v0.11?
3. How strict should initial loop-detection defaults be for coding-agent vs world-operator profiles?
