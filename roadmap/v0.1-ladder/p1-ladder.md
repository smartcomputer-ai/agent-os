# Example Ladder

How to start showing what the kernel can do?

Below is a concrete, staged plan to (1) level‑up the runtime, (2) ship a credible v1.0, and (3) demonstrate it with a ladder of tiny “test apps” that get incrementally more complex. I’ve grounded the recommendations in your current model (worlds, deterministic stepper, AIR, reducers, plans, effects, receipts, caps/policy) and the v1 scopes you’ve defined.   

---

## North‑star alignment (but sequence matters)

* **North star:** a running world that can be modified by an LLM‑based agent via the constitutional loop (propose → shadow → approve → apply). Keep this *design‑time* path on the same deterministic substrate and journal as runtime. That’s already how the system is shaped—lean into it. 
* **Sequence:** get a “walking skeleton” world executing simple reducers → micro‑effects → plans → receipts → policy/cap gating; then wire in the governance loop/shadow‑runs; only then introduce an LLM adapter and a small “self‑edit” demo. This preserves determinism and auditability while you add surface area.  

---

## Milestone roadmap to v1.0

> Each milestone has: scope you should implement, a small demo (“test app”) that forces the path, and acceptance tests. The demos avoid complex externals; when needed, use the HTTP/TIMER/BLOB builtin effects you’ve already reserved in AIR v1.  

### **M0 — Walking skeleton (worlds + journal + snapshots)**

**Implement**

* Deterministic stepper processing one event at a time over an append‑only journal; periodic snapshots; CAS for nodes/blobs; minimal restore/replay. Treat journal as authoritative. 
* AIR loader/validator to canonicalize authoring sugar → typed values → canonical CBOR with schema-bound hashing. Enforce identity and DAG sanity early. 
* Minimal **runtime API** to append DomainEvents and tick the stepper (tests can drive it; a CLI can come later). 

**Demo 0 – “CounterSM” (no effects)**

* A tiny reducer with typestate (`pc = Idle → Counting → Done`) that consumes events and emits nothing external. This validates reducer ABI, state schema binding, and golden replay determinism. 

**Acceptance**

* Golden replay test: replay journal to byte‑identical snapshot; corrupt segment quarantining test passes (load refuses bad segments). 
* Loader test: same authored JSON (sugar vs canonical) hashes to same CBOR for values and nodes. 

---

### **M1 — Micro‑effects path (Timer, Blob)**

**Implement**

* Effect Manager skeleton + **Timer** and **Blob** adapters. Each emits **signed receipts** (ed25519/HMAC), the kernel appends them, and (for micro‑effects) converts to `sys/*` *receipt events* for reducers. Enforce idempotency & height fences on receipts.   
* Capability grants (constraints + expiry) and the **policy gate** with first‑match‑wins, default‑deny. In v1 this is allow/deny only.  

**Demo 1 – “Hello Timer” (reducer‑only)**

* Reducer emits `timer.set` (allowed as a micro‑effect), receives `sys/TimerFired@1`, advances `pc`. Validates reducers can emit at most one micro‑effect and consume built‑in receipt events.  

**Demo 2 – “Blob Echo” (reducer‑only)**

* Reducer writes bytes via `blob.put` and later reads them via `blob.get`, handling `sys/BlobPutResult@1` / `sys/BlobGetResult@1`. Forces CAS plumbing + receipt‑to‑event routing. 

**Acceptance**

* Policy denies non‑micro effects from reducers (origin‑aware), allows timer/blob via tight cap slots; denial is journaled as `PolicyDecisionRecorded`.  
* Cap constraint enforcement at enqueue (hosts/models/max_tokens) and expiry checks are journaled. 

---

### **M2 — Plan engine (single plan orchestration) + HTTP adapter**

**Implement**

* Plan executor for v1 steps: `emit_effect`, `await_receipt`, `raise_event`, `await_event`, `assign`, `end`; guards on edges; deterministic scheduling. Record `PlanResult` when `end` has a value.  
* **HTTP adapter** with capability checks (host/verb/path prefixes) and receipts.  
* **Manifest triggers** (Reducer→Plan) so DomainIntent events start plans with `correlate_by` keys.  

