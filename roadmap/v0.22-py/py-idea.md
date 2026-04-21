# Python Runtime Idea

Status: design note / roadmap sketch

This note describes why AgentOS should add a Python runtime lane, what the product experience should feel like, and how the runtime should fit into the existing kernel/effect architecture. It is intentionally higher level than the AIR schema proposal in `defop-idea.md`.

The short version:

```text
Keep WASM as the hard deterministic lane.
Add Python as the ergonomic lane.
Start with Python effects.
Then add Python workflow reducers with replay profiles.
Package code as content-addressed bundles.
Keep the kernel authoritative over schemas, capabilities, policy, effects, receipts, and replay.
```

## Why Python

AgentOS is currently strong architecturally, but too hard to author for. Workflow code has to be WASM, and custom effects are effectively host-side Rust or hardcoded adapter work. That makes the system correct, but it slows down the most important loop: describing useful agent behavior and connecting it to real tools.

Python is the best first runtime to loosen that bottleneck.

It is the default language for agent glue code, data wrangling, LLM tooling, API clients, notebooks, eval scripts, and AI-generated programs. If an agent or developer wants to connect Slack, Stripe, GitHub, a vector database, a custom HTTP API, or some local package, Python is the place where the odds are best that the library already exists and that generated code will be readable.

The goal is not to make Python replace WASM. The goal is to add a second lane:

```text
WASM   -> strict, deterministic, small trusted surface
Python -> productive, bundled, auditable, good enough by default
```

Users should be able to mix both in the same world:

```text
WASM workflow   -> Python effect
Python workflow -> Rust built-in effect
Python workflow -> Python effect
WASM workflow   -> module-local helper
```

The existing AgentOS effect boundary is already the right shape for this. Workflows emit typed intents as data. Effects execute outside the deterministic owner. Receipts re-enter through the journal. That boundary should be preserved exactly.

## Product Goal

The authoring experience should feel like normal Python:

```python
from pydantic import BaseModel
from aos import workflow, effect

class OrderCreated(BaseModel):
    order_id: str
    amount_cents: int

class SlackPostParams(BaseModel):
    channel: str
    text: str

class SlackPostReceipt(BaseModel):
    ok: bool
    message_id: str | None = None

@effect(
    name="acme/slack.post@1",
    kind="acme.slack.post",
    params=SlackPostParams,
    receipt=SlackPostReceipt,
    cap_type="slack.out",
)
async def post_to_slack(ctx, params: SlackPostParams) -> SlackPostReceipt:
    token = await ctx.secrets.get("slack/bot@1")
    ...

@workflow(
    name="acme/order.step@1",
    state=...,
    event=OrderCreated,
    effects=["acme/slack.post@1"],
)
def order_step(ctx, state, event):
    ...
```

A user should not have to manually write AIR for the common case. The Python SDK should inspect decorators and type annotations, generate AIR schemas/ops, build a bundle, upload that bundle to CAS, and produce a manifest patch.

The important product promise is:

> Write ordinary Python. AgentOS turns it into typed, content-addressed, capability-gated world behavior.

## Design Principles

1. Keep the kernel language-agnostic.

   Python code must not become a special path through the kernel. The kernel should see canonical input bytes, canonical output bytes, schemas, op identity, effect intents, receipts, capabilities, policies, and journal records.

2. Keep effects as the only external I/O path from workflows.

   Python workflows should not make direct network calls, read ambient secrets, spawn subprocesses, or touch host files. They should emit effect intents. Python effects can perform external I/O after the kernel has authorized and durably opened the work.

3. Do not pretend arbitrary Python is strictly deterministic.

   WASM remains the strict replay lane. Python workflows get explicit replay profiles: checked replay or decision-log replay.

4. Make code content-addressed.

   A world should not depend on a GitHub branch, local filesystem path, or custom deployed worker version for replay. Code and dependencies must be bundled, hashed, and pinned.

5. Keep authoring locality, but keep runtime authority in AIR.

   Python decorators provide locality for humans. Generated AIR is the authoritative contract used by the kernel and runtime.

6. Prefer generic language runners over per-program workers.

   Do not deploy one worker per agent-generated program. Deploy generic Python runners that hydrate arbitrary content-addressed bundles.

