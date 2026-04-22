# defop refactor

Note: this was written by someone without full access to the codebase, so some statements need to be taken with a grain of salt. This is not a spec but a directional memo.

Important: however, as we move towards the op model, we must refactor _agressively_ because we are on an experimental branch and do NOT need to worry about breaking anything. Do not maintain any backward compatibility. Treat it as if we are building on a blank slate.


So bar our design proposals kept this asymmetry:

```text
workflow/pure call surface  -> defined in defmodule
effect call surface         -> defined in defeffect
effect implementation       -> hidden behind manifest effect_bindings.adapter_id
```

That is not the ideal shape. The current specs really do encode that split: `defmodule` is currently only `workflow | pure` and requires a `wasm_hash`; `defeffect` separately owns `params_schema`, `receipt_schema`, `cap_type`, and `origin_scope`; and the manifest maps effect `kind` to a string `adapter_id`.   

My revised recommendation is:

## Introduce one unified executable operation concept

I would separate **code artifact** from **typed executable operation**.

The clean split is:

```text
defschema   = data shapes
defmodule   = executable bundle / runtime artifact
defop       = typed executable entrypoint: workflow, pure function, effect, cap enforcer, etc.
defcap      = capability type and constraints
defpolicy   = authorization rules
manifest    = active catalog, routing, bindings, defaults
```

In this model, `defeffect` disappears as a first-class root form, or becomes compatibility sugar for:

```text
defop { op_kind: "effect", ... }
```

This is the important move: **all callable things are `defop`s**.

A workflow step is a `defop`.

A pure function is a `defop`.

An effect handler is a `defop`.

A capability enforcer is a `defop`.

A Python async integration function is a `defop`.

A WASM reducer is a `defop`.

A Rust built-in host operation can also be represented as a `defop` with `runtime.kind = "builtin"`.

The module becomes the package that contains code; the operation becomes the thing the kernel can call or dispatch.

---

# Why this is cleaner than `defadapter`

The current problem is that `defmodule` and `defeffect` both want to be “the thing with an interface,” but only `defmodule` has an implementation artifact. Adding `defadapter` gives effects an implementation artifact too, but now you have three concepts:

```text
defmodule   -> typed WASM workflow/pure implementation
defeffect   -> typed effect contract
defadapter  -> effect implementation
```

That is better than `adapter_id`, but still not fully unified.

The deeper truth is that there are only two orthogonal things:

```text
1. executable bundle
2. typed entrypoint into that bundle
```

So make those explicit.

---

# Proposed AIR shape

## `defmodule`: runtime bundle only

`defmodule` should stop being “workflow or pure.” It should become “this is a content-addressed executable bundle.”

```json
{
  "$kind": "defmodule",
  "name": "acme/order_bundle@1",
  "runtime": {
    "kind": "python",
    "engine": "cpython",
    "python": "3.12",
    "artifact": {
      "format": "aos.python.bundle.v1",
      "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    "target": "linux-x86_64-cp312"
  }
}
```

For WASM:

```json
{
  "$kind": "defmodule",
  "name": "acme/order_wasm@1",
  "runtime": {
    "kind": "wasm",
    "engine": "wasmtime",
    "artifact": {
      "format": "wasm",
      "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}
```

