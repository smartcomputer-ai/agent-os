# P10: Agent SDK Testing and Eval Harness

**Priority**: P1
**Effort**: Medium
**Risk if deferred**: High (SDK behavior will be validated mostly through live LLM evals, making regressions slow, flaky, and hard to diagnose)
**Status**: Proposed
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`

## Goal

Make `aos-harness-py` the primary test and eval harness for deterministic `aos-agent` SDK behavior, while keeping `aos-agent-eval` as the live provider/tool acceptance lane during the transition.

Primary outcome:

1. SDK correctness tests are deterministic and script external effects,
2. live LLM/provider evals remain available but are not the foundation for reducer correctness,
3. agent fixtures are easier to write, inspect, and extend from Python,
4. traces, turn plans, run state, intervention, and Fabric-backed execution can be asserted through one harness family,
5. current `aos-agent-eval` cases can be ported or reused without losing their live-eval value.

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

The gap is not the low-level substrate. The gap is agent-specific fixture and driver code.

## Design Stance

### 1) Test lanes should be explicit

Use three lanes:

1. Rust unit tests in `crates/aos-agent` for reducer/helper invariants,
2. Python deterministic harness tests through `aos-harness-py` for SDK/workflow integration,
3. live provider/tool evals through `aos-agent-eval` or its successor for acceptance and quality checks.

Do not use live LLM behavior to prove deterministic SDK semantics.

### 2) `aos-harness-py` is the future SDK harness

New SDK integration coverage should be written against `aos-harness-py` unless the test is a narrow Rust unit invariant.

The Python harness should own:

1. session and run lifecycle fixtures,
2. context-plan assertions,
3. run trace assertions,
4. scripted LLM turns,
5. scripted tool batches and receipts,
6. intervention flows such as steer, interrupt, cancel, pause, and resume,
7. replay/reopen assertions,
8. non-user/domain-event run-cause fixtures,
9. Fabric fake-controller and live-gated fixtures.

### 3) Keep `aos-agent-eval` during migration

`aos-agent-eval` should not be expanded into a second SDK harness.

Keep it for:

1. current live prompt/tool coverage,
2. provider compatibility checks,
3. host adapter smoke coverage,
4. source cases to port into deterministic Python fixtures,
5. a live acceptance lane after the deterministic harness exists.

Once Python fixtures cover the deterministic cases, `aos-agent-eval` can shrink to a small live eval runner or be replaced by a Python runner that can switch between scripted and live modes.

### 4) Scripted LLM evals are receipt choreography

A deterministic scripted-LLM eval means:

1. send session/run input,
2. run the workflow until it emits `sys/llm.generate@1`,
3. inspect the emitted LLM params,
4. create a known LLM output blob or scripted output ref,
5. admit a matching `LlmGenerateReceipt`,
6. answer the workflow's follow-up `sys/blob.get@1` with a fixed `LlmOutputEnvelope`,
7. script any tool-call argument blobs and tool receipts,
8. assert final run state, trace entries, turn reports, files, and replay behavior.

The model is not called in this lane. The test decides exactly what the model "said."

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

### [ ] 2) Add agent fixture helpers to `aos-harness-py`

Add Python helpers for:

1. staging the `aos-agent` eval world or a focused session fixture,
2. building/importing the Rust-authored `aos-agent` AIR package,
3. patching or materializing `session_workflow` WASM where needed,
4. sending session ingress events,
5. installing explicit tool registries and profiles,
6. opening or faking host session state,
7. reading keyed `SessionState` by session id,
8. constructing `RunCause` payloads for direct ingress and domain-event-origin runs.

These helpers should be small wrappers over the existing `WorkflowHarness` and `WorldHarness` primitives.

### [ ] 3) Add scripted LLM and blob helpers

Required outcome:

1. tests can build a valid `LlmOutputEnvelope` from Python,
2. tests can store or fake output blob refs deterministically,
3. tests can respond to the LLM output `blob.get` path,
4. tests can script tool-call argument blobs,
5. tests can assert the emitted `LlmGenerateParams` before returning a receipt.

If the current harness cannot store arbitrary blobs directly, add a small API for deterministic test blob insertion rather than encoding that behavior in each test.

### [ ] 4) Port representative `aos-agent-eval` cases

Port a narrow set first:

1. read/write token,
2. apply patch,
3. exec roundtrip,
4. grep or glob,
5. fallback/report case.

The goal is not to duplicate every live eval immediately. The goal is to prove that the Python harness can express the agent session story deterministically.

### [ ] 5) Add trace and intervention assertions

As P7 lands, Python fixtures should assert:

1. run started/completed/failed trace entries,
2. run cause/provenance trace entries,
3. turn-planned entries,
4. LLM turn entries,
5. tool batch entries,
6. effect, domain-event, and receipt summaries,
7. steer/follow-up/interrupt/cancel entries,
8. replay/reopen preserving the same trace state.

### [ ] 6) Add Fabric test modes

As P8 lands, Python fixtures should support:

1. fake Fabric controller tests for deterministic adapter behavior,
2. live Fabric tests behind explicit env/feature gates,
3. host signal and exec progress assertions through run traces,
4. local and Fabric target config comparisons using the same agent-level contracts.

### [ ] 7) Decide the future of `aos-agent-eval`

After the Python lane has enough coverage, choose one:

1. keep `aos-agent-eval` as a small Rust live eval binary,
2. move live eval driving into Python and retire the Rust binary,
3. keep the JSON case format and share it between scripted and live runners.

The decision should be based on maintenance cost and whether live eval reporting benefits from Python fixture ergonomics.

## Non-Goals

P10 does **not** attempt:

1. replacing low-level Rust reducer tests,
2. making live LLM/provider evals deterministic,
3. building a benchmark leaderboard,
4. final product telemetry or UI,
5. testing full scheduler/heartbeat or factory work-item workflows as part of `aos-agent` SDK correctness,
6. requiring Fabric for ordinary SDK tests.

## Acceptance Criteria

1. New SDK integration tests can run through `aos-harness-py` without provider credentials.
2. At least three current `aos-agent-eval` behaviors have deterministic Python harness equivalents.
3. The Python harness can script an LLM turn and its follow-up blob reads.
4. The Python harness can start a run with a non-user/domain-event `RunCause`.
5. The Python harness can assert session/run state, trace summaries, and replay/reopen stability.
6. `aos-agent-eval` remains available for live provider/tool acceptance during migration.
7. P7/P8 trace, intervention, and Fabric fixtures have a clear deterministic test lane.