## Runtime Shape

The Python runtime should be a generic runner outside the deterministic kernel.

```text
AgentOS node
  kernel
    - routing
    - workflow state
    - canonicalization
    - capability checks
    - policy checks
    - open work
    - journal/replay
  runtimes
    - WASM runtime
    - Python workflow runner
    - Python effect runner
    - Rust/builtin adapters
```

The runner API should be small and byte-oriented. It should not expose kernel internals directly.

For workflow execution:

```text
WorkflowStepInput(canonical CBOR)
  -> Python runner
  -> WorkflowStepOutput(canonical CBOR)
```

For effect execution:

```text
EffectIntent(canonical params, intent hash, cap context, idempotency)
  -> Python runner
  -> stream frames and terminal receipt payload
```

The runner can provide Python objects to user code, but those objects are SDK conveniences. The boundary between runner and node should stay explicit, stable, and canonical.

## Milestone 1: Python Effects

Python effects should come first.

They deliver the largest developer experience win with the least architectural risk, because effects are already the nondeterministic side of AgentOS. The kernel already knows how to:

- canonicalize effect params
- check capability grants
- evaluate policy
- record open work
- publish only after durable flush
- admit receipts back through world input
- route receipt continuations to the recorded origin

Python effect support adds a new way to implement external async work.

The user writes:

```python
@effect(...)
async def send_email(ctx, params) -> Receipt:
    ...
```

The builder emits:

```text
schemas for params and receipt
effect op / effect contract metadata
bundle metadata
manifest patch
```

At runtime:

```text
1. Workflow emits an effect intent.
2. Kernel canonicalizes params and checks caps/policy.
3. Kernel records open work.
4. Journal frame durably flushes.
5. Effect runtime starts the Python runner.
6. Python user code performs I/O.
7. Runner validates and canonicalizes receipt payload.
8. Runner signs or stamps receipt envelope.
9. Receipt re-enters as world input.
```

This immediately enables:

```text
WASM workflow -> custom Python effect -> typed receipt
```

That alone is enough to make AOS much more usable.

## Milestone 2: Python Workflow Reducers

Python workflows should be synchronous reducers first.

Do not start with Temporal-style coroutine syntax. It is attractive, but it makes the first implementation much wider. Start with the simplest mental model:

```text
old state + event + context -> new state + domain events + effect intents
```

Python workflows should be plain functions:

```python
@workflow(...)
def step(ctx, state, event):
    ...
    return Step(state=state, events=[], effects=[...])
```

The workflow function may:

- inspect its state
- inspect the event
- use deterministic context supplied by the kernel
- construct domain events
- construct effect intents
- call local helper functions

The workflow function may not:

- perform network I/O
- read ambient secrets
- call time/random directly for authoritative decisions
- spawn subprocesses
- mutate host state
- await external work directly

External work remains an effect.

Later, coroutine workflow syntax can be layered on top as sugar that compiles to reducer state. That is a different milestone.

## Replay Profiles

Python workflow determinism should be explicit. A world author should choose a profile per workflow op.

### Strict

Strict means replay by recomputation and require identical output.

This is the WASM-like promise. For Python, this should be opt-in and narrow. It probably requires tight sandboxing, pinned dependencies, fixed environment, blocked I/O, fixed hash seed, and restricted imports.

This is not the first product bet.

### Checked

Checked means Python is rerun during replay and its canonical output is compared to the original output.

If the output differs, replay fails or reports drift. This is useful for teams that want Python authoring but still want replay to prove behavior.

Checked replay should be supported, but it should not be the default for arbitrary pip-based workflows.

### Decision Log

Decision-log mode is the pragmatic default for Python workflows.

On live execution:

```text
input envelope
  -> Python step
  -> canonical workflow output
  -> store output in CAS if large
  -> append workflow decision record
  -> apply output to world state
```

On replay:

```text
input envelope
  -> match recorded workflow decision
  -> use canonical output
  -> optionally rerun Python in audit mode
```

This weakens the philosophical claim that the workflow state is always recomputed from code. But it preserves the operational property that matters most:

```text
same journal + same CAS + same receipts -> same world state
```

