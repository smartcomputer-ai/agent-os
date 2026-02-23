# Spec: Replace Plans With Workflows

## 0. Summary

We remove the **Plan interpreter** (defplan, step DAG execution, plan ops like `await_receipt`, etc.) and replace it with **Workflows**: deterministic WASM modules that implement orchestration as event-driven state machines.

A Workflow module:

* runs deterministically (like reducers)
* cannot perform side effects directly
* **emits effect intents**
* **consumes receipt events** (and other domain events)
* persists its own state (per workflow instance)
* is started/routed via manifest wiring (triggers/subscriptions)

The kernel:

* continues to enforce “no side effects except via effects”
* continues to journal all events/effect-intents/receipts
* continues to snapshot state for fast replay/forking
* continues to apply caps/policies at effect enqueue time
* gains a lightweight “workflow runtime” (per-instance, fuel-limited)

---

## 1. Goals and non-goals

### Goals

1. **Remove plans entirely**

   * no DAG plan IR
   * no plan-step interpreter
   * no plan ops DSL

2. **Keep solid-state semantics**

   * deterministic replay from journal
   * snapshots for fast recovery
   * easy forking (clone journal/snapshot roots)

3. **Improve composition and ergonomics**

   * orchestration written in Rust
   * use Cargo for reuse/imports
   * no AIR composition gymnastics for workflow logic

4. **Keep strict side-effect boundary**

   * only effect adapters can touch the world
   * workflows emit effect intents; receipts arrive as events

5. **Support scale**

   * many workflow instances concurrently (keyed)
   * parallelism via multiple instances and/or fanout

### Non-goals (initial version)

* Static analyzability of orchestration graphs (replaced with metadata + traces)
* Automatic migration of arbitrary old plan state to workflows (we’ll define a migration path)
* Rich distributed scheduling (we’ll rely on current worker/executor model)

---

## 2. Conceptual model

### 2.1 Entities

**Reducer module (existing):**

* deterministic state transition function
* consumes events
* updates application state
* may emit limited effects (optional; depends on your current rules)

**Workflow module (new):**

* deterministic orchestration state machine
* consumes events + receipts
* maintains per-instance workflow state (own storage)
* emits effect intents (often broader than reducers)
* optionally emits domain events to reducers

**Effect adapter (existing):**

* executes side effect in host environment
* returns a receipt payload as an event (journaled)
* may include references/blobs as receipts require

### 2.2 Workflow instances and keying

A workflow is not “one global thing.” It has **instances** keyed by an **InstanceKey** (string/bytes). This maps cleanly to your Cells model.