**Demo 3 – “Fetch & Notify (single plan)”**

* Reducer validates an input event and emits `FetchRequested@1`.
* Trigger starts `fetch_plan@1`:

  1. `http.request` → 2) `await_receipt` → 3) `raise_event` back to reducer with a typed result.
* Demonstrates Single‑Plan orchestration pattern and typed `raise_event` boundary.  

**Acceptance**

* Validator rejects plans whose `await_receipt.for` doesn’t reference an emitted handle; rejects `emit_effect.kind` not in `allowed_effects`; rejects missing `required_caps`. 
* Policy default‑deny, explicit allow for http to a test host from *plans* only (origin_kind filter). 

---

### **M3 — Parallelism inside a plan + fan‑in**

**Implement**

* No new primitives, just ensure scheduler handles multiple ready steps deterministically so plan DAG can fan‑out N `emit_effect`s and join after `await_receipt`s.  

**Demo 4 – “Aggregator (fan‑out/join)”**

* Plan fires three `http.request`s in parallel, awaits all, `assign` merges outputs, `end`. Proves readiness & join semantics. 

**Acceptance**

* Replay with recorded receipts yields byte‑identical plan outputs and reducer state.

---

### **M4 — Choreography (multi‑plan) + compensations**

**Implement**

* Nothing new in kernel; exercise triggers and correlation keys across multiple small plans. Provide reducer‑driven compensation (business logic stays in reducer).  

**Demo 5 – “3‑plan chain + compensation”**

* `Event A → charge_plan → Event B → reserve_plan → Event C → notify_plan`.
* If `reserve_plan` fails, reducer emits compensation intent to trigger `refund_plan`. Illustrates choreography and reducer‑driven saga. 

**Acceptance**

* Correlation key threading (`correlate_by`) verified end‑to‑end; no orphan result events. 

---

### **M5 — Governance loop (+ shadow run)**

**Implement**

* Proposal → Shadow → Approval → Apply flow as first‑class design‑time events in the journal, with shadow predicting effects/costs/diffs. Plan/cap/policy changes only take effect via Apply. 
* Shadow receipts are stubbed; produce typed diffs and predicted effects for approval review (“least‑privilege” grants derived from shadow). 

**Demo 6 – “Safe upgrade of a plan”**

* Propose adding an extra `assign` + `http.request` step to `fetch_plan`. Shadow shows predicted extra effect + cost; approval grants updated HTTP cap; apply; rerun the world; observe new behavior and recorded `PlanResult`. 

**Acceptance**

* Shadow output includes `{effects_predicted, diffs}`; after Apply, manifest root changes atomically and execution uses new refs. 

---

### **M6 — LLM adapter + policy**

**Implement**

* `llm.generate` adapter with usage/cost in receipt; enforce cap constraints (model allowlists, max_tokens ceilings) and allow only from plans (deny from reducers).   

**Demo 7 – “LLM summarizer plan”**

* Plan that (a) fetches text via HTTP, (b) calls `llm.generate`, (c) posts result to a sink (HTTP) or raises a reducer event for tracking. Mirrors the AIR example of a daily digest, but keep it minimal. 

**Acceptance**

* Policy journals decisions (origin_kind/name visible); receipts carry usage/cost for audit.

---

### **v1.0 Definition of Done**

* Deterministic stepper + journal + snapshots + restore; receipts with signatures + fences; content‑addressed store. 
* AIR v1 loader/validator (schemas, modules, plans, caps, policy) with canonicalization rules and semantic checks.      
* Plan engine supporting the six v1 steps + guards; manifest routing/triggers; capability ledger; policy gate; four built‑in adapters (HTTP, Blob, Timer, LLM).   
* Governance loop with shadow run; `PlanResult` records readable from journal. 
* End‑to‑end replay tests for all demos; failure handling (timeouts via `timer.set`, idempotency) validated.  