WASM remains the lane for strict recomputation. Python becomes a lane for committed, auditable decisions.

## Artifacts

Python code should be exposed through a content-addressed artifact, not as a pointer to a Git ref or
a custom deployed worker. The stable forms are:

```text
python_bundle  -> packaged tree with bundle metadata, locks, source, wheels, generated AIR
workspace_root -> pinned workspace tree root used directly by the runner
```

The workspace form keeps the edit loop short: edit the workspace, produce a new root hash, and replace
the module definition. It is still replay-stable because AIR pins the exact root hash.

Example bundle:

```text
bundle/
  aos.bundle.json
  pyproject.toml
  uv.lock
  src/
    orders/
      workflow.py
      effects.py
      transforms.py
  wheels/
  generated/
    schemas.air.json
    ops.air.json
  sbom.json
```

The authoritative identity is the artifact root hash. Git metadata can be stored as provenance, but replay must not require GitHub or a branch name.

The bundle manifest should say:

```json
{
  "kind": "aos.python.bundle",
  "version": 1,
  "python": "3.12",
  "targets": [
    {
      "platform": "linux-x86_64",
      "abi": "cp312",
      "lock": "uv.lock",
      "wheelhouse": "wheels/"
    }
  ],
  "entrypoints": {
    "workflow:acme/order.step@1": "orders.workflow:step",
    "effect:acme/slack.post@1": "orders.effects:post_to_slack"
  }
}
```

For normal use, prefer a lockfile plus wheelhouse. For heavier deployments, optionally support PEX or OCI image digests later. The first milestone should not require solving every native dependency story.

## Generic Runners

The runtime should not require one deployed worker per workflow version.

Deploy generic runners:

```text
aos-py-runner
aos-py-effect-runner
aos-wasm-runner
```

The runner receives:

```text
artifact kind
artifact root hash
entrypoint
invocation mode inferred from runtime kind and op kind
input bytes
runtime limits
```

It hydrates the artifact, creates or reuses an isolated environment, invokes the entrypoint, and returns canonical output.

This is what makes agent-generated code practical. Hundreds of generated workflows should create hundreds of artifact roots, not hundreds of bespoke worker deployments.

## Sandboxing

Python workflows and Python effects need different sandboxes.

### Workflow Sandbox

Workflow execution should be restrictive:

- no network
- no secrets
- no subprocess
- no host filesystem except read-only bundle files
- sanitized environment
- deterministic `PYTHONHASHSEED`
- UTC locale/time settings
- no ambient clock/random for authoritative decisions
- import linting for clearly unsafe modules
- CPU and memory limits

The goal is not perfect determinism. The goal is to prevent accidental collapse of the effect boundary and to make checked replay useful.

### Effect Sandbox

Effect execution can access external systems, but only through the effect context granted after kernel authorization.

Effect code should receive:

- canonical params
- intent hash
- idempotency key
- effect op identity
- cap metadata
- approved secret handles
- blob helpers
- stream-frame helper
- logger/tracing context

The runner should validate the returned receipt payload against the AIR receipt schema before constructing the terminal receipt.

## Idempotency And Retries

Python effects must be written as "ensure started or reconciled", not "blindly run once".

The node can restart after open work is durable. The effect runtime can see the same intent again. The adapter must use the intent hash and idempotency key to avoid duplicate external side effects when possible.

The SDK should make the correct pattern easy:

```python
async def effect(ctx, params):
    async with ctx.idempotent_operation() as op:
        ...
```

The exact helper can come later, but the runtime contract should be clear from the beginning.

## Schema Authoring

Pydantic should be an authoring surface, not the runtime authority.

The Python SDK should map Python types into AIR schemas and then the kernel should validate/canonicalize against AIR. Pydantic JSON Schema can be useful for inspection, but AIR schemas should be generated directly from Python type information where possible.

The first supported subset should be deliberately small:

```text
BaseModel       -> record
str             -> text
bool            -> bool
int             -> int or nat with explicit annotation
Decimal         -> dec128
bytes           -> bytes
datetime        -> time
timedelta       -> duration
UUID            -> uuid
list[T]         -> list
set[T]          -> set
dict[str, T]    -> map{text, T}
T | None        -> option<T>
tagged Union    -> variant
```

