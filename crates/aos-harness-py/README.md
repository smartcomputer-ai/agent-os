# `aos-harness-py`

Python bindings and helper modules for the AgentOS harness surfaces.

This crate is a mixed Rust/Python package:

- Rust exposes the native bridge in `aos_harness._core`
- Python adds typed helpers, fixture utilities, and recommended test entrypoints in `aos_harness`

The package now follows the same split as the v0.16 roadmap:

- `WorkflowHarness` for narrow, deterministic workflow tests
- `WorldHarness` for realistic single-world tests

## Package Layout

```text
crates/aos-harness-py/
├── Cargo.toml
├── pyproject.toml
├── src/lib.rs
├── python/aos_harness/
│   ├── __init__.py
│   ├── _core.pyi
│   ├── py.typed
│   ├── types.py
│   ├── receipts.py
│   ├── fixtures.py
│   └── testing.py
└── examples/
    └── timer_smoke.py
```

## The Two Harnesses

### `WorkflowHarness`

Use this for workflow-focused tests.

Intended properties:

- deterministic
- usually `Ephemeral`
- pointed at authored AIR plus an optional workflow crate
- no full authored-world root required
- ideal for receipt choreography, timer control, and state assertions

Primary constructors:

- `WorkflowHarness.from_workflow_dir(...)`
- `WorkflowHarness.from_air_dir(...)`

Authored workflow constructors also accept `build_profile="debug" | "release"`.

### `WorldHarness`

Use this for realistic world tests.

Intended properties:

- single-world execution
- often `PersistedLocal`
- full manifest semantics
- snapshots, reopen, and realistic local runtime behavior in scope

Primary constructors:

- `WorldHarness.from_world_dir(...)`
- `WorldHarness.from_persisted_world_dir(...)`
- `WorldHarness.from_ephemeral_world_dir(...)`

## How It Hangs Together

There are three layers.

### 1) Native bridge: `aos_harness._core`

The Rust extension exposes:

- `WorkflowHarness`
- `WorldHarness`
- core send/run/effect/state/snapshot/reopen/time/artifact methods
- low-level receipt constructors

This layer stays thin over:

- `aos-runtime`
- `aos-authoring`

### 2) Python SDK layer: `aos_harness`

The Python package adds:

- `aos_harness.receipts`
  - helpers like `timer_set_ok(...)`, `http_request_ok(...)`, `llm_generate_ok(...)`
  - normalizes bridge details such as JSON-shaped `intent_hash`
- `aos_harness.fixtures`
  - smoke-fixture discovery and world-staging helpers
  - temp-world setup for world tests that need an authored local root
- `aos_harness.testing`
  - recommended entrypoints like:
    - `workflow_from_smoke_fixture(...)`
    - `workflow_from_authored_dir(...)`
    - `world_from_smoke_fixture(...)`
    - `world_from_authored_dir(...)`
- `aos_harness.types`
  - Python type aliases and `TypedDict` shapes

### 3) Authoring/runtime substrate

Under the package:

- `aos-authoring` prepares authored inputs
- `aos-runtime` provides the shared harness model
- `PersistedLocal` still means the real hosted local path for world tests

## Workflow Tests vs World Tests

This is the most important design rule.

### Workflow tests

A workflow test should be as simple as possible:

1. point to the workflow crate
2. point to minimal AIR defs / manifest
3. run deterministic harness operations

That is why `WorkflowHarness` now has direct authored-input constructors.
The timer example uses this path and does not stage a temp world root anymore.

What is still required:

- a minimal AIR manifest/defs set
- optional import roots if the AIR depends on extra defs
- a workflow crate when a placeholder module hash must be compiled and patched

What is not required:

- a full authored world dir
- `aos.sync.json`
- workspaces
- `PersistedLocal`

### World tests

A world test is different.
A world is defined by its manifest and runtime/storage path, so world tests should still start from an authored world dir or equivalent world bootstrap inputs.

Use `WorldHarness` for:

- persisted local runtime semantics
- multi-workflow coordination
- realistic reopen/snapshot/control operations
- authored world dirs that include `air/`, sync config, imports, workspaces, and local-state behavior

## Recommended Usage

### Workflow-level script