> **Deferred to 1.1** (optional): keyed reducers (“cells”), plan cancellation/await‑any, plan‑level retries/for‑each, human approval as a first‑class decision (not just an adapter), WASM‑based adapters. Keep these out of 1.0 to stabilize the substrate.  

---

## Demo ladder (quick index)

| Level | Name                | What it proves              | Pattern                   |
| ----- | ------------------- | --------------------------- | ------------------------- |
| 0     | CounterSM           | Reducer ABI + replay        | Reducer only              |
| 1     | Hello Timer         | Micro‑effects, sys receipts | Reducer + timer.set       |
| 2     | Blob Echo           | CAS + blob receipts         | Reducer + blob.{put,get}  |
| 3     | Fetch & Notify      | Single‑plan orchestration   | Plan‑driven               |
| 4     | Aggregator          | Fan‑out/join in DAG         | Plan DAG parallelism      |
| 5     | 3‑plan chain + comp | Choreography + reducer saga | Multi‑plan + comp         |
| 6     | Safe upgrade        | Propose→Shadow→Apply        | Design‑time loop          |
| 7     | LLM summarizer      | Caps/policy                 | LLM in plan               |

Each app is a glorified integration test: a tiny reducer (WASM), a few `defschema`s, one or more `defplan`s, caps, a default policy, and a manifest with routing/triggers. That’s precisely what AIR v1 expects.  

---

## Tooling/DevEx you should add as you go (thin, but essential)

* **Minimal CLI** (text‑first): `world init`, `world run`, `journal tail`, `snapshot create/restore`, `plan start`, `receipts show`, `cap grant`, `policy set`, `propose`, `shadow`, `approve`, `apply`. This mirrors the architecture’s CLI outline and makes demos runnable without harness code. 
* **Inspectors:** dump canonical AIR; `air fmt`, `air diff`, `air patch`; plan visualizer (text). Use the same canonical CBOR everywhere so hashes line up. 
* **SDK stubs:** tiny Rust helper for the reducer ABI (step envelope, CBOR in/out, fences/idempotency helpers) to keep reducers boring and testable. 

---

## Engineering notes (what to build *and why*, in this order)

1. **Deterministic substrate first.** Canonical CBOR + content addressing + stepper replay are the bedrock for receipts, policies, and audits; everything else composes on top of this.  
2. **Clear reducer/plan boundary.** Keep business invariants and typestate in reducers; push orchestration and external IO into plans. Enforce via policy (origin_kind) and a reducer micro‑effects allowlist. This keeps shadow‑runs analyzable and receipts at explicit choke points.  
3. **Capabilities before policy *features*.** Get least‑privilege working—host/model allowlists and constraint enforcement—before adding richer approvals/rate limits. That matches your v1 policy scope.  
4. **Plan engine with small surface.** The six step kinds + guards, no loops, no recursion (by design). That keeps plans analyzable and shadow‑simulable.  
5. **Receipt rigor.** Each adapter signs receipts including intent hash, inputs/outputs hashes, timings, and cost; kernel verifies and journals; late receipts are fenced out after rollback. This is your audit backbone.  
6. **Design‑time loop.** Make “self‑modification as data” real: proposals are AIR patches; shadow produces typed diffs and predicted effect counts/costs; approvals lead to manifest swaps. That’s the agent‑ready control plane.  
7. **LLM last.** Once governance is real, add `llm.generate` with caps/policy—and showcase it with a trivial summarizer plan and *one* “self‑edit” demo (agent proposes a one‑step plan change).  

---

## A tiny “v1 launch demo” you can show end‑to‑end

> **“World that upgrades itself to add a step.”**

1. Start a world with `fetch_plan@1` (GET a safe test URL, `end`).
2. Propose `fetch_plan@2` (add an `assign` to tweak headers + a second GET). Shadow shows 1 extra effect and +X bytes. Approve + apply.
3. Trigger the reducer → plan runs v2 → receipts show both HTTP calls → `PlanResult` includes the concatenated body hash.
4. Optional: propose adding a summarization step; shadow predicts LLM tokens/cents for review; approve updates LLM cap grants; run again and inspect receipts.