For a Rust built-in:

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_effects@1",
  "runtime": {
    "kind": "builtin",
    "engine": "aos-host",
    "artifact": {
      "builtin_id": "sys.effects.v1"
    }
  }
}
```

This shared runtime object is where Python/WASM/JS/native/builtin packaging lives. No separate adapter runtime format. No special Python effect format. No separate Python workflow format. One module artifact model.

The existing CAS model is already a good fit for this because the backend contract is logical `hash -> bytes`, and CAS is already used for AIR nodes, WASM modules, snapshots, workspace blobs, and other content-addressed data. 

---

## `defop`: typed executable entrypoint

A `defop` is the local place where the operation’s signature, role, authority, execution class, and implementation entrypoint are defined.

### Workflow op

```json
{
  "$kind": "defop",
  "name": "acme/order.step@1",
  "op_kind": "workflow",
  "workflow": {
    "state": "acme/OrderState@1",
    "event": "acme/OrderEvent@1",
    "context": "sys/WorkflowContext@1",
    "key_schema": "acme/OrderId@1",
    "effects_emitted": [
      "acme/slack.post@1",
      "sys/timer.set@1"
    ],
    "cap_slots": {
      "slack": "slack.out",
      "timer": "timer"
    },
    "determinism": "decision_log"
  },
  "impl": {
    "module": "acme/order_bundle@1",
    "entrypoint": "orders.workflow:step",
    "calling_convention": "sync_reducer"
  }
}
```

### Pure op

```json
{
  "$kind": "defop",
  "name": "acme/normalize_json@1",
  "op_kind": "pure",
  "pure": {
    "input": "acme/RawJson@1",
    "output": "acme/NormalizedJson@1",
    "context": "sys/PureContext@1"
  },
  "impl": {
    "module": "acme/order_bundle@1",
    "entrypoint": "orders.transforms:normalize_json",
    "calling_convention": "sync_function"
  }
}
```

### Effect op

```json
{
  "$kind": "defop",
  "name": "acme/slack.post@1",
  "op_kind": "effect",
  "effect": {
    "kind": "acme.slack.post",
    "params": "acme/SlackPostParams@1",
    "receipt": "acme/SlackPostReceipt@1",
    "cap_type": "slack.out",
    "origin_scope": ["workflow", "system"],
    "execution_class": "external_async"
  },
  "impl": {
    "module": "acme/order_bundle@1",
    "entrypoint": "orders.effects:post_to_slack",
    "calling_convention": "async_effect"
  }
}
```

### Capability enforcer op

```json
{
  "$kind": "defop",
  "name": "sys/CapEnforceHttpOut.check@1",
  "op_kind": "cap_enforcer",
  "pure": {
    "input": "sys/CapCheckInput@1",
    "output": "sys/CapCheckOutput@1"
  },
  "impl": {
    "module": "sys/builtin_cap_enforcers@1",
    "entrypoint": "http_out",
    "calling_convention": "sync_function"
  }
}
```

Then `defcap.enforcer` should reference an op, not a module:

```json
{
  "$kind": "defcap",
  "name": "sys/http.out@1",
  "cap_type": "http.out",
  "schema": "...",
  "enforcer": {
    "op": "sys/CapEnforceHttpOut.check@1"
  }
}
```

That matters because once a single Python/WASM/builtin bundle can export many functions, “module” is too coarse as the target. You want to bind to the actual operation.

---

# What happens to `defeffect`?

I would remove canonical `defeffect`.

Not because effects are unimportant, but because the effect contract becomes this:

```json
{
  "$kind": "defop",
  "op_kind": "effect",
  "effect": {
    "kind": "acme.slack.post",
    "params": "acme/SlackPostParams@1",
    "receipt": "acme/SlackPostReceipt@1",
    "cap_type": "slack.out",
    "origin_scope": ["workflow", "system"],
    "execution_class": "external_async"
  },
  "impl": { "...": "..." }
}
```

That contains everything `defeffect` currently contains, plus the implementation reference. No duplication.

If you still want abstract effect contracts later, add them deliberately as `definterface` or `defport`. But I would not keep `defeffect` as the default path. It is exactly the split that is making this feel awkward.

---

# What happens to adapters?

“Adapter” becomes an implementation style, not an AIR root kind.

An effect op may be implemented by:

```text
Python async function
WASM nondeterministic adapter
Rust built-in
JS/Node function
remote executor
owner-local timer scheduler
internal deterministic kernel function
```

But AIR does not need a separate `defadapter`.

The effect op says:

```json
"execution_class": "external_async"
```

and the implementation says:

```json
"module": "acme/order_bundle@1",
"entrypoint": "orders.effects:post_to_slack",
"calling_convention": "async_effect"
```

The existing effect lifecycle remains: workflows emit intents as data; the kernel canonicalizes params, checks capabilities and policy, records open work, flushes durably, and only then starts async execution. That fence is the important invariant, and this design preserves it. 

---

# Manifest changes

The manifest should stop having a separate `effects` catalog and `effect_bindings`.

Today the manifest has:

```json
"modules": [],
"effects": [],
"effect_bindings": [
  {
    "kind": "acme.slack.post",
    "adapter_id": "slack_adapter"
  }
]
```

I would move to:

```json
{
  "$kind": "manifest",
  "air_version": "2",
  "schemas": [],
  "modules": [],
  "ops": [],
  "caps": [],
  "policies": [],
  "secrets": [],
  "routing": {
    "subscriptions": [
      {
        "event": "acme/OrderEvent@1",
        "op": "acme/order.step@1",
        "key_field": "order_id"
      }
    ]
  },
  "op_bindings": {
    "acme/order.step@1": {
      "slots": {
        "slack": "cap_slack",
        "timer": "cap_timer"
      }
    }
  }
}
```

This is important: bindings should be **op-level**, not module-level.

A single Python bundle may contain:

```text
orders.workflow:step
orders.workflow:refund_step
orders.effects:post_to_slack
orders.effects:charge_card
orders.transforms:normalize_json
```

Those entrypoints should not share one coarse `module_bindings` block. The workflow op should get its own cap slot bindings. The effect op should have its own execution authority model. The pure op may need no bindings.

So:

```text
module_bindings  -> op_bindings
routing.module   -> routing.op
effects list     -> derived from ops where op_kind = effect
effect_bindings  -> gone
```

This also removes the weird `adapter_id` indirection. Effect execution points to a named op and a named module, both content-addressed through the manifest.

---

# Effect identity should become op-based, not string-kind-based

Current effects are centered on `EffectKind`, an open string such as `http.request` or `llm.generate`. That is useful for policy and observability, but too weak as the canonical identity of a typed executable operation.

I would distinguish:

```text
effect op name:  sys/http.request@1
semantic kind:   http.request
op hash:         sha256(...)
```

Workflows should declare and emit effect **op refs**:

```json
"effects_emitted": [
  "sys/http.request@1",
  "acme/slack.post@1"
]
```

The op itself contains the semantic kind:

```json
"effect": {
  "kind": "http.request",
  "params": "sys/HttpRequestParams@1",
  "receipt": "sys/HttpRequestReceipt@1",
  "cap_type": "http.out"
}
```

Policy can still match `effect.kind = "http.request"`.

But canonicalization and dispatch use the op ref/hash, so the runtime knows exactly which schema and implementation are active.

The effect intent should become something like:

```json
{
  "effect_op": "acme/slack.post@1",
  "effect_op_hash": "sha256:...",
  "kind": "acme.slack.post",
  "params_cbor": "...",
  "origin": {
    "op": "acme/order.step@1",
    "op_hash": "sha256:...",
    "instance_key": "..."
  },
  "intent_hash": "sha256:..."
}
```

The `intent_hash` preimage should include both:

```text
origin workflow op hash
effect op hash
origin instance key
effect params
cap grant
emission position
workflow-requested idempotency key
```

This avoids ambiguity across code epochs. It also makes receipt routing stronger. The current workflow contract already says receipt continuation routing is manifest-independent and keyed by recorded origin identity, not normal domain-event routing. 

---

# Built-ins under this model

The built-ins become ordinary ops.

Today `sys/http.request@1` is a `defeffect` entry with params, receipt, cap type, and origin scope. 

In the new model it becomes:

```json
{
  "$kind": "defop",
  "name": "sys/http.request@1",
  "op_kind": "effect",
  "effect": {
    "kind": "http.request",
    "params": "sys/HttpRequestParams@1",
    "receipt": "sys/HttpRequestReceipt@1",
    "cap_type": "http.out",
    "origin_scope": ["workflow", "system", "governance"],
    "execution_class": "external_async"
  },
  "impl": {
    "module": "sys/builtin_effects@1",
    "entrypoint": "http.request",
    "calling_convention": "builtin_effect"
  }
}
```

Timer:

```json
{
  "$kind": "defop",
  "name": "sys/timer.set@1",
  "op_kind": "effect",
  "effect": {
    "kind": "timer.set",
    "params": "sys/TimerSetParams@1",
    "receipt": "sys/TimerSetReceipt@1",
    "cap_type": "timer",
    "origin_scope": ["workflow", "system", "governance"],
    "execution_class": "owner_local_async"
  },
  "impl": {
    "module": "sys/builtin_effects@1",
    "entrypoint": "timer.set",
    "calling_convention": "builtin_effect"
  }
}
```

Workspace/introspection:

```json
"execution_class": "internal_deterministic"
```

That matches the existing effect classes: internal deterministic effects, owner-local async effects, and external async effects. 

---

# Python authoring becomes very natural

A Python package can define many ops in one bundle:

```python
from aos import workflow, effect, pure
from pydantic import BaseModel