```python
from aos_harness import WorkflowHarness, receipts
from aos_harness.testing import smoke_fixture_root

fixture_root = smoke_fixture_root("01-hello-timer")
harness = WorkflowHarness.from_workflow_dir(
    "demo/TimerSM@1",
    str(fixture_root / "workflow"),
    build_profile="debug",
    effect_mode="scripted",
)

harness.send_event(
    "demo/TimerEvent@1",
    {"Start": {"deliver_at_ns": 1_000_000, "key": "retry"}},
)

while True:
    status = harness.run_to_idle()
    effects = harness.pull_effects()
    if not effects:
        if status.get("runtime_quiescent", status.get("daemon_quiescent", False)):
            break
        raise AssertionError(status)

    for effect in effects:
        harness.apply_receipt_object(
            receipts.timer_set_ok(
                harness,
                effect,
                delivered_at_ns=1_000_000,
                key="retry",
            )
        )
```

See [examples/timer_smoke.py](./examples/timer_smoke.py).

### World-level script

```python
from aos_harness.testing import world_from_smoke_fixture

with world_from_smoke_fixture("00-counter", backend="persisted_local") as harness:
    harness.send_event("demo/CounterEvent@1", {"Start": {"target": 2}})
    harness.run_to_idle()
    state = harness.state_get("demo/CounterSM@1")
```

## Temp Roots and Staging

The temp-root story is now different for the two lanes.

### Workflow lane

Workflow tests no longer require a temp authored-world root in Python.

The narrow authored-input path is:

- `air/`
- optional `workflow/`
- optional import roots

The Rust authoring helper builds a `LoadedManifest` from those inputs and uses a scratch root internally only for local build/cache state.
That scratch root is an implementation detail of the binding, not part of the Python API.

### World lane

World tests still sometimes need staging, because the authored-world path may require:

- a temp local state root
- a minimal generated `aos.sync.json`
- local patching of fixture workflow paths

That logic remains centralized in:

- `stage_authored_world(...)`
- `stage_smoke_fixture(...)`
- `world_from_authored_dir(...)`
- `world_from_smoke_fixture(...)`

For simple smoke fixtures, `include_workspaces=False` remains the default.

## Typing

Typing is provided through:

- `_core.pyi`
- `py.typed`
- `types.py`

This gives the native module a typed Python surface while still allowing pure-Python helpers to add normal type hints on top.

Important types include:

- `BackendName`
- `EffectModeName`
- `JsonValue`
- `EffectObject`
- `ReceiptObject`

## Receipt Helpers

`aos_harness.receipts` provides convenience helpers over the generic receipt API.

Current helpers:

- `timer_set_ok(...)`
- `blob_put_ok(...)`
- `blob_get_ok(...)`
- `http_request_ok(...)`
- `llm_generate_ok(...)`

These work with both `WorkflowHarness` and `WorldHarness`.

## Development Workflow

Install in editable mode:

```bash
uv venv .venv-aos-harness-py
uv pip install --python .venv-aos-harness-py/bin/python maturin
VIRTUAL_ENV=$PWD/.venv-aos-harness-py \
PATH=$PWD/.venv-aos-harness-py/bin:$PATH \
.venv-aos-harness-py/bin/maturin develop --manifest-path crates/aos-harness-py/Cargo.toml
```

Useful checks:

```bash
cargo check -p aos-authoring -p aos-harness-py
cargo test -p aos-authoring build_loaded_manifest_from_authored_paths_supports_workflow_fixtures_without_world_root -- --nocapture
python3 -m py_compile crates/aos-harness-py/python/aos_harness/*.py
.venv-aos-harness-py/bin/python crates/aos-harness-py/examples/timer_smoke.py
AOS_HARNESS_VERBOSE=1 .venv-aos-harness-py/bin/python crates/aos-harness-py/examples/timer_smoke.py
```

Set `AOS_HARNESS_VERBOSE=1` to print coarse timing around harness open/build, event send, idle rounds, effect pulls, and receipt application. This is useful when the first run spends time compiling the workflow crate before any test output appears.

## Current Limitation

The workflow lane is now module-centered from Python, but the world lane still depends on authored-world bootstrap helpers and staged local-world roots when the inputs are not already a complete world dir.

That is acceptable for world tests.
It is not acceptable to leak that complexity back into workflow tests, which is why the two harnesses now have separate entrypoints.
