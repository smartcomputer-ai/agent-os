# `aos-harness-py`

Python bindings for two test lanes:

- `WorkflowHarness`
  isolated, kernel-first workflow tests with scripted receipt choreography
- `WorldHarness`
  runtime-backed single-world tests on the unified `aos-node` SQLite runtime

## Current Shape

`WorkflowHarness` is the narrow deterministic lane:

- builds a `LoadedManifest` directly from authored AIR/workflow inputs
- runs one kernel in-process
- exposes `pull_effects()` and `apply_receipt_object()` for scripted effect tests
- supports logical timer control helpers such as `time_set()` / `time_jump_next_due()`
- exposes effect intent identity by canonical `effect` name plus optional resolved definition and
  executor identity fields (`effect_hash`, `executor_module`, `executor_module_hash`,
  `executor_entrypoint`)

`WorldHarness` is the realistic world lane:

- stages authored inputs, creates a real local world, and reopens it through the unified `aos-node`
  runtime in-process
- exercises real world creation, state reads, journal/checkpoint flow, and trace queries
- does not expose scripted effect pulling or logical time control

That split is intentional:

- use `WorkflowHarness` to unit-test workflow behavior
- use `WorldHarness` to test world/runtime behavior

## Important Constraints

- `aos-harness-py` no longer depends directly on `aos-runtime`
- `effect_mode="scripted"` is the only supported mode today
- `WorldHarness` has one backend: the unified in-process node runtime with a SQLite journal
- effect receipt helpers expect AIR v2 effect names such as `sys/timer.set@1`, not removed v1
  effect-kind strings like `timer.set`

## Example

See [examples/timer_smoke.py](./examples/timer_smoke.py) for the scripted workflow lane.

## Agent Helpers

`aos_harness.agent` provides small Python builders around the reusable
`aos.agent/SessionWorkflow@1` contracts. The helpers are meant for custom agent workflow tests that
need deterministic session input, effect inspection, scripted LLM receipts, and state assertions
without live provider credentials.

Typical flow:

```python
from aos_harness.agent import (
    agent_workflow,
    apply_llm_generate_ok,
    expect_llm_generate,
    respond_llm_output_blob,
    run_requested,
    send_session_input,
)

harness = agent_workflow()
send_session_input(
    harness,
    "session-1",
    1,
    run_requested("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
)
harness.run_to_idle()

llm = expect_llm_generate(harness.pull_effects())
apply_llm_generate_ok(
    harness,
    llm,
    output_ref="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
)
harness.run_to_idle()

blob_get = harness.pull_effects()[0]
respond_llm_output_blob(harness, blob_get, assistant_text="done")
harness.run_to_idle()
```

Tool registries can be installed with `tool_spec()` and `tool_registry_set()` when a custom agent
needs host, effect, domain-event, inspect, or workspace tools in its turn plan.

For existing `aos-agent` acceptance coverage, keep using Rust unit tests plus `aos-agent-eval`.
The Python agent helpers are additive and do not refactor the live eval lane.