class OrderEvent(BaseModel):
    order_id: str
    amount_cents: int

class OrderState(BaseModel):
    status: str = "new"

class SlackPostParams(BaseModel):
    channel: str
    text: str

class SlackPostReceipt(BaseModel):
    ok: bool
    message_id: str | None = None

@workflow(
    name="acme/order.step@1",
    state=OrderState,
    event=OrderEvent,
    effects=["acme/slack.post@1"],
    cap_slots={"slack": "slack.out"},
    determinism="decision_log",
)
def step(ctx, state: OrderState | None, event: OrderEvent):
    ...

@effect(
    name="acme/slack.post@1",
    kind="acme.slack.post",
    params=SlackPostParams,
    receipt=SlackPostReceipt,
    cap_type="slack.out",
    execution_class="external_async",
)
async def post_to_slack(ctx, params: SlackPostParams) -> SlackPostReceipt:
    ...

@pure(
    name="acme/normalize_order@1",
    input=OrderEvent,
    output=OrderEvent,
)
def normalize_order(event: OrderEvent) -> OrderEvent:
    ...
```

The builder emits:

```text
defschema OrderEvent
defschema OrderState
defschema SlackPostParams
defschema SlackPostReceipt

defmodule acme/order_bundle@1
  runtime = python bundle hash

