
Right now AIR is trying to be both:

1. a **public world/API contract**, and
2. an **implementation/deployment manifest**.

That is the source of a lot of the friction. I think you should split those apart.

My recommended direction:

```text
AIR v3 / manifest:
  public, portable, content-addressed API contract
  schemas, workflows, effects, routing, queries/views
  no implementation pointers
  no defmodule
  no impl.module
  no entrypoint
  probably no defsecret binding_id

Runner/deployment bindings:
  private/operator-controlled execution config
  workflow/effect route -> runner pool
  trust mode, artifact digest, signer, secrets, limits, tenant policy
  not part of the portable world contract

World:
  manifest hash
  journal
  CAS state roots / deltas / decisions / receipts
```

So yes: **strip implementation out of AIR. Do not drop AIR.**

The current docs describe AIR as a unified control plane for schemas, modules, workflows, effects, secrets, manifests, and routing, with workflow/effect definitions pointing to modules and entrypoints.   That made sense for the deterministic WASM-first design. But if you want external workflow runners and external effect workers, keeping `defmodule` and `impl` in the public manifest starts to fight the architecture.

## The better mental model

I would reframe AOS as:

> **A manifest-governed world log with typed APIs, CAS-backed state, and pluggable execution.**

Not:

> “A node that runs WASM workflows from a manifest.”

That means AIR becomes closer to an IDL plus world contract:

```text
What messages/events exist?
What workflows exist?
What state shape does a workflow claim?
What effects may a workflow emit?
What receipts come back?
How are events routed?
What queries/views are exposed?
What is the current version of this contract?
```

It should not answer:

```text
Which binary runs this?
Which container image?
Which entrypoint?
Which Python/Rust/TS runtime?
Which secret resolver?
Which worker pool?
Which trust policy?
Which deployment target?
```

Those are deployment concerns.

## What AIR v3 could look like

Something like this:

```json
{
  "$kind": "manifest",
  "air_version": "3",

  "schemas": [
    {
      "name": "com.acme/OrderEvent@1",
      "hash": "sha256:..."
    },
    {
      "name": "com.acme/OrderState@1",
      "hash": "sha256:..."
    },
    {
      "name": "com.acme/ChargeParams@1",
      "hash": "sha256:..."
    },
    {
      "name": "com.acme/ChargeReceipt@1",
      "hash": "sha256:..."
    }
  ],

  "workflows": [
    {
      "name": "com.acme/order@1",
      "state": "com.acme/OrderState@1",
      "event": "com.acme/OrderEvent@1",
      "key_schema": "com.acme/OrderId@1",
      "effects_emitted": [
        "com.acme/payment.charge@1"
      ],
      "queries": [
        {
          "name": "get",
          "params": "com.acme/OrderQuery@1",
          "result": "com.acme/OrderView@1"
        }
      ]
    }
  ],

  "effects": [
    {
      "name": "com.acme/payment.charge@1",
      "params": "com.acme/ChargeParams@1",
      "receipt": "com.acme/ChargeReceipt@1"
    }
  ],

  "routing": {
    "subscriptions": [
      {
        "event": "com.acme/OrderEvent@1",
        "workflow": "com.acme/order@1",
        "key_field": "order_id"
      }
    ]
  }
}
```

No `defmodule`. No `impl`. No `entrypoint`.

Compare that to the current schemas: `defworkflow` currently requires `impl`, and `defeffect` currently requires `impl`; `defmodule` exists specifically to declare runtime/artifact identity.    Those are the parts I would remove from the public world manifest.

## Where implementation goes instead

Implementation belongs in a **runner binding registry**, which can be node-local, cluster-local, or managed by a hosting control plane.

Example:

```yaml
workflow_bindings:
  - workflow: com.acme/order@1
    contract_hash: sha256:...
    runner_pool: order-rust-workers
    protocol: aos.workflow.v1

    trust:
      mode: decision_log
      signer: runner/acme-orders
      artifact_digest: ghcr.io/acme/order-runner@sha256:...
      attestation_policy: optional

    state:
      strategy: cas_delta
      cas_scope: world
      hot_cache: sticky

    limits:
      max_concurrency: 512
      timeout_ms: 10000

effect_bindings:
  - effect: com.acme/payment.charge@1
    contract_hash: sha256:...
    runner_pool: stripe-workers
    protocol: aos.effect.v1

    trust:
      signer: runner/acme-stripe
      artifact_digest: ghcr.io/acme/stripe-adapter@sha256:...

    secrets:
      allow:
        - stripe/live_key

    limits:
      max_concurrency: 64
      timeout_ms: 30000
      max_retries: 5
```

