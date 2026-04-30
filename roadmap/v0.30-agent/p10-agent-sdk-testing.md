# P10: Agent SDK Deterministic Harness

**Priority**: P1
**Effort**: Medium
**Risk if deferred**: High (SDK behavior will be validated mostly through live LLM evals, making regressions slow, flaky, and hard to diagnose)
**Status**: Proposed
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.30-agent/p6-turn-planner.md`, `roadmap/v0.30-agent/p7-run-traces-and-intervention.md`

## Goal

Make `aos-harness-py` the first-class deterministic harness for `aos-agent` SDK behavior, while keeping `aos-agent-eval` unchanged as the live provider/tool acceptance lane.

Primary outcome:

1. SDK correctness tests are deterministic and script external effects,
2. live LLM/provider evals remain available but are not the foundation for reducer correctness,
3. agent fixtures are easier to write, inspect, and extend from Python,
4. traces, turn plans, run state, and intervention can be asserted through one Python helper layer,
5. current `aos-agent-eval` remains available and does not block deterministic SDK coverage.

## Current Fit

`aos-agent-eval` is useful today but has the wrong long-term center of gravity for SDK testing.

It currently:

1. creates a temporary world,
2. compiles and patches the agent workflow module,
3. seeds a per-attempt workspace,
4. installs registry/profile state,
5. sends `RunRequested`,
6. executes real host adapters,
7. executes `sys/llm.generate@1` against live providers,
8. asserts tool usage, assistant text, tool output, and file state.

That is valuable as a live prompt/tool acceptance lane. It is not deterministic in the model-output sense.

`aos-harness-py` already has the better substrate for SDK tests:

1. `WorkflowHarness` runs a single workflow in-process with scripted effect choreography,
2. `WorkflowHarness.pull_effects()` exposes emitted external effects,
3. `WorkflowHarness.apply_receipt_object()` admits scripted receipts,
4. typed helpers already exist for LLM, blob, HTTP, and timer receipts,
5. state, blob, trace, snapshot, and reopen helpers are available,
6. `WorldHarness` exercises the unified node runtime and SQLite journal for realistic world tests.

The gap is not the low-level substrate. The gap is a small agent-specific Python driver layer.

## Design Stance

### 1) Test lanes should be explicit

Use three lanes:

1. Rust unit tests in `crates/aos-agent` for reducer/helper invariants,
2. Python deterministic harness tests through `aos-harness-py` for SDK/workflow integration,
3. live provider/tool evals through `aos-agent-eval` or its successor for acceptance and quality checks.

Do not use live LLM behavior to prove deterministic SDK semantics.

### 2) `aos-harness-py` is the future SDK harness

New SDK integration coverage should be written against `aos-harness-py` unless the test is a narrow Rust unit invariant.

For the first cut, the Python harness should own:

1. session and run lifecycle fixtures,
2. turn-plan assertions,
3. run trace assertions,
4. scripted LLM turns,
5. scripted tool batches and receipts,
6. basic intervention flows such as steer, follow-up, and interrupt,
7. replay/reopen assertions,
8. non-user/domain-event run-cause fixtures.

Fabric fake-controller tests, broad eval-case migration, and live-gated hosted execution remain later work.

### 3) Keep `aos-agent-eval` during migration

`aos-agent-eval` should not be expanded into a second SDK harness.

Keep it for:

1. current live prompt/tool coverage,
2. provider compatibility checks,
3. host adapter smoke coverage,
4. source cases to port into deterministic Python fixtures,
5. a live acceptance lane after the deterministic harness exists.

Do not decide the future of `aos-agent-eval` in this first cut. It can keep serving the live acceptance lane while the Python deterministic lane matures.

### 4) Scripted LLM evals are receipt choreography

A deterministic scripted-LLM eval means:

1. send session/run input,
2. run the workflow until it emits `sys/llm.generate@1`,
3. inspect the emitted LLM params,
4. choose a known output ref,
5. admit a matching `LlmGenerateReceipt`,
6. answer the workflow's follow-up `sys/blob.get@1` with a fixed `LlmOutputEnvelope`,
7. script any tool-call argument blobs and tool receipts,
8. assert final run state, trace entries, turn reports, usage, and replay behavior.

The model is not called in this lane. The test decides exactly what the model "said."

The first cut does not need arbitrary test blob insertion. The workflow asks for blobs through
`sys/blob.get@1`, and tests can return deterministic bytes through the existing receipt helper.

### 5) Live evals are still important

Live evals answer different questions:

1. does the current prompt/tool schema work with a provider model,
2. does the model choose the expected tools,
3. do real host adapters behave correctly,
4. do token budgets and provider-specific fields work,
5. are regressions visible against a representative model.

Those tests can use pass-rate thresholds and provider credentials. They should not be required for ordinary SDK correctness.

## Scope

### [ ] 1) Document and enforce the test lane split

Required outcome:

1. `aos-agent` contributor docs say when to use Rust tests, `aos-harness-py`, and live evals,
2. roadmap items P4-P9 reference the right lane for their acceptance tests,
3. live provider credentials are never required for deterministic SDK tests.

First cut:

1. document that `aos-agent-eval` stays as the live provider/tool lane for now.
2. document that new SDK integration tests use `aos-harness-py` unless a Rust unit test is enough.
3. document that provider credentials and real host/Fabric infrastructure are not required for the deterministic lane.

### [ ] 2) Add an `aos_harness.agent` helper module

Add Python helpers for:

1. opening `crates/aos-agent` as a `WorkflowHarness` for `aos.agent/SessionWorkflow@1`,
2. constants for `aos.agent/SessionWorkflow@1` and `aos.agent/SessionInput@1`,
3. sending session ingress events,
4. building `SessionInput` variants,
5. installing explicit test tool registries and profiles when needed,
6. faking ready host session state through `HostSessionUpdated`,
7. reading keyed `SessionState` by session id,
8. constructing `RunCause` payloads for direct ingress and domain-event-origin runs.

These helpers should be small wrappers over existing `WorkflowHarness` primitives.

Suggested helper names:

1. `agent_workflow(...)`,
2. `session_input(session_id, observed_at_ns, input_kind)`,
3. `run_requested(...)`,
4. `run_start_requested(...)`,
5. `host_session_updated(...)`,
6. `turn_observed(...)`,
7. `run_steer_requested(...)`,
8. `run_interrupt_requested(...)`,
9. `follow_up_input_appended(...)`.

### [ ] 3) Add scripted LLM and blob helpers

Required outcome:

1. tests can build a valid `LlmOutputEnvelope` from Python,
2. tests can build a valid `LlmToolCallList` from Python,
3. tests can find and assert emitted `sys/llm.generate@1` params,
4. tests can admit an `llm_generate_ok` receipt with deterministic token usage,
5. tests can respond to the LLM output `blob.get` path,
6. tests can script tool-call argument blobs when tool calls are present.

Arbitrary blob insertion is not required for the first cut. Use `blob_get_ok()` when the workflow
requests known blob refs.

Suggested helper names:

1. `find_effect(effects, effect)`,
2. `expect_llm_generate(...)`,
3. `llm_output_envelope_bytes(...)`,
4. `llm_tool_calls_bytes(...)`,
5. `apply_llm_generate_ok(...)`,
6. `respond_blob_get_bytes(...)`.

### [ ] 4) Add session state, turn plan, and trace assertions

Add Python helpers for:

1. current run lookup,
2. current turn plan lookup,
3. selected message refs,
4. selected tool ids,
5. run trace kinds,
6. last trace kind,
7. trace contains kind,
8. run history summaries,
9. `last_llm_usage`.

The first cut should assert `RunStarted`, `TurnPlanned`, `LlmRequested`, `LlmReceived`, and
`RunFinished` where applicable.

### [ ] 5) Add first deterministic agent fixtures

Add a narrow set first:

1. no-tool LLM completion:
   run starts, LLM is requested, output envelope is read through `blob.get`, run completes, trace and usage are recorded.
2. host-session-ready tool planning:
   a ready host session causes host tools to appear in the turn plan.
3. scripted tool-call path:
   model returns a known tool call, the harness scripts the required blobs/receipts, and the workflow queues the follow-up turn.
4. intervention path:
   steer is injected into the next turn or interrupt blocks further dispatch.
5. domain-event run cause:
   `RunStartRequested` with a non-user `RunCause` starts a run and records provenance.

The goal is to prove the Python harness can express the agent session story deterministically, not
to port every live eval case.

### [ ] 6) Add replay/reopen checks

Required outcome:

1. fixtures can snapshot/reopen the harness,
2. reopened state preserves session/run state,
3. reopened trace summaries match the original deterministic run,
4. replay/reopen checks do not require provider credentials or live adapters.

### [ ] 7) Defer broad eval migration and Fabric modes

Deferred:

1. porting many `aos-agent-eval` cases,
2. changing or replacing `aos-agent-eval`,
3. live providers,
4. real host execution,
5. Fabric fake-controller tests,
6. Fabric live-gated tests,
7. direct skill resolver tests,
8. a shared JSON case format for scripted and live runners.

## Non-Goals

P10 does **not** attempt:

1. replacing low-level Rust reducer tests,
2. making live LLM/provider evals deterministic,
3. building a benchmark leaderboard,
4. final product telemetry or UI,
5. testing full scheduler/heartbeat or factory work-item workflows as part of `aos-agent` SDK correctness,
6. requiring Fabric for ordinary SDK tests,
7. replacing `aos-agent-eval` in this phase.

## Acceptance Criteria

1. New SDK integration tests can run through `aos-harness-py` without provider credentials.
2. `aos_harness.agent` can open the `aos-agent` session workflow and send typed session inputs.
3. The Python harness can script an LLM turn and its follow-up blob reads.
4. The Python harness can assert turn plans, selected tools, run traces, run history, and `last_llm_usage`.
5. The Python harness can start a run with a non-user/domain-event `RunCause`.
6. At least three deterministic Python fixtures cover no-tool completion, host-ready planning, tool-call flow, intervention, or domain-event cause.
7. Replay/reopen preserves the asserted session/run state and trace summaries.
8. `aos-agent-eval` remains available unchanged for live provider/tool acceptance.
9. Fabric and broad eval migration are explicitly deferred.