defop acme/order.step@1
  op_kind = workflow
  impl = acme/order_bundle@1 :: orders:step

defop acme/slack.post@1
  op_kind = effect
  impl = acme/order_bundle@1 :: orders:post_to_slack

defop acme/normalize_order@1
  op_kind = pure
  impl = acme/order_bundle@1 :: orders:normalize_order
```

There is no duplicate input/output definition. The Python annotations are the authoring source. The generated AIR `defop` is the canonical source. The runtime validates against AIR, not against ambient Python type hints.

---

# The effect contract is still explicit

This model does **not** blur workflow/effect separation.

A workflow op still cannot call Slack. It can only emit:

```python
ctx.emit("acme/slack.post@1", params, cap="slack")
```

The kernel still:

```text
resolves effect op
loads params schema
canonicalizes params
checks workflow effects_emitted allowlist
checks cap grant type and constraints
checks policy
records open work
flushes journal
publishes to effect runtime
admits receipt
routes continuation to recorded origin
```

So the core AgentOS invariant stays intact: external I/O never hides behind a normal function call; it crosses the effect boundary and returns as a signed receipt. The current architecture is already built around that separation.  

---

# What about definitional locality?

This design gives locality at the right level.

For an effect, the local definition is:

```json
defop acme/slack.post@1:
  params
  receipt
  cap_type
  origin_scope
  execution_class
  impl module
  impl entrypoint