Workers can advertise what they support:

```json
{
  "worker_id": "orders-worker-17",
  "protocols": ["aos.workflow.v1"],
  "supports": [
    {
      "kind": "workflow",
      "name": "com.acme/order@1",
      "contract_hash": "sha256:..."
    }
  ],
  "capacity": {
    "max_inflight": 64
  },
  "signer": "runner/acme-orders",
  "artifact_digest": "ghcr.io/acme/order-runner@sha256:..."
}
```

But worker self-advertisement alone should not establish authority. The coordinator should only lease tasks when the advertised capability matches an operator-approved binding. Otherwise any worker could claim to execute `payment.charge@1`.

So the split is:

```text
AIR:
  “This workflow/effect exists and has this typed contract.”

Runner registration:
  “This process claims it can execute that contract.”

Binding policy:
  “This process/signature/artifact is allowed to execute that contract for this world.”
```

## Should `strict`, `checked`, and `decision_log` stay in AIR?

I would move them mostly out of AIR.

The current workflow schema already has `determinism: strict | checked | decision_log`.  In the old model, that made sense because AIR pointed directly to the implementation artifact. In the new model, the manifest is an API contract, so determinism is not really a property of the API. It is a property of the **runner binding plus admission protocol**.

I would put this in the binding:

```yaml
trust:
  mode: strict | checked | decision_log
```

Then every committed workflow decision records what actually happened:

```json
{
  "kind": "WorkflowAdvanced",
  "workflow": "com.acme/order@1",
  "contract_hash": "sha256:...",
  "runner_binding": "order-rust-workers@2026-05-04",
  "trust_mode": "decision_log",
  "runner_signer": "runner/acme-orders",
  "prior_state_ref": "sha256:...",
  "state_commitment": {
    "kind": "cas_delta",
    "delta_ref": "sha256:...",
    "new_root": "sha256:..."
  },
  "emitted_events": [],
  "emitted_effects": [],
  "decision_hash": "sha256:..."
}
```

That gives you auditability without pretending the public manifest proves implementation discipline.

One nuance: a manifest may optionally declare a **required minimum admission mode** if users need contractual guarantees. For example:

```json
{
  "name": "com.acme/regulatory_approval@1",
  "state": "com.acme/ApprovalState@1",
  "event": "com.acme/ApprovalEvent@1",
  "effects_emitted": [],
  "required_admission": "strict"
}
```

But I would not put this in the first v3 unless you already have a use case. Start with the binding controlling it.

## The state issue: you are right, but there is one non-negotiable invariant

I agree that workers should not shuttle large workflow state back to the coordinator.

The coordinator should not receive this after every event:

```text
new_state_bytes: 50 MB
```

It should receive this:

```text
new_state_ref: sha256:...
```

or this:

```text
state_delta_ref: sha256:...
new_state_root: sha256:...
```

The worker should write state directly to CAS, and the coordinator should only commit the root or delta reference.

That fits the existing AOS backend model well: the backend contract already separates the journal from CAS/blob storage, and CAS is logically just `hash -> bytes` with reads verified against the requested hash.  The current workflow design also already has the right machinery in spirit: keyed workflow state uses a content-addressed `CellIndex`, with hot cache, dirty delta layer, CAS spill, and snapshot roots. 

The non-negotiable part is this:

> The coordinator does not need state bytes, but the world journal needs a durable commitment to the state transition before progress is considered committed.

That commitment can be small. It does not need to be full state.

Possible state strategies:

```text
state_root_each_step:
  worker writes changed CAS nodes and returns new root hash
  journal commits new root
  simple and strong

cas_delta:
  worker writes a compact state delta to CAS
  journal commits delta ref
  snapshots periodically materialize full roots
  good for large state

derived_strict:
  journal records events/effects/receipts
  deterministic runner can recompute state
  snapshots are periodic optimization
  strongest when runner is actually deterministic

decision_log:
  journal records workflow decisions and state commitments
  replay applies decisions rather than trusting code re-execution
  best for arbitrary runtimes

hot_ephemeral:
  worker keeps state in memory and only occasionally checkpoints
  fast, but not enough for committed AOS progress unless the intervening
  transition evidence is recoverable from journal
```

The dangerous version is:

```text
worker updates hot state
coordinator journals only “event processed”
worker does not write root/delta/decision evidence
effect is published
worker crashes
```