This compresses everything users need to *trust* AOS: determinism, receipts, capabilities/policy, plans vs reducers, and the constitutional loop.   

---

## Risks & trims (to keep the happy path happy)

* **Do not** add keyed reducers (“cells”), cancellation/await‑any, or rate limits to v1.0. Design APIs for them, but keep code paths small until the demos demand them. Put them under a 1.1 flag.  
* **Keep adapters boring**: HTTP, Blob, Timer, LLM only; receipts are uniform; no WASM adapters until 1.2. 
* **Default‑deny policy** from day one; only explicitly allow what the demos need, and only from plans. This prevents drift and proves governance posture. 

---

## What you get at the end of this plan

* A minimal but **earned** v1.0 runtime with a crisp boundary between pure computation (reducers) and governed effects (plans), all replayable from the journal + receipts. 
* A small **demo suite** that doubles as integration tests and adapter contracts (timer, blob, http, llm). 
* A design‑time story that makes “agent modifies its world” not just aspirational, but a routine, audited action through AIR patches and shadow runs. 

If you’d like, I can draft the first two demos’ concrete AIR and reducer stubs (schemas, manifest, reducer skeleton, plan JSON) so they’re ready to drop into your test harness.


--- 
Awesome — here are the **first two “test apps” fully designed** so you can drop them into your repo and wire them into the unit/integration harness. I’ve kept them small, strict to AIR v1, and aligned with the runtime boundaries (reducers own state + business logic; plans do orchestration — though these two demos intentionally avoid plans). I include **authoring‑sugar JSON** for AIR nodes and **Rust reducer skeletons** that follow the WASM reducer ABI. Notes call out where the kernel/validator enforces things.   

> **Hashes**: anywhere you see `sha256:000…000` it’s a placeholder. Your loader/packager will canonicalize nodes to CBOR and compute real hashes before producing `manifest.air.cbor`. That’s by design in AIR v1. 

---

## Test App 0 — **CounterSM** (reducer‑only, no effects)

**What it proves**

* Reducer ABI shape, state evolution, routing from events → reducer, and **golden replay** (no receipts involved).  

### AIR nodes (authoring sugar)

**Schemas**

```json
{
  "$kind": "defschema",
  "name": "demo/CounterPc@1",
  "type": { "variant": { "Idle": {"unit":{}}, "Counting": {"unit":{}}, "Done": {"unit":{}} } }
}
```

```json
{
  "$kind": "defschema",
  "name": "demo/CounterState@1",
  "type": {
    "record": {
      "pc": { "ref": "demo/CounterPc@1" },
      "count": { "nat": {} },
      "target": { "option": { "nat": {} } }
    }
  }
}
```

```json
{
  "$kind": "defschema",
  "name": "demo/CounterEvent@1",
  "type": {
    "variant": {
      "Start": { "record": { "target": { "nat": {} } } },
      "Bump":  { "record": { "by": { "nat": {} } } },
      "Stop":  { "unit": {} }
    }
  }
}
```

**Reducer module**

```json
{
  "$kind": "defmodule",
  "name": "demo/CounterSM@1",
  "module_kind": "reducer",
  "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "abi": {
    "reducer": {
      "state": "demo/CounterState@1",
      "event": "demo/CounterEvent@1",
      "effects_emitted": []
    }
  }
}
```

The reducer declares no effects; the validator enforces the ABI and type references. 

**Manifest (minimal)**

```json
{
  "$kind": "manifest",
  "schemas": [
    {"name":"demo/CounterPc@1","hash":"sha256:000..."},
    {"name":"demo/CounterState@1","hash":"sha256:000..."},
    {"name":"demo/CounterEvent@1","hash":"sha256:000..."}
  ],
  "modules": [
    {"name":"demo/CounterSM@1","hash":"sha256:000..."}
  ],
  "plans": [],
  "caps": [],
  "policies": [],
  "routing": {
    "events": [
      { "event": "demo/CounterEvent@1", "reducer": "demo/CounterSM@1" }
    ]
  }
}
```