```

For a workflow, the local definition is:

```json
defop acme/order.step@1:
  state
  event
  context
  effects_emitted
  cap_slots
  determinism profile
  impl module
  impl entrypoint
```

For a pure function:

```json
defop acme/normalize_json@1:
  input
  output
  context
  impl module
  impl entrypoint
```

The module itself is only:

```json
defmodule acme/order_bundle@1:
  runtime
  bundle hash
  target
  lock/dependency metadata
```

That is the cleanest separation I see.

The operation has the semantic interface.

The module has the executable bytes.

The manifest decides which ops are active and how they are routed/bound.

---

# Why not just extend `defmodule` with `module_kind: "adapter"`?

You can do that as a stepping stone, but I would not make it the final architecture.

This shape:

```json
{
  "$kind": "defmodule",
  "module_kind": "adapter",
  "abi": {
    "effect": {
      "params": "...",
      "receipt": "..."
    }
  }
}
```

works for one effect per module. But it becomes clumsy when one Python bundle contains ten effects, three pure transforms, and two workflows. You either duplicate the same runtime bundle across many `defmodule`s, or you reinvent `exports` inside `defmodule`.

Once you add exports, you have basically discovered `defop`.

So the two viable clean designs are:

```text
Option A:
  defmodule = bundle with exports
  each export has op_kind/signature/impl

Option B:
  defmodule = bundle
  defop = named export with op_kind/signature/impl
```

I prefer **Option B** because ops become independently referenceable, routable, bindable, governable, and upgradable. It also avoids making one large `defmodule` node change every time you add or replace one operation.

---

# How upgrades work

This model is better for upgrades.

A workflow instance should pin:

```text
workflow_op_hash
module_hash
runtime profile
```

An open effect should pin:

```text
effect_op_hash
effect impl module_hash
entrypoint
params schema hash
receipt schema hash
```

Then a new manifest can change:

```text
acme/slack.post@1 -> new op hash
```

or:

```text
acme/order.step@1 -> new op hash
```

while old in-flight work still resolves to the old op hash.

This is cleaner than `adapter_id`, because `adapter_id` is not enough identity. You want content-addressed executable identity, not a string label.

Receipts should include something like:

```json
{
  "intent_hash": "...",
  "effect_op": "acme/slack.post@1",
  "effect_op_hash": "sha256:...",
  "executor": {
    "module": "acme/order_bundle@1",
    "module_hash": "sha256:...",
    "entrypoint": "orders.effects:post_to_slack"
  },
  "status": "ok",
  "payload_cbor": "...",
  "signature": "..."
}
```

The existing generic receipt envelope already carries origin identity, intent identity, effect kind, payload bytes, status, adapter id, cost, and signature; in this model, `adapter_id` should become an executor/op identity rather than a free string. 

---

# How the manifest would validate effects

In the current system, `manifest.effects` is the authoritative effect catalog, and `effect_bindings` maps external effect `kind` to `adapter_id`. 

In the new system:

```text
effect catalog = all active defops where op_kind = effect
```

Validation rules:

1. Every workflow `effects_emitted[]` must reference an active `defop` with `op_kind = effect`.
2. The effect op defines the params schema, receipt schema, cap type, origin scope, and execution class.
3. A workflow can only emit that op if its origin kind is allowed.
4. The cap slot type must match the effect op’s `cap_type`.
5. Policy can match either:

   * `effect_op`
   * `effect.kind`
   * `cap_type`
   * `origin_kind`
   * `origin_op`
6. The kernel canonicalizes params using the effect op’s params schema.
7. Receipt payloads are canonicalized using the effect op’s receipt schema.

This is exactly the same enforcement story as today, just with a better source of truth.

---

# Origin scopes should be cleaned up now

There is already a small schema smell: `defeffect.schema.json` uses `origin_scope = workflow | plan | both`, while policy origin matching uses `workflow | system | governance`.  

In the new `defop.effect.origin_scope`, I would make it explicit:

```json
"origin_scope": ["workflow", "system", "governance"]
```

No `both`.

No `plan`.

No overloading.

This also makes future policy extensions easier.

---

# Runtime profiles fit naturally

A `defmodule` carries the runtime:

```json
"runtime": {
  "kind": "python",
  "engine": "cpython",
  "artifact": { "hash": "sha256:..." },
  "target": "linux-x86_64-cp312"
}
```

A `defop` carries the execution mode:

```json
"impl": {
  "calling_convention": "sync_reducer"
}
```

and the role-specific profile:

```json
"workflow": {
  "determinism": "decision_log"
}
```

or:

```json
"effect": {
  "execution_class": "external_async"
}
```

So the same Python bundle can be invoked in different ways:

```text
workflow op -> sync reducer, no ambient I/O, decision-log or checked replay
pure op     -> sync function, no effects
effect op   -> async function, allowed to do external I/O after owner authorization
```

That is much cleaner than having separate Python module types for workflow and adapter.

---

# Packaging

A Python bundle can be:

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
    modules.air.json
    ops.air.json
```

