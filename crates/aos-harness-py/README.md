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

## Example

See [examples/timer_smoke.py](./examples/timer_smoke.py) for the scripted workflow lane.