Unsupported shapes should fail with clear diagnostics. Do not try to support arbitrary JSON Schema on day one.

## Relationship To Ops

The AIR mechanics are described separately in `defop-idea.md`.

At the product level, the important idea is:

```text
bundle = code artifact
op     = typed callable entrypoint
```

A single Python bundle can export:

```text
workflow op
effect op
```

Each op has a kernel role. An effect op is not directly routed from domain events. The op kind determines which kernel subsystem may invoke the entrypoint and which input/output envelope is valid.

Public pure ops are deferred from v0.22. Deterministic helper functions can still live inside a bundle
and be called by workflow/effect code, but AIR does not expose them as independently callable world ops.

This lets Python be broad without making the kernel vague.

## Upgrade Model

Every live execution should pin what it used.

A workflow instance should pin:

```text
workflow op identity
resolved implementation identity
artifact kind and root hash
replay profile
```

An open effect should pin:

```text
effect op identity
resolved implementation identity
artifact kind and root hash
entrypoint
params schema hash
receipt schema hash
```

That lets a world upgrade Python code while old in-flight work still reconciles against the code epoch that opened it.

The conservative first version can still require quiescence for manifest apply. But the identity model should be designed so rolling code epochs are possible later.

## Observability

Python runtime support should be observable from day one.

Useful events and diagnostics:

- bundle hydration time
- environment creation/cache hit
- import errors
- schema generation errors
- workflow step duration
- workflow decision output hash
- checked replay drift
- effect start/retry/receipt timing
- Python exception with sanitized traceback
- resource limit exits
- unsupported dependency/platform errors

The user should be able to answer:

```text
Which bundle ran?
Which entrypoint ran?
Which op invoked it?
Which intent did it handle?
Which receipt did it produce?
Was replay using recomputation or decision log?
```

## Security Notes

Python code should be treated as untrusted user code unless explicitly configured otherwise.

That means:

- process isolation at minimum
- no ambient secrets
- no ambient host filesystem writes
- no implicit network from workflows
- explicit resource limits
- signed or stamped receipts from the runner, not from user code
- auditable bundle identity

The first local developer implementation can be lighter, but the architecture should not depend on trusted in-process Python.

## Non-Goals For v0.22

Do not try to solve everything in the first cut.

Non-goals:

- arbitrary Python determinism
- arbitrary JSON Schema conversion
- full native dependency portability across all platforms
- coroutine workflow syntax
- Python cap enforcers as a default path
- distributed Python runner scheduling
- marketplace/security review of third-party bundles
- deletion GC for old bundles

The first version should make the common path work well.

## Roadmap

### Phase 1: Python Effect Runtime

Build:

- `@effect` decorator
- Pydantic-to-AIR schema generation
- bundle builder
- Python async effect runner
- dynamic effect execution from artifact root and entrypoint
- receipt payload validation/canonicalization
- basic sandboxing
- tests with a WASM workflow calling a Python effect

This phase should not require Python workflows.

### Phase 2: Python Workflow Reducers

Build:

- `@workflow` decorator
- sync reducer invocation mode
- workflow input/output bridge
- decision-log replay mode
- checked replay mode
- workflow sandbox
- tests for replay from journal and CAS

### Phase 3: Bundle Cache And Runner Hardening

Build:

- environment cache keyed by artifact root hash and target
- dependency wheelhouse support
- resource limits
- better diagnostics
- trace integration
- bundle provenance metadata

### Phase 4: Code Epochs

Build:

- op/implementation pinning for workflow instances
- op/implementation pinning for open effects
- old bundle reachability roots
- rolling upgrade semantics

### Phase 5: More Language Runtimes

Once Python is working behind the generic bundle/op boundary, add JS or other runtimes using the same model. Do not design JS as a separate architecture.

## Summary

Python support should make AgentOS dramatically easier to use without weakening the core model.

The core bet is:

```text
Python for authoring and integration.
AIR for contracts and canonicalization.
CAS for code identity.
Effects for external I/O.
Receipts for audit.
Replay profiles for honesty about determinism.
WASM for strict deterministic workloads.
```

If we keep those boundaries, Python becomes an ergonomic runtime lane instead of a shortcut around the architecture.