The `defmodule` points at the bundle hash.

Each `defop` points at the module plus entrypoint.

This lets one bundle provide many ops without duplicating artifact hashes.

It also supports mixed language composition:

```text
Python workflow op -> Rust built-in effect op
WASM workflow op   -> Python effect op
Python pure op     -> WASM workflow helper
WASM workflow op   -> WASM nondeterministic effect op
```

The manifest just sees ops.

---

# Migration path

I would not try to flip the whole system at once.

## Phase 1: internal lowering

Keep accepting current AIR, but lower it internally:

```text
old defmodule workflow -> defmodule bundle + defop workflow
old defmodule pure     -> defmodule bundle + defop pure
old defeffect          -> defop effect without first-class impl, resolved through legacy adapter registry
```

The kernel internally uses the `defop` index.

## Phase 2: add new manifest fields

Add:

```json
"ops": [],
"op_bindings": {}
```

Keep:

```json
"modules": [],
"effects": [],
"effect_bindings": [],
"module_bindings": {}
```

as compatibility fields.

## Phase 3: Python SDK emits new-style AIR

The Python SDK should emit:

```text
defschema
defmodule bundle
defop workflow/effect/pure
```

No `defeffect`.

No `defadapter`.

## Phase 4: built-ins become ops

Convert built-in effects, cap enforcers, workspace workflows, and introspection effects to canonical `defop`s.

## Phase 5: deprecate old forms

Eventually:

```text
defeffect       -> removed or authoring sugar
effect_bindings -> removed
module_bindings -> replaced by op_bindings
```

---

# The final mental model

The ideal model is:

```text
World
  Manifest
    Schemas
    Modules = executable bundles
    Ops     = typed executable entrypoints
      workflow ops
      pure ops
      effect ops
      cap enforcer ops
    Caps
    Policies
    Routing
    Op bindings
```

Then the kernel asks only:

```text
What op is being invoked?
What is its role?
What schema validates its input/output?
What module/runtime implements it?
What authority gates it?
What journal records pin it?
```

That is the unification you are looking for.

---

# My strongest recommendation

Do **not** add `defadapter` as a canonical AIR form.

Do **not** extend `defmodule` with a simple `adapter` kind as the final design.

Instead:

```text
Keep defmodule, but redefine it as executable bundle metadata.
Add defop as the one canonical typed executable entrypoint.
Fold defeffect into defop(op_kind = "effect").
Replace effect_bindings.adapter_id with direct op/module implementation refs.
Replace module_bindings with op_bindings.
```

That gives you locality, removes duplicate I/O definitions, supports Python/Pydantic authoring cleanly, supports multi-entrypoint bundles, keeps the effect receipt model, and makes upgrades/version pinning much more coherent.