That breaks recovery. The coordinator cannot prove what state transition happened or why the effect was emitted.

The safe version is:

```text
1. coordinator leases WorkflowTask to worker
2. worker loads hot state or CAS state
3. worker computes transition
4. worker writes changed state/delta objects to CAS
5. worker returns only refs + emitted events/effects
6. coordinator verifies refs exist and match contract expectations
7. coordinator journals WorkflowAdvanced
8. only after durable flush does coordinator publish effects
```

This preserves the effect invariant already central to AOS: external work is recorded durably before execution, and adapters/workers do not mutate world state directly. 

## Hot state and queries through workers

I also agree that workflow state should be queryable through the coordinator while still being served by the worker.

That suggests a separate **query contract** in AIR:

```json
{
  "name": "com.acme/order@1",
  "state": "com.acme/OrderState@1",
  "event": "com.acme/OrderEvent@1",
  "effects_emitted": ["com.acme/payment.charge@1"],
  "queries": [
    {
      "name": "get",
      "params": "com.acme/OrderQuery@1",
      "result": "com.acme/OrderView@1"
    },
    {
      "name": "list_open",
      "params": "com.acme/ListOpenOrders@1",
      "result": "com.acme/OpenOrderPage@1"
    }
  ]
}
```

Then the control plane can do:

```text
client -> coordinator -> active workflow runner -> hot state query result
```

The query response should include a consistency token:

```json
{
  "world_seq": 12345,
  "manifest_hash": "sha256:...",
  "workflow": "com.acme/order@1",
  "state_ref": "sha256:...",
  "result": "..."
}
```

If the hot worker is unavailable, the coordinator can either:

```text
serve from last committed CAS root / snapshot
hydrate another query runner from CAS
replay journal tail into a read-only runner
return “hot state unavailable”
```

This is a good split:

```text
coordinator:
  owns routing, journal, leases, open effects, state commitments, audit

worker:
  owns hot state, language runtime, query execution, CAS writes

CAS:
  owns durable bytes

journal:
  owns ordered facts and state-root/delta commitments
```

## What the coordinator must still own

Even if workers run everything, the coordinator still needs to own a few things, otherwise AOS collapses into “some workers and a database.”

The coordinator should own:

```text
manifest hash and active contract
journal sequence
event admission
routing
workflow task leases
effect intent admission
open work
receipt admission
state root/delta commitments
CAS root pinning
runner binding authorization
query consistency tokens
world snapshots/checkpoints
```

Workers may execute logic and manage hot state, but they should not be able to independently decide that the world advanced.

That is the line that keeps AOS coherent.

## What AIR should keep

I would keep these parts:

### `defschema`

This is probably the most important part of AIR. Without schemas and canonical typed values, you lose stable hashing, typed receipts, event compatibility, introspection, and agent-editable contracts. The current `defschema` is already simple: name plus type expression. 

### `defworkflow`, but contract-only

Keep:

```text
name
state schema
event schema
key schema
effects_emitted
queries/views
annotations maybe
```

Remove:

```text
impl
module
entrypoint
runtime
artifact
probably determinism
```

### `defeffect`, but contract-only

Keep:

```text
name
params schema
receipt schema
optional stream schema
```

Remove:

```text
impl
module
entrypoint
runtime class
adapter route
secret binding
```

### `manifest.routing`

Keep routing in AIR. Routing is part of the world’s public behavior. The current manifest already maps event schemas to workflows, with `key_field` for keyed workflows.  That belongs in the contract.

### canonicalization and hashing

Keep this aggressively. AIR’s canonical CBOR / schema-directed normalization is a core differentiator. The current AIR spec already says event payloads, effect params, receipts, workflow state, and context envelopes all follow the same schema-directed normalization path before journal append or hashing. 

That is extremely valuable even if execution is external.

## What AIR should remove

I would remove these from public AIR v3:

```text
defmodule
impl.module
impl.entrypoint
runtime artifact identity
secret binding_id
executor identity
adapter route
deployment target
trust/determinism mode, unless optional as a required policy
```

`defsecret` is especially suspect in a contract-only manifest. The current `defsecret` contains an opaque node-local `binding_id`, such as an environment variable binding.  That is not portable world API. I would move secret bindings entirely into deployment config.

The manifest can still reference abstract credential/resource names through schemas, for example:

```json
{
  "api_key": {
    "ref": "sys/SecretRef@1"
  }
}
```

But the mapping from `stripe/live_key` to Kubernetes Secret, Vault path, environment variable, or cloud secret manager should not be in AIR.