* `workflow_id`: identifies the workflow module (content-addressed or logical name)
* `instance_key`: identifies a particular long-running instance (e.g. “PR#1234”, “user:42”, “deploy:abc”)

Each instance has durable workflow state:

* `status`: Running | Waiting | Completed | Failed | Suspended
* `data`: arbitrary deterministic bytes (CBOR/MessagePack) validated by schema
* `inflight`: correlation IDs for awaited receipts (optional)
* `version`: workflow state schema version (optional)

---

## 3. Kernel responsibilities

### 3.1 Event routing into workflows

The kernel must decide which workflows receive which events. This is equivalent to “plan triggers.”

Add a **Workflow Routing Table** to the manifest:

* subscriptions by event kind/type
* optional filters (origin, tags)
* mapping from event payload → instance_key (key derivation)
* policy for creating missing instances (auto-create or ignore)

**Routing decision must be deterministic**.

#### Routing rules

For each incoming event `E`:

1. Determine candidate workflow subscriptions `S` that match `E.type`.
2. For each subscription, compute `instance_key = key_fn(E)` (pure, deterministic).
3. If instance exists, deliver event to it.
4. If missing:

   * if subscription `create_if_missing=true`: create workflow instance with initial state and deliver event
   * else ignore.

### 3.2 Workflow execution model

Workflows execute on event delivery in a deterministic WASM VM.

Each delivery is a “tick”:

* input: one event (domain event or receipt)
* state: workflow instance state
* output:

  * updated workflow state
  * zero or more effect intents
  * zero or more emitted domain events (optional)
  * logs/trace (optional)

**Determinism**: Given same prior state and same input event bytes, output must be identical.

### 3.3 Fuel and resource limits

Since workflows are Turing-complete, enforce deterministic limits:

* `wasm_fuel_per_tick` (instruction budget)
* `max_effects_emitted_per_tick`
* `max_events_emitted_per_tick`
* `max_state_bytes` per workflow instance
* `max_inflight_receipts`

On limit exceed:

* emit a deterministic “WorkflowFault” event
* mark instance Failed (or Suspended depending on policy)

### 3.4 Await semantics (receipts)

Plans had built-in await ops. Workflows implement await by state:

Workflow can emit an effect intent with a `correlation_id`.
Receipt events include the same correlation_id.
Workflow stores awaited IDs in state.

When a receipt arrives:

* route receipt event to the workflow instance (via correlation or normal routing)
* workflow transitions state accordingly.

**Routing receipts**
Two strategies (pick one or support both):

1. **Correlation-based routing (recommended)**

   * effect intent includes `workflow_id` + `instance_key` + `correlation_id`
   * adapter receipt event includes same triplet
   * kernel routes receipt directly to the owning workflow instance

2. **Event-type routing**

   * receipt event is just another event type
   * subscriptions route it
   * instance_key derived from receipt payload

Correlation-based is simpler and less error-prone.

### 3.5 Effect intent enqueue and policy/caps enforcement

Before enqueueing an effect intent emitted by a workflow:

* validate effect params against effect schema
* enforce caps binding (if required)
* evaluate policies (origin=workflow)
* if denied: emit a deterministic “EffectDenied” receipt event (or fault event) routed back to the workflow

This keeps the effect boundary crisp.

### 3.6 Journaling and snapshots

All of the following must be journaled deterministically:

* input events
* workflow state update (or at least resulting state hash)
* effect intents
* receipts
* faults/denials

Snapshots must include:

* reducer state
* workflow instance states (or a merkelized store and root hash)

Forking works unchanged: fork = copy snapshot root + journal tail pointer.

---

## 4. Data model (new definitions)

Even if you keep AIR for schemas/effects/policy, plans are removed. Introduce `defworkflow` (or treat it as a module kind).

### 4.1 defworkflow (conceptual)

Fields:

* `workflow_id`
* `wasm_module_ref`
* `input_event_types`: list
* `state_schema_ref`
* `effects_emitted`: allowlist (effect kinds)
* `max_*` overrides (optional)
* `metadata`: human-readable summary, tags, owners

### 4.2 workflow instance state (stored)

* `workflow_id`
* `instance_key`
* `status`
* `state_bytes` (canonical CBOR validated by `state_schema_ref`)
* `inflight` map: correlation_id → {effect_kind, issued_at, timeout_at?}
* `last_event_seq` (optional for debugging)

### 4.3 routing table entries

* `workflow_id`
* `event_type`
* `key_fn`: deterministic expression / compiled wasm helper / predeclared mapping
* `create_if_missing`: bool
* `init_state`: optional deterministic constant or `init_fn` (pure)

**Important**: to avoid reinventing a DSL, prefer `key_fn` to be:

* either “pick field X from payload”
* or a pure WASM helper function inside the workflow module (invoked deterministically)

---

## 5. Workflow module API (WASM ABI)

Define a stable ABI that workflow wasm modules implement.

### 5.1 Required exports

1. `workflow_on_event(ctx, state, event) -> (new_state, outputs)`

* called for each delivered event

2. `workflow_init(ctx, init_args) -> state` (optional)

* called on auto-create

### 5.2 Context available to workflow code (no side effects)

* `now_logical`: logical time or journal index (deterministic)
* `world_id`
* `workflow_id`
* `instance_key`
* `event_seq` / journal position
* optional: read-only access to selected reducer state via deterministic snapshots (careful; see below)

### 5.3 Outputs structure

* `effects`: list of effect intents
* `events`: list of domain events (to be fed into reducer pipeline)
* `logs`: optional structured debug traces
* `metrics`: optional counters

### 5.4 Effect intent shape

* `effect_kind`
* `params` (typed)
* `correlation_id` (required if expecting receipt)
* optional: `timeout_at` or `ttl`
* optional: `cap_ref`/binding references

---

## 6. Interaction with reducers (how workflows cause domain state changes)

Two patterns:

### Pattern A: workflow emits domain events (recommended)

Workflow emits an event like `OrderApproved{...}`; reducer consumes it and updates state.

Pros:

* keeps reducers authoritative for domain state
* consistent event-sourced model
* easy replay

Cons:

* more event types

### Pattern B: workflow directly invokes reducers (not recommended initially)

Workflow calls reducer functions via ABI.

Pros:

* fewer events

Cons:

* blur of boundaries, harder auditing, less uniform journaling

**Recommendation**: Use Pattern A initially.

---

## 7. Time, timers, retries, and “sleep”

Plans often did “retry with backoff” or “sleep then continue.” In workflows:

### 7.1 Timers as effects

Introduce an effect kind `timer.schedule`:

* adapter simply waits and emits a receipt event at due time
* receipt routed to workflow instance

This preserves determinism (time progression is external, receipts are journaled).

### 7.2 Retries

Workflow implements retries by state:

* on receipt failure, compute next attempt count and schedule next effect (or timer + effect)
* store attempt metadata in workflow state

---

## 8. Concurrency and fanout

### 8.1 Per-instance single-threaded determinism

Within an instance, events must be processed in a deterministic order.
Simplest: serialize delivery per instance.

### 8.2 Fanout

Workflows can create child instances by emitting a “start workflow instance” event or calling a kernel primitive:

* `workflow.spawn(workflow_id, instance_key, init_payload)` as an effect-like kernel op (deterministic and journaled)
* child workflows proceed independently

### 8.3 Join

Join is “await receipts/events from child workflows”:

* child emits completion events; parent subscribes/awaits them

---

## 9. Governance and upgrades

Since you noted plans rarely change alone, define **Bundle Upgrades** as the unit of change.

### 9.1 Bundle

A bundle contains:

* workflow wasm modules
* reducer wasm modules
* schemas
* effect defs
* policies/caps
* manifest routing wiring

### 9.2 Upgrade event

Introduce `BundleActivate{bundle_hash}` event:

* after governance approval
* atomically switches active bundle
* journaled

### 9.3 Compatibility rules

You need one of:

1. **Additive-only event evolution** (recommended baseline)
2. **Versioned event types** with backward handlers
3. **Deterministic upcasters** (V1→V2 at replay)

Workflow state schema evolution:

* state contains version tag
* workflow code handles older versions or a deterministic migration step on first tick after upgrade

---

## 10. Observability: replacing “plan graph visibility”

With plans gone, visibility comes from:

1. **Workflow metadata**

* declared effect allowlist
* human summary
* tags (“deploy”, “coding-agent”, “billing”)

2. **Structured traces**

* every tick logs: input event, state hash before/after, emitted effects, correlation IDs
* store as journaled debug records or derivable from events

3. **Execution graph reconstruction tool**

* offline/online tool that builds a graph from:

  * workflow instance ticks
  * effect correlation edges
  * receipt edges
  * spawn edges

This gives you “what happened” and “what it does” without static DAGs.

---

## 11. Migration plan (from plans to workflows)

### 11.1 Phase 1: parallel support

* keep plans temporarily
* add workflows runtime
* allow new systems to use workflows
* implement receipt routing with correlation IDs (works for both)

### 11.2 Phase 2: plan-to-workflow port

For each plan:

* identify instance keying (what makes a run distinct?)
* create workflow state machine equivalent
* map plan steps to workflow states
* replace plan triggers with workflow routing table

### 11.3 Phase 3: remove plan interpreter

* disable loading of defplan
* remove plan scheduling/execution components
* retain any shared pieces (effect queue, policy/caps, schemas)

---

## 12. Security and policy model updates

### 12.1 Origin kinds

Policies currently might distinguish plan vs reducer. Replace with:

* `origin_kind: reducer | workflow | governance | system`

### 12.2 Default deny posture

Recommended defaults:

* reducers: allow only “internal” effects (or none)
* workflows: allow a declared allowlist, subject to caps/policy
* governance/system: special-case

---

## 13. Minimal kernel surface area (implementation checklist)

To ship v1 workflows without plans, kernel needs:

1. Manifest routing table for event→workflow instance delivery
2. Storage for workflow instance state (snapshot-integrated)
3. WASM execution for workflow modules with fuel limits
4. Effect intent enqueue pipeline (already exists)
5. Correlation-based receipt routing
6. Deterministic fault handling and journaling for workflow errors
7. Bundle activation mechanism (optional but strongly recommended)

---

## 14. Example: a typical workflow

**Use case**: “Code review workflow”

* event: `PullRequestOpened{repo, pr_number}`
* steps:

  1. emit `git.fetch_pr` effect → receipt includes diff
  2. emit `llm.review_diff` effect → receipt includes summary
  3. emit `github.comment` effect → receipt confirms posted
  4. emit domain event `PRReviewed{...}` to reducer

In workflow state:

* `phase = Fetching | Reviewing | Commenting | Done`
* store `diff_ref`, `review_text`, `attempts`, `inflight_correlation_id`

No plan DAG required; all composition in Rust.

---

## 15. Key design choices you should decide explicitly

These are the “fork in the road” points that materially change complexity:

1. **Receipt routing**: correlation-based vs event-type routing
   (I’d do correlation-based.)

2. **Do workflows get read access to reducer state?**

   * If yes: expose a deterministic snapshot-read API (read-only) with strict limits.
   * If no: workflows rely only on events and their own state (simpler, purer).

3. **Do reducers emit effects at all?**

   * If you want stricter separation: reducers emit *no* external effects; workflows do all orchestration and I/O.
   * If you keep reducer effects: still enforce allowlist and caps/policy.

4. **Upgrade granularity**: module-level upgrades vs bundle-level atomic upgrade
   Given your observation, bundle-level is usually the right primitive.