AIR manifest requires listing refs by name+hash; your build step fills the hashes. Routing ensures any `demo/CounterEvent@1` appended to the journal is delivered to `CounterSM`. 

### Reducer skeleton (Rust → WASM)

```rust
use serde::{Serialize, Deserialize};
use aos_wasm_sdk::{StepInput, StepOutput, DomainEvent, cbor};

#[derive(Serialize, Deserialize, Clone)]
pub enum Pc { Idle, Counting, Done }
impl Default for Pc { fn default() -> Self { Pc::Idle } }

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct State {
    pub pc: Pc,
    pub count: u64,
    pub target: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    Start { target: u64 },
    Bump  { by: u64 },
    Stop,
}

#[no_mangle]
pub extern "C" fn step(ptr: i32, len: i32) -> (i32, i32) {
    aos_wasm_sdk::entry(step_impl, ptr, len)
}

fn step_impl(input: StepInput<State, Event>) -> StepOutput<State, serde_cbor::Value> {
    let mut s = input.state;
    let mut domain_events: Vec<DomainEvent> = vec![]; // none in this demo
    let effects = vec![]; // reducers own state; no external effects here

    match (s.pc.clone(), input.event) {
        (Pc::Idle, Event::Start { target }) => {
            s.pc = Pc::Counting;
            s.count = 0;
            s.target = Some(target);
        }
        (Pc::Counting, Event::Bump { by }) => {
            let target = s.target.unwrap_or(0);
            s.count = s.count.saturating_add(by);
            if s.count >= target { s.pc = Pc::Done; }
        }
        (Pc::Counting, Event::Stop) => { s.pc = Pc::Done; }
        _ => {} // ignore mismatches (idempotent)
    }

    StepOutput { state: s, domain_events, effects, ann: None }
}
```

Deterministic reducer, no wall‑clock or IO; replaying the same event stream yields the same state bytes — exactly what the stepper + journal model guarantees.  

**Acceptance (as tests)**

* Append `Start{target:3}`, then `Bump{by:1}` × 3 → final `pc=Done,count=3`.
* Golden replay: re‑load world, replay journal → byte‑identical snapshot. 

---

## Test App 1 — **Hello Timer** (reducer micro‑effect + receipt back)

**What it proves**

* **Micro‑effects from reducers** (`timer.set`) with **capability + policy** allow, and **receipt→event** delivery via `sys/TimerFired@1`. Demonstrates reducer typestate, fences, and origin‑aware policy gating.   

### AIR nodes (authoring sugar)

**Schemas**

We model a union of domain and receipt events so the reducer can consume both a `Start` command and the timer receipt as a single `event` type. (The kernel routes *both* schemas to this reducer; see `routing.events` below.) 

```json
{
  "$kind": "defschema",
  "name": "demo/TimerPc@1",
  "type": { "variant": { "Idle":{"unit":{}}, "Awaiting":{"unit":{}}, "Done":{"unit":{}}, "TimedOut":{"unit":{}} } }
}
```

```json
{
  "$kind": "defschema",
  "name": "demo/TimerState@1",
  "type": {
    "record": {
      "pc": { "ref": "demo/TimerPc@1" },
      "key": { "option": { "text": {} } },
      "deadline_ns": { "option": { "nat": {} } },
      "fired_key": { "option": { "text": {} } }
    }
  }
}
```

```json
{
  "$kind": "defschema",
  "name": "demo/TimerEvent@1",
  "type": {
    "variant": {
      "Start": { "record": { "deliver_at_ns": { "nat": {} }, "key": { "text": {} } } },
      "Fired": { "ref": "sys/TimerFired@1" }
    }
  }
}
```

> `sys/TimerFired@1` (and its companion param/receipt schemas) are part of the built‑ins set; include them in the manifest so hashing is consistent across worlds. 

**Capability (timer)**