## Are you moving away from the original vision?

You are moving away from one part of it:

```text
“the manifest contains deterministic implementation artifacts”
```

But you are not moving away from the stronger and more useful part:

```text
“a world is a typed, governed, event-sourced, content-addressed system
with explicit effects and receipts”
```

The original overview says AgentOS is about a deterministic event-sourced core, typed AIR, explicit effects, signed receipts, portability, governance, and a minimal trusted base.  A contract-only AIR still supports that. In fact, it may support it better because the trusted core becomes smaller.

The tradeoff is that there are now two levels of replay:

```text
contract replay:
  replay journal + decisions + state refs + receipts
  works across arbitrary runners

implementation replay:
  rerun workflow code and verify it produced the same decisions
  only available for strict/checked runners
```

That is okay, as long as AOS is honest about it.

The new invariant should be:

> Same manifest, same journal, same CAS commitments, and same receipts reconstruct the same world state.

Not necessarily:

> Same manifest and same implementation module can always be re-executed to produce the same state.

The latter is only true in strict mode.

## How this compares to Temporal

Temporal is excellent at durable execution. Its model is that workflows are code in a general-purpose language; a Workflow Execution records commands and events in an Event History, and that history is the source of truth for replay. Temporal re-runs workflow code and uses the recorded history to guide it back to the prior state. ([Temporal Docs][1])

Temporal workers poll task queues by synchronous RPC; polling gives backpressure, load balancing, task routing, and no need for workers to expose inbound ports. ([Temporal Docs][2]) The Temporal server has a History Service that persists workflow state/history and then adds tasks to task queues, while a Matching Service hosts task queues for dispatch. ([Temporal Docs][3])

So Temporal already gives you:

```text
durable workflow execution
workers
task queues
replay
retries
timeouts
activity execution
production maturity
multi-language SDKs
```

But Temporal does not give you, out of the box:

```text
AIR-like public API manifest
content-addressed world contracts
agent-editable schema/effect/routing catalog
CAS-rooted world export/fork semantics
explicit signed effect receipts as first-class world records
manifest governance loop as world data
portable “manifest + journal + CAS” world identity
```

You can build those on top of Temporal, but then AOS becomes an application framework layered over Temporal rather than its own runtime.

Also, Temporal has practical payload/history limits. Its self-hosted defaults page documents gRPC and transaction size limits, payload warning/error thresholds, history size/count limits, and limits around pending operations. ([Temporal Docs][4]) That does not mean Temporal is bad; it means your instinct to keep large state in CAS and store references is correct even if you use Temporal.

## How this compares to Inngest

Inngest is closer to the “standard language runtime, durable steps, no custom workflow runtime” direction. Its docs describe functions written in TypeScript, Python, or Go that run on your own compute, with Inngest handling durability, retries, state persistence, and flow control. ([inngest.com][5]) Inngest’s step model persists step outputs, memoizes successful steps, and re-enters functions while skipping completed work. ([inngest.com][5])

Inngest self-hosting also has a queue, executor, state store, and database split: the queue handles fairness/flow control, the executor runs functions and writes incremental run state, and the state store persists pending/ongoing run data. ([inngest.com][6])

So Inngest is closer to:

```text
run code anywhere
persist step outputs
memoize completed steps
do not force deterministic replay in the same way Temporal does
```

AOS could borrow from that, but again, Inngest does not give you AIR/CAS/world-governance as the center of the model.

## Should you drop AOS and just use Temporal?

Use Temporal if the product goal is mainly:

```text
production-grade durable execution
multi-language workflow SDKs
worker fleets
retries/timeouts
battle-tested operations
shipping soon
```

Do not rebuild Temporal just to have task queues and durable workflows. Temporal already exists and is mature.

Keep building AOS if the product goal is:

```text
worlds as portable objects
manifest-governed APIs
typed schema/effect contracts
CAS-backed state and artifacts
agent-readable/agent-editable control plane
signed receipts and provenance
fork/replay/export of world state
coordination between humans and agents around contract evolution
```

The strongest argument for AOS is **not** “we can make a better Temporal.” The strongest argument is:

> Temporal is durable execution. AOS is a manifest-governed world substrate.

If that distinction matters, keep AOS.

## A realistic hybrid option

You could also build AOS on top of Temporal:

```text
AIR manifest:
  AOS contract layer

Temporal workflows:
  coordinate world/event processing

Temporal activities:
  execute effects and runner tasks

CAS:
  stores state roots, deltas, blobs, receipts

AOS journal:
  either separate, or represented through Temporal history + external audit log
```

