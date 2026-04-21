# P7: Python Effects

Status: planned.

## Goal

Add the first Python runtime lane on top of the completed op model.

This phase should enable:

```text
WASM workflow op -> Python effect op -> typed receipt
```

Python workflows are a later milestone. Start with Python effects because effects are already the nondeterministic side of the architecture.

## Work

- Define Python bundle artifact format for `defmodule.runtime.kind = "python"`.
- Add a Python effect calling convention such as `python_async_effect`.
- Add a generic Python effect runner interface:
  - bundle hash
  - entrypoint
  - canonical params
  - effect op identity/hash
  - intent hash
  - idempotency key
  - secret/runtime context
  - tracing context
- Add node-side runner integration.
- Validate returned receipt payload against the recorded effect op receipt schema.
- Produce normal terminal receipts for Python exceptions.
- Add a minimal Python SDK authoring surface:
  - `@effect`
  - Pydantic/type-to-AIR schema generation for a small supported subset
  - generated `defschema`, `defmodule`, and `defop`
- Package a content-addressed Python bundle and upload it to CAS.
- Add a small e2e fixture: WASM workflow emits a custom Python effect.

## Non-Goals

- Python workflow reducers.
- Python pure functions.
- Coroutine workflow syntax.
- Perfect Python sandboxing.
- Full native dependency packaging story.
- Multi-tenant hosted security.

## Main Touch Points

- new Python runner crate or service boundary, depending on implementation choice
- `crates/aos-node/src/execution`
- `crates/aos-effect-adapters` or a new runtime adapter layer
- `crates/aos-authoring`
- `crates/aos-cli`
- `.venv` / `setup-python.sh` support if Python tooling is in-repo
- `crates/aos-harness-py`
- new fixtures under smoke or roadmap test assets

## Done When

- A user can write a Python async function, build it into a content-addressed bundle, and activate it as an effect op.
- A WASM workflow can emit that effect op.
- The node starts the Python runner only after durable append.
- The Python runner returns a receipt that is schema-normalized by the kernel.
- Python effect failures produce normal error receipts and do not mutate world state directly.