If you don’t already vend built‑in caps in your bootstrap, define (or reference) a `timer` capability type. It has no parameters in v1. 

```json
{
  "$kind": "defcap",
  "name": "sys/timer@1",
  "cap_type": "timer",
  "schema": { "record": {} }
}
```

**Policy (allow timer from reducers only)**

Default‑deny posture; explicitly allow `timer.set` when origin is a reducer. First‑match‑wins. 

```json
{
  "$kind": "defpolicy",
  "name": "demo/default_policy@1",
  "rules": [
    { "when": { "effect_kind": "timer.set", "origin_kind": "reducer" }, "decision": "allow" }
  ]
}
```

**Reducer module**

Declare the micro‑effect allowlist and a slot `timer` (cap_type `timer`) that we’ll bind in the manifest. The validator enforces `effects_emitted ⊆ micro‑effects` and that the slot is bound.  

```json
{
  "$kind": "defmodule",
  "name": "demo/TimerSM@1",
  "module_kind": "reducer",
  "wasm_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "abi": {
    "reducer": {
      "state": "demo/TimerState@1",
      "event": "demo/TimerEvent@1",
      "effects_emitted": ["timer.set"],
      "cap_slots": { "timer": "timer" }
    }
  }
}
```

**Manifest**

Bind the reducer’s `timer` slot to a concrete CapGrant; route both the domain `Start` *and* `sys/TimerFired@1` to the reducer; set default policy.  

```json
{
  "$kind": "manifest",
  "schemas": [
    {"name":"demo/TimerPc@1","hash":"sha256:000..."},
    {"name":"demo/TimerState@1","hash":"sha256:000..."},
    {"name":"demo/TimerEvent@1","hash":"sha256:000..."},
    {"name":"sys/TimerSetParams@1","hash":"sha256:000..."},
    {"name":"sys/TimerSetReceipt@1","hash":"sha256:000..."},
    {"name":"sys/TimerFired@1","hash":"sha256:000..."}
  ],
  "modules": [
    {"name":"demo/TimerSM@1","hash":"sha256:000..."}
  ],
  "plans": [],
  "caps": [
    {"name":"sys/timer@1","hash":"sha256:000..."}
  ],
  "policies": [
    {"name":"demo/default_policy@1","hash":"sha256:000..."}
  ],
  "defaults": {
    "policy": "demo/default_policy@1",
    "cap_grants": [
      { "name":"timer_grant", "cap":"sys/timer@1", "params": {} }
    ]
  },
  "module_bindings": {
    "demo/TimerSM@1": { "slots": { "timer": "timer_grant" } }
  },
  "routing": {
    "events": [
      { "event": "demo/TimerEvent@1", "reducer": "demo/TimerSM@1" },
      { "event": "sys/TimerFired@1", "reducer": "demo/TimerSM@1" }
    ]
  }
}
```

### Reducer skeleton (Rust → WASM)