Pros:

```text
much less infrastructure to build
mature worker/task/retry engine
multi-language SDKs immediately
good production story
```

Cons:

```text
AOS world log no longer fully owns the execution model
AIR governance becomes a layer above Temporal, not the substrate
Temporal history/payload model shapes your design
harder to make “manifest + journal + CAS == world” the primitive
harder to support arbitrary decision-log semantics cleanly
effect receipts may feel bolted on rather than native
```

I would only choose this if speed to production matters more than owning the world substrate.

## The architecture I would choose

I would build AOS Core as a small coordinator with a contract-only AIR.

```text
AOS Core:
  AIR v3 loader / canonicalizer
  manifest governance
  world journal
  CAS root/delta pinning
  routing
  task leasing
  effect admission
  receipt admission
  query proxy
  snapshot/checkpoint manager

Workflow runners:
  Rust first
  arbitrary runtimes later
  hot state cache
  CAS writes
  query serving
  workflow decisions

Effect workers:
  arbitrary runtimes
  CAS writes for large payloads
  signed receipts
  stream frames
```

The key journal records become:

```text
ManifestChanged
DomainEventAdmitted
WorkflowTaskLeased
WorkflowAdvanced
EffectIntentOpened
EffectStreamFrameAdmitted
EffectReceiptAdmitted
StateCheckpointed
SnapshotCreated
RunnerBindingObserved
```

The core protocol:

```text
WorkflowTask:
  world_id
  manifest_hash
  workflow name
  contract hash
  key
  input event or receipt
  prior state root/ref
  journal height
  task token

WorkflowDecision:
  task token
  prior state root/ref
  new state root/ref or delta ref
  emitted domain events
  emitted effect intents
  query/view invalidations maybe
  runner identity
  signature
```

For effects:

```text
EffectTask:
  world_id
  manifest_hash
  effect name
  contract hash
  params ref/cbor
  intent hash
  task token

EffectReceipt:
  task token
  intent hash
  receipt payload ref/cbor
  status
  cost/timing
  runner identity
  signature
```

The coordinator validates contracts and commits refs. It does not need to know or run the implementation.

## The most important invariant

I would make this the central AOS rule:

> Workers may execute logic and write CAS objects, but only the coordinator can publish world roots, admit effects, admit receipts, and advance the journal.

This allows external runtimes without giving up the world.

## What to build first

I would sequence it like this:

```text
1. Define AIR v3 contract-only:
   defschema, defworkflow, defeffect, routing, queries
   remove defmodule, impl, entrypoint, defsecret binding

2. Define WorkflowDecision and EffectReceipt protocols:
   refs, not large bytes
   CAS-first payload/state handling
   runner signatures
   contract hashes

3. Build Rust workflow runner SDK:
   reducer-style first
   hot state cache
   CAS state root/delta writes
   query handlers

4. Build coordinator task service:
   long-poll or stream workers
   leases
   binding registry
   state-root commitment
   post-flush effect publication

5. Add effect worker SDK:
   Rust first
   then TypeScript/Python

6. Add strict/checked verification later:
   deterministic WASM can become one runner type
   not the foundation of the entire architecture
```

## Bottom line

I would **not** drop AIR.

I would drop the parts of AIR that make it an implementation manifest.

The new design should be:

```text
AIR:
  typed world/API contract

Runner bindings:
  implementation and trust

Journal:
  ordered facts and decisions

CAS:
  state, deltas, blobs, payloads, receipts

Coordinator:
  authority over admission and world advancement

Workers:
  execution, hot state, queries, CAS writes
```

That keeps the original vision where it matters, but removes the brittle assumption that the world manifest must know how every workflow and effect is implemented. It also gives you a path to arbitrary runtimes without turning AOS into an underpowered Temporal clone.

[1]: https://docs.temporal.io/workflows "Temporal Workflow | Temporal Platform Documentation"
[2]: https://docs.temporal.io/task-queue "Task Queues | Temporal Platform Documentation"
[3]: https://docs.temporal.io/temporal-service/temporal-server "Temporal Server | Temporal Platform Documentation"
[4]: https://docs.temporal.io/self-hosted-guide/defaults "Self-hosted Temporal Service defaults | Temporal Platform Documentation"
[5]: https://www.inngest.com/docs/learn/how-functions-are-executed "How Inngest functions are executed: Durable Execution - Inngest Documentation"
[6]: https://www.inngest.com/docs/self-hosting "Self-hosting - Inngest Documentation"
