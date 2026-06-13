# P10: Agent SDK Deterministic Harness

**Priority**: P1
**Effort**: Medium
**Risk if deferred**: Medium (new agent workflows will lack a first-class deterministic Python lane and will keep leaning on ad hoc live evals)
**Status**: Completed
**Depends on**: `roadmap/v0.23-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.23-agent/p6-turn-planner.md`, `roadmap/v0.23-agent/p7-run-traces-and-intervention.md`

## Goal

Make `aos-harness-py` the first-class deterministic harness for new agent-style workflow
testing, while keeping current `aos-agent` coverage centered on Rust unit tests plus
`aos-agent-eval` for now.

Primary outcome:

1. new agent workflows can test SDK-style behavior deterministically and script external effects,
2. `aos-agent` keeps using reducer/unit tests for deterministic core behavior,
3. agent fixtures are easier to write, inspect, and extend from Python,
4. traces, turn plans, run state, and intervention can be asserted through one Python helper layer,
5. current `aos-agent-eval` remains available unchanged as the live provider/tool acceptance lane.

## Current Fit

`aos-agent-eval` is useful today and should remain the live acceptance lane for `aos-agent`.

It currently:

1. creates a temporary world,
2. compiles and patches the agent workflow module,
3. seeds a per-attempt workspace,
4. installs registry/profile state,
5. sends `RunRequested`,
6. executes real host adapters,
7. executes `sys/llm.generate@1` against live providers,
8. asserts tool usage, assistant text, tool output, and file state.

That is valuable as a live prompt/tool acceptance lane. It is not deterministic in the model-output
sense, and this P10 cut does not try to refactor it into a deterministic harness.

`aos-harness-py` already has the better substrate for SDK tests:

1. `WorkflowHarness` runs a single workflow in-process with scripted effect choreography,
2. `WorkflowHarness.pull_effects()` exposes emitted external effects,
3. `WorkflowHarness.apply_receipt_object()` admits scripted receipts,
4. typed helpers already exist for LLM, blob, HTTP, and timer receipts,
5. state, blob, trace, snapshot, and reopen helpers are available,
6. `WorldHarness` exercises the unified node runtime and SQLite journal for realistic world tests.

The gap is not the low-level substrate. The gap is a small agent-oriented Python driver layer that
new agent workflows can use without depending on provider credentials or live host infrastructure.

## Design Stance

### 1) Test lanes should be explicit

Use three lanes:

1. Rust unit tests in `crates/aos-agent` for reducer/helper invariants,
2. live provider/tool evals through `aos-agent-eval` for current `aos-agent` acceptance and quality checks,
3. Python deterministic harness tests through `aos-harness-py` for new agent workflow integration and future migrated coverage.

Do not refactor `aos-agent-eval` in this phase. For `aos-agent` itself, use Rust unit tests for
deterministic reducer semantics and keep `aos-agent-eval` for live provider/tool behavior.

### 2) `aos-harness-py` is the new-agent deterministic harness

New agent workflows should be able to write deterministic integration coverage against
`aos-harness-py`. Existing `aos-agent` acceptance coverage can remain in `aos-agent-eval` until
there is a specific reason to port cases.

For the first cut, the Python harness should own:

1. session and run lifecycle fixtures,
2. turn-plan assertions,
3. run trace assertions,
4. scripted LLM turns,
5. scripted tool batches and receipts,
6. basic intervention flows such as steer, follow-up, and interrupt,
7. replay/reopen assertions,
8. non-user/domain-event run-cause fixtures.

Fabric fake-controller tests, broad eval-case migration, `aos-agent-eval` refactoring, and live-gated
hosted execution remain later work.

### 3) Keep `aos-agent-eval` during migration

`aos-agent-eval` should not be expanded into a second deterministic SDK harness and should not be
refactored as part of this P10 cut.

Keep it for:

1. current live prompt/tool coverage,
2. provider compatibility checks,
3. host adapter smoke coverage,
4. current `aos-agent` workflow acceptance while the Python deterministic lane matures for new agents.

Do not decide the future of `aos-agent-eval` in this first cut. It keeps serving the live acceptance
lane, and `aos-agent` continues to use unit tests plus `aos-agent-eval` for now.

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

Those tests can use pass-rate thresholds and provider credentials. They remain the right lane for
current `aos-agent` provider/tool acceptance, while deterministic reducer semantics stay in unit
tests.

## Scope

### [x] 1) Document and enforce the test lane split

Required outcome:

1. contributor docs say when to use Rust tests, `aos-harness-py`, and live evals,
2. roadmap items P4-P9 reference the right lane for their acceptance tests,
3. live provider credentials are never required for deterministic harness tests.

First cut:

1. document that `aos-agent-eval` stays as the live provider/tool lane for now.
2. document that `aos-agent` itself continues to use Rust unit tests plus `aos-agent-eval` for now.
3. document that new agent workflows should be able to use `aos-harness-py` for deterministic integration tests.
4. document that provider credentials and real host/Fabric infrastructure are not required for the deterministic lane.

Done:

1. P10 now records the lane split: current `aos-agent` stays on Rust unit tests plus
   `aos-agent-eval`; new agent workflows can use `aos-harness-py`.
2. P5 was updated so pending `aos-harness-py` coverage is not a blocker for current `aos-agent`
   acceptance.
3. `aos-agent-eval` refactoring/replacement is explicitly a non-goal for this phase.
4. `crates/aos-harness-py/README.md` documents the additive Python agent helper lane.

### [x] 2) Add an `aos_harness.agent` helper module

Add Python helpers for new agent workflow fixtures:

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

Done:

1. added `crates/aos-harness-py/python/aos_harness/agent.py`.
2. added `agent_workflow(...)`, including a default one-bin shim for the reusable
   `aos.agent/SessionWorkflow@1` and explicit `source_root`/`workflow_dir` support for custom
   agent workflows.
3. added constants for `aos.agent/SessionWorkflow@1` and `aos.agent/SessionInput@1`.
4. added session input builders including `session_input`, `run_requested`,
   `run_start_requested`, `host_session_updated`, `turn_observed`, `run_steer_requested`,
   `run_interrupt_requested`, and `follow_up_input_appended`.
5. added `direct_run_cause`, `domain_event_run_cause`, and `cause_ref`.
6. added tool registry convenience builders including `tool_spec`, `tool_registry_set`, and
   executor/mapper helpers.

### [x] 3) Add scripted LLM and blob helpers

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

Done:

1. added `find_effect` and `expect_llm_generate`.
2. added `llm_output_envelope_bytes`, `llm_tool_calls_bytes`, and `llm_tool_call`.
3. added `apply_llm_generate_ok`, `respond_blob_get_bytes`, `respond_llm_output_blob`, and
   `respond_llm_tool_calls_blob`.
4. these helpers reuse the existing typed receipt helpers and `WorkflowHarness` primitives.

### [x] 4) Add session state, turn plan, and trace assertions

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

Done:

1. added `session_state` and `session_key`.
2. added `current_run`, `current_turn_plan`, `selected_message_refs`, and `selected_tool_ids`.
3. added `run_trace_entries`, `run_trace_kinds`, `last_trace_kind`, `trace_contains_kind`, and
   `require_trace_kinds`.
4. added `run_history_summaries` and `last_llm_usage`.

### [x] 5) Add first deterministic agent fixtures

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

The goal is to prove the Python harness can express an agent session story deterministically for
new agent workflows, not to port current `aos-agent-eval` cases.

Progress:

1. `crates/aos-harness-py/tests/test_agent_workflow.py` adds checked pytest coverage for no-tool
   completion, host-session-ready tool planning, scripted tool-call follow-up, interrupt
   intervention, and domain-event run cause provenance.

### [x] 6) Add replay/reopen checks

Required outcome:

1. fixtures can snapshot/reopen the harness,
2. reopened state preserves session/run state,
3. reopened trace summaries match the original deterministic run,
4. replay/reopen checks do not require provider credentials or live adapters.

Progress:

1. `crates/aos-harness-py/tests/test_agent_workflow.py` includes checked reopen coverage for the
   no-tool deterministic run.

### [x] 7) Defer broad eval migration and Fabric modes

Deferred:

1. porting many `aos-agent-eval` cases,
2. changing or replacing `aos-agent-eval`,
3. live providers,
4. real host execution,
5. Fabric fake-controller tests,
6. Fabric live-gated tests,
7. direct skill resolver tests,
8. a shared JSON case format for scripted and live runners.

Done:

1. P10 explicitly defers broad `aos-agent-eval` migration, changing/replacing `aos-agent-eval`,
   Fabric modes, live providers, and shared scripted/live case formats.
2. The implemented Python helper lane is additive and does not refactor `aos-agent-eval`.

## Non-Goals

P10 does **not** attempt:

1. replacing low-level Rust reducer tests,
2. making live LLM/provider evals deterministic,
3. building a benchmark leaderboard,
4. final product telemetry or UI,
5. testing full scheduler/heartbeat or factory work-item workflows as part of `aos-agent` SDK correctness,
6. requiring Fabric for ordinary SDK tests,
7. replacing or refactoring `aos-agent-eval` in this phase,
8. forcing existing `aos-agent` acceptance tests onto `aos-harness-py`.

## Acceptance Criteria

1. New agent workflow integration tests can run through `aos-harness-py` without provider credentials.
2. `aos_harness.agent` can open an agent session workflow and send typed session inputs.
3. The Python harness can script an LLM turn and its follow-up blob reads.
4. The Python harness can assert turn plans, selected tools, run traces, run history, and `last_llm_usage`.
5. The Python harness can start a run with a non-user/domain-event `RunCause`.
6. At least three deterministic Python fixtures cover no-tool completion, host-ready planning, tool-call flow, intervention, or domain-event cause for the harness lane.
7. Replay/reopen preserves the asserted session/run state and trace summaries.
8. `aos-agent-eval` remains available unchanged for live provider/tool acceptance.
9. Fabric and broad eval migration are explicitly deferred.