```rust
use serde::{Serialize, Deserialize};
use aos_wasm_sdk::{StepInput, StepOutput, EffectIntent, DomainEvent, cbor};

#[derive(Serialize, Deserialize, Clone)]
pub enum Pc { Idle, Awaiting, Done, TimedOut }
impl Default for Pc { fn default() -> Self { Pc::Idle } }

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct State {
    pub pc: Pc,
    pub key: Option<String>,
    pub deadline_ns: Option<u64>,
    pub fired_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SysTimerFired {
    pub requested: SysTimerSetParams,
    pub receipt:   SysTimerSetReceipt,
    pub status: String
}
#[derive(Serialize, Deserialize)]
pub struct SysTimerSetParams { pub deliver_at_ns: u64, pub key: String }
#[derive(Serialize, Deserialize)]
pub struct SysTimerSetReceipt { pub delivered_at_ns: u64, pub key: String }

#[derive(Serialize, Deserialize)]
pub enum Event {
    Start { deliver_at_ns: u64, key: String },
    Fired (SysTimerFired)
}

#[no_mangle]
pub extern "C" fn step(ptr: i32, len: i32) -> (i32, i32) {
    aos_wasm_sdk::entry(step_impl, ptr, len)
}

fn step_impl(input: StepInput<State, Event>) -> StepOutput<State, serde_cbor::Value> {
    let mut s = input.state;
    let mut domain_events: Vec<DomainEvent> = vec![];
    let mut effects: Vec<EffectIntent<serde_cbor::Value>> = vec![];

    match (s.pc.clone(), input.event) {
        (Pc::Idle, Event::Start { deliver_at_ns, key }) => {
            // emit the micro-effect (timer.set) using bound slot "timer"
            effects.push(EffectIntent {
                kind: "timer.set".into(),
                params: cbor!({"deliver_at_ns": deliver_at_ns, "key": key.clone()}),
                cap_slot: Some("timer".into())
            });
            s.key = Some(key);
            s.deadline_ns = Some(deliver_at_ns);
            s.pc = Pc::Awaiting;
        }

        (Pc::Awaiting, Event::Fired(sys)) => {
            // Idempotent fence: only accept matching key
            if let (Some(k), Some(deadline)) = (s.key.clone(), s.deadline_ns) {
                if sys.requested.key == k && sys.requested.deliver_at_ns == deadline && sys.status == "ok" {
                    s.fired_key = Some(sys.receipt.key);
                    s.pc = Pc::Done;
                } else {
                    // Ignore stray/late receipts; stepper/replay guarantees help here
                }
            }
        }

        _ => {}
    }

    StepOutput { state: s, domain_events, effects, ann: None }
}
```

Why this fits v1 guardrails:

* **Reducers may emit only micro‑effects** like `timer.set`; heavy IO/LLM would be denied by policy from reducers and should be lifted to a plan. The origin‑aware policy rule above allows only the timer here.  
* The adapter turns the effect into a signed **receipt**; the kernel converts it into the built‑in `sys/TimerFired@1` event routed back to the reducer. Replay uses recorded receipts, preserving determinism.  

**Acceptance (as tests)**

1. Append `demo/TimerEvent@1.Start{deliver_at_ns: T, key:"k1"}` → reducer emits `timer.set`.
2. Adapter returns a receipt → kernel appends `sys/TimerFired@1` → reducer consumes it and moves `pc=Done`.
3. Golden replay yields identical state bytes and same journal ordering (`EffectQueued` → `ReceiptAppended` → event to reducer). 

---

## Harness notes (both apps)

* **Authoring → canonicalization**: Load the JSON above through the AIR loader. It will type‑check, canonicalize to CBOR, and compute `sha256(cbor(node))` hashes; use those to assemble `manifest.air.cbor`. This is the identity model for values and nodes in AIR v1. 
* **Routing**: `routing.events[]` wires which events a reducer consumes. For Hello Timer, route both the domain event (`demo/TimerEvent@1`) and the built‑in receipt (`sys/TimerFired@1`) to the same reducer; the **reducer’s `event` schema is the variant family** that includes both.  
* **Policy & caps**: The kernel checks capability constraints and policy **at enqueue time**. Budget enforcement is deferred. 
* **Journal invariants**: Use these demos to validate `EffectIntent`/`Receipt` lifecycles and `PolicyDecisionRecorded` entries for the timer allow rule. 

---

## What’s next (optional quick follow‑ups)

* **Test App 2 — Blob Echo**: same pattern as Hello Timer but with `blob.put/get` receipts mapped to `sys/Blob*Result@1` and state fences (exercise CAS + receipts). 
* **Single‑Plan demo** (“Fetch & Notify”): introduce one plan (`emit_effect` → `await_receipt` → `raise_event`) to verify the plan engine, required_caps, and `allowed_effects` checks.  

If you want, I can also produce **ready‑to‑paste test fixtures**: (1) a tiny journal appender that feeds the exact Start/Bump events, (2) canned receipts for a fake timer adapter to exercise replay, and (3) a minimal “manifest‑builder” script that hashes nodes and fills those `sha256:000…` placeholders for you. 
