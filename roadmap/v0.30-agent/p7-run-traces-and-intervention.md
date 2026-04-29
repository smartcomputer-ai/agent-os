# P7: Run Traces and Operator Intervention

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (agent failures will remain hard to diagnose, and pause/steer/cancel behavior will stay ad hoc)  
**Status**: Core run trace model and recording complete; intervention replacement and host signal integration still pending  
**Depends on**: `roadmap/v0.30-agent/p5-session-run-model.md`, `roadmap/v0.30-agent/p6-context-engine.md`

## Goal

Add first-class run traces and deterministic intervention semantics.

Primary outcome:

1. every run has an inspectable trace of LLM turns, context plans, tool batches, effects, receipts, and outcomes,
2. operator steer/follow-up/interrupt/cancel behavior is explicit and replay-safe,
3. active runs can be diagnosed without reconstructing state from raw journal frames,
4. intervention semantics work for both local host sessions and Fabric-backed host sessions where supported,
5. Demiurge can report meaningful task progress and failure causes.

## Current Fit

The current code has early pieces but no coherent model:

1. `last_tool_plan_hash` stores only a narrow reference to the last plan.
2. `last_output_ref` is run-like but stored in session state.
3. `pending_steer` and `pending_follow_up` are queues with weak semantics.
4. pause/resume/cancel change `SessionLifecycle`, but there is no separate run lifecycle.
5. active host effects may continue running even if the agent marks itself cancelled.
6. stream frames are observed only as effect progress, not surfaced as a run trace.

P5 and P6 provide the right attachment points:

1. run state owns active execution,
2. context reports are run-scoped,
3. session status is separate from run lifecycle.

## Design Stance

### 1) Traces are deterministic state, not debug logs

Run traces should be derived from admitted inputs and receipts.

The trace can store bounded summaries directly and put large payloads behind blob refs.

Trace entries should cover:

1. run started,
2. run cause/provenance,
3. context planned,
4. LLM turn requested,
5. LLM receipt received,
6. tool calls observed,
7. tool batch planned,
8. tool effect or domain event emitted,
9. stream/progress frame observed,
10. tool receipt settled,
11. intervention requested/applied,
12. run completed/failed/cancelled/interrupted.

### 2) Keep trace payloads bounded

The run state should not become an unbounded blob.

Recommended shape:

1. compact trace entries in run state,
2. large message/tool/effect payloads by hash ref,
3. full receipts already remain journaled,
4. open correlation refs for embedding workflows,
5. trace report views reconstruct detail by following refs when needed.

### 3) Steer is not the same as interrupt

Use distinct semantics:

1. follow-up: user input for the next run or next waiting state,
2. steer: instruction injected into the next model turn of an active run,
3. interrupt: request to stop or cut over an active run,
4. cancel: terminal operator decision for a run,
5. pause/resume: session-level availability/control state.

These should not be collapsed into one host command queue.

### 4) Interrupt must be effect-aware

Deterministic reducer state cannot pretend an external effect stopped until a receipt or rejection is admitted.

Interrupt behavior should be explicit:

1. mark interrupt requested,
2. emit signal/cancel effect where available,
3. block new LLM/tool dispatch while interruption is pending,
4. settle in-flight work through receipts/rejections,
5. transition run lifecycle only through deterministic admitted events.

Fabric matters here because Fabric host sessions expose session signaling and exec progress. Local host adapters and Fabric adapters should converge on the same AOS effect receipts/stream frames.

### 5) Observability should serve tests and operators

The same trace data should be useful for:

1. deterministic unit tests,
2. prompt/tool eval assertions,
3. Demiurge task status output,
4. future UI/operator views,
5. failure triage after replay.

## Scope

### [x] 1) Define run trace contracts

Add contracts for:

1. run trace entry,
2. trace entry kind,
3. run cause/provenance reference,
4. context report reference,
5. LLM turn summary,
6. tool batch summary,
7. effect/domain-event/receipt summary,
8. intervention summary,
9. run outcome summary,
10. open correlation refs for embedding workflows.

Keep the first schema small and extensible.

Done:

1. added `RunTraceEntryKind`, `RunTraceRef`, `RunTraceEntry`, `RunTrace`, and `RunTraceSummary`.
2. kept trace payloads compact: refs, summary strings, and open metadata maps.
3. added open `Custom { kind }` trace entry support for embedding workflows.
4. generated AIR schemas for the trace contracts.

### [x] 2) Attach traces to run state

Required outcome:

1. current run exposes a bounded trace,
2. completed runs keep a bounded trace summary or trace ref,
3. large payloads remain by hash ref,
4. product-specific correlation is stored in open metadata/refs rather than new trace variants,
5. trace entries are replay-identical.

Done:

1. `RunState` now owns a bounded `RunTrace`.
2. `RunRecord` now keeps a bounded `RunTraceSummary`.
3. trace insertion uses deterministic reducer timestamps and monotonic per-run sequence numbers.
4. bounded trace behavior drops oldest entries and increments `dropped_entries`.
5. removed `conversation_message_refs`; durable message history is now `transcript_message_refs`, while actual model input is represented by `context_plan.selected_refs` and trace entries.

### [x] 3) Record context and LLM turn trace entries

Required outcome:

1. context plan/report is recorded before LLM dispatch,
2. LLM request summary records provider/model/message refs/tool refs,
3. LLM receipt summary records output ref, status, finish reason when available,
4. failures record typed cause and relevant refs.

Done:

1. run start records `RunStarted` with cause/input refs.
2. context planning records `ContextPlanned` with selected refs and context report metadata.
3. LLM dispatch records `LlmRequested` with provider/model/message refs/tool refs.
4. LLM receipts record `LlmReceived` with output ref, provider id, status, and finish reason.
5. run completion/failure/cancellation records `RunFinished` before summary is retained in run history.

### [x] 4) Record tool/effect trace entries

Required outcome:

1. observed tool calls are recorded,
2. tool batch plan hash/ref is recorded,
3. each emitted tool effect records effect kind, params hash, issuer ref, and call id,
4. each emitted domain event records schema, payload hash/ref, key when present, and call id,
5. stream frames/progress frames are recorded as bounded summaries,
6. receipts update the trace with status and output refs.

Done:

1. observed LLM tool calls record `ToolCallsObserved`.
2. tool planning records `ToolBatchPlanned` with plan hash, call ids, accepted count, and group count.
3. emitted LLM/blob/tool effects record either `LlmRequested` or `EffectEmitted` with effect kind, params hash, and issuer ref when present.
4. emitted domain events record `DomainEventEmitted` with schema and payload hash.
5. stream frames record `StreamFrameObserved` with effect, kind, sequence, payload size, and refs.
6. admitted receipts/rejections record `ReceiptSettled` with effect, status, params hash, issuer ref, and intent id.

### [ ] 5) Replace ad hoc steer/follow-up queues

Add explicit intervention operations for:

1. append follow-up input,
2. steer active run,
3. interrupt active run,
4. cancel active run,
5. pause/resume session.

Required outcome:

1. steer has defined placement in the next model turn,
2. follow-up starts or queues a later run,
3. interrupt blocks further turn dispatch until resolved,
4. cancel has deterministic terminal semantics,
5. all intervention requests are trace entries.

Current cut:

1. existing host commands now record `InterventionRequested` trace entries.
2. ref-based follow-up, steer, interrupt, and cancel operations still need to replace `pending_steer` and `pending_follow_up`.
3. interrupt-aware dispatch blocking and deterministic cancellation semantics remain pending.

### [ ] 6) Add signal integration

Integrate host signaling with run interruption:

1. active host session can receive an interrupt/cancel signal when supported,
2. unsupported signals produce a typed trace entry and deterministic fallback,
3. Fabric and local host adapters use the same AOS host signal effect contract,
4. exec progress stream frames are traceable.

### [ ] 7) Update eval and Demiurge surfaces

Required outcome:

1. `aos-harness-py` deterministic fixtures can assert trace events without live provider nondeterminism,
2. Demiurge task status includes current run lifecycle and last meaningful trace event,
3. failure output includes typed cause and relevant output refs,
4. existing `aos-agent-eval` live behavior still works as the provider/tool acceptance lane,
5. trace fixture requirements align with `roadmap/v0.30-agent/p10-agent-sdk-testing.md`.

Current cut:

1. `aos-agent-eval` now reads durable `transcript_message_refs` instead of the removed conversation mirror.
2. Demiurge still compiles against the trace-enabled state model.
3. task status surfacing of last meaningful trace event remains pending.

## Non-Goals

P7 does **not** attempt:

1. final UI design,
2. full deterministic scripted-LLM eval harness beyond the trace hooks needed by P10,
3. subagent supervision,
4. semantic memory,
5. factory-specific work ledger trace schemas,
6. policy/capability gating or approval semantics,
7. marketplace/package concepts.

## Acceptance Criteria

1. [x] A run exposes a deterministic trace containing context, LLM, tool, effect, receipt, and intervention summaries.
2. [x] Trace storage is bounded and large payloads stay behind refs.
3. [x] Run cause/provenance and open correlation refs are visible without product-specific trace variants.
4. [x] Domain-event tool emissions are traceable alongside effect emissions.
5. [ ] Steer, follow-up, interrupt, cancel, pause, and resume have distinct semantics.
6. [ ] Interrupt/cancel does not claim external work stopped until an admitted receipt/rejection supports the transition.
7. [ ] Local and Fabric-backed host signaling can share the same agent-level intervention model.
8. [ ] Demiurge or a focused fixture proves live intervention and trace inspection.
