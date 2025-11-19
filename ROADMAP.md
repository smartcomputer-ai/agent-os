# Roadmap

The current roadmap is structured as a clear **ladder**: each rung bringing us closer to a demoable version. The sequence matters because each step builds on the next.

## North‑star

* **North star:** a running world that can be modified by an LLM‑based agent via the constitutional loop (propose → shadow → approve → apply). Keep this *design‑time* path on the same deterministic substrate and journal as runtime. That’s already how the system is shaped—lean into it. 
* **Sequence:** get a “walking skeleton” world executing simple reducers → micro‑effects → plans → receipts → policy/cap budgets; then wire in the governance loop/shadow‑runs; only then introduce an LLM adapter and a small “self‑edit” demo. This preserves determinism and auditability while you add surface area.  

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


### **M1 — Micro‑effects path (Timer, Blob)**

**Implement**

* Effect Manager skeleton + **Timer** and **Blob** adapters. Each emits **signed receipts** (ed25519/HMAC), the kernel appends them, and (for micro‑effects) converts to `sys/*` *receipt events* for reducers. Enforce idempotency & height fences on receipts.   
* Capability ledger (grants with optional budgets/expiry) and the **policy gate** with first‑match‑wins, default‑deny. In v1 this is allow/deny only.  

**Demo 1 – “Hello Timer” (reducer‑only)**

* Reducer emits `timer.set` (allowed as a micro‑effect), receives `sys/TimerFired@1`, advances `pc`. Validates reducers can emit at most one micro‑effect and consume built‑in receipt events.  

**Demo 2 – “Blob Echo” (reducer‑only)**

* Reducer writes bytes via `blob.put` and later reads them via `blob.get`, handling `sys/BlobPutResult@1` / `sys/BlobGetResult@1`. Forces CAS plumbing + receipt‑to‑event routing. 

**Acceptance**

* Policy denies non‑micro effects from reducers (origin‑aware), allows timer/blob via tight cap slots; denial is journaled as `PolicyDecisionRecorded`.  
* Budget settlement on receipts (bytes/cents) decrements grant balances; enqueue pre‑checks for known sizes. 


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


### **M3 — Parallelism inside a plan + fan‑in**

**Implement**

* No new primitives, just ensure scheduler handles multiple ready steps deterministically so plan DAG can fan‑out N `emit_effect`s and join after `await_receipt`s.  

**Demo 4 – “Aggregator (fan‑out/join)”**

* Plan fires three `http.request`s in parallel, awaits all, `assign` merges outputs, `end`. Proves readiness & join semantics. 

**Acceptance**

* Replay with recorded receipts yields byte‑identical plan outputs and reducer state.


### **M4 — Choreography (multi‑plan) + compensations**

**Implement**

* Nothing new in kernel; exercise triggers and correlation keys across multiple small plans. Provide reducer‑driven compensation (business logic stays in reducer).  

**Demo 5 – “3‑plan chain + compensation”**

* `Event A → charge_plan → Event B → reserve_plan → Event C → notify_plan`.
* If `reserve_plan` fails, reducer emits compensation intent to trigger `refund_plan`. Illustrates choreography and reducer‑driven saga. 

**Acceptance**

* Correlation key threading (`correlate_by`) verified end‑to‑end; no orphan result events. 


### **M5 — Governance loop (+ shadow run)**

**Implement**

* Proposal → Shadow → Approval → Apply flow as first‑class design‑time events in the journal, with shadow predicting effects/costs/diffs. Plan/cap/policy changes only take effect via Apply. 
* Shadow receipts are stubbed; produce typed diffs and predicted budgets for approval review (“least‑privilege” grants derived from shadow). 

**Demo 6 – “Safe upgrade of a plan”**

* Propose adding an extra `assign` + `http.request` step to `fetch_plan`. Shadow shows predicted extra effect + cost; approval grants updated HTTP cap; apply; rerun the world; observe new behavior and recorded `PlanResult`. 

**Acceptance**

* Shadow output includes `{effects_predicted, diffs}`; after Apply, manifest root changes atomically and execution uses new refs. 

### **M6 — LLM adapter + budgets + policy**

**Implement**

* `llm.generate` adapter with usage/cost in receipt; enforce conservative **pre‑check** vs **settlement** on budgets and allow only from plans (deny from reducers).   

**Demo 7 – “LLM summarizer plan”**

* Plan that (a) fetches text via HTTP, (b) calls `llm.generate`, (c) posts result to a sink (HTTP) or raises a reducer event for tracking. Mirrors the AIR example of a daily digest, but keep it minimal. 

**Acceptance**

* Token/cents budgets decrement on receipt; over‑budget grants are denied on next enqueue; policy journals decisions (origin_kind/name visible).  


### **v1.0 Definition of Done**

* Deterministic stepper + journal + snapshots + restore; receipts with signatures + fences; content‑addressed store. 
* AIR v1 loader/validator (schemas, modules, plans, caps, policy) with canonicalization rules and semantic checks.      
* Plan engine supporting the six v1 steps + guards; manifest routing/triggers; capability ledger; policy gate; four built‑in adapters (HTTP, Blob, Timer, LLM).   
* Governance loop with shadow run; `PlanResult` records readable from journal. 
* End‑to‑end replay tests for all demos; failure handling (timeouts via `timer.set`, idempotency) validated.  

> **Deferred to 1.1** (optional): keyed reducers (“cells”), plan cancellation/await‑any, plan‑level retries/for‑each, human approval as a first‑class decision (not just an adapter), WASM‑based adapters. Keep these out of 1.0 to stabilize the substrate.  


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
| 7     | LLM summarizer      | Caps/budgets/policy         | LLM in plan               |

Each app is a glorified integration test: a tiny reducer (WASM), a few `defschema`s, one or more `defplan`s, caps, a default policy, and a manifest with routing/triggers. That’s precisely what AIR v1 expects.  


## Tooling/DevEx you should add as you go (thin, but essential)

* **Minimal CLI** (text‑first): `world init`, `world run`, `journal tail`, `snapshot create/restore`, `plan start`, `receipts show`, `cap grant`, `policy set`, `propose`, `shadow`, `approve`, `apply`. This mirrors the architecture’s CLI outline and makes demos runnable without harness code. 
* **Inspectors:** dump canonical AIR; `air fmt`, `air diff`, `air patch`; plan visualizer (text). Use the same canonical CBOR everywhere so hashes line up. 
* **SDK stubs:** tiny Rust helper for the reducer ABI (step envelope, CBOR in/out, fences/idempotency helpers) to keep reducers boring and testable. 


## Engineering notes (what to build *and why*, in this order)

1. **Deterministic substrate first.** Canonical CBOR + content addressing + stepper replay are the bedrock for receipts, policies, and audits; everything else composes on top of this.  
2. **Clear reducer/plan boundary.** Keep business invariants and typestate in reducers; push orchestration and external IO into plans. Enforce via policy (origin_kind) and a reducer micro‑effects allowlist. This keeps shadow‑runs analyzable and receipts at explicit choke points.  
3. **Capabilities before policy *features*.** Get least‑privilege *working*—host/model allowlists, budgets pre‑check at enqueue and settle on receipt—before adding richer approvals/rate limits. That matches your v1 policy scope.  
4. **Plan engine with small surface.** The six step kinds + guards, no loops, no recursion (by design). That keeps plans analyzable and shadow‑simulable.  
5. **Receipt rigor.** Each adapter signs receipts including intent hash, inputs/outputs hashes, timings, and cost; kernel verifies and journals; late receipts are fenced out after rollback. This is your audit backbone.  
6. **Design‑time loop.** Make “self‑modification as data” real: proposals are AIR patches; shadow produces typed diffs and predicted effect counts/costs; approvals lead to manifest swaps. That’s the agent‑ready control plane.  
7. **LLM last.** Once governance is real, add `llm.generate` with budgets—and showcase it with a trivial summarizer plan and *one* “self‑edit” demo (agent proposes a one‑step plan change).  


## “v1 launch demo”

> **“World that upgrades itself to add a step.”**

1. Start a world with `fetch_plan@1` (GET a safe test URL, `end`).
2. Propose `fetch_plan@2` (add an `assign` to tweak headers + a second GET). Shadow shows 1 extra effect and +X bytes. Approve + apply.
3. Trigger the reducer → plan runs v2 → receipts show both HTTP calls → `PlanResult` includes the concatenated body hash.
4. Optional: propose adding a summarization step; shadow predicts LLM tokens/cents; approve updates LLM cap budget; run again and inspect receipts.

This compresses everything users need to *trust* AOS: determinism, receipts, capabilities/policy, plans vs reducers, and the constitutional loop.   


## Risks & trims (to keep the happy path happy)

* **Do not** add keyed reducers (“cells”), cancellation/await‑any, or rate limits to v1.0. Design APIs for them, but keep code paths small until the demos demand them. Put them under a 1.1 flag.  
* **Keep adapters boring**: HTTP, Blob, Timer, LLM only; receipts are uniform; no WASM adapters until 1.2. 
* **Default‑deny policy** from day one; only explicitly allow what the demos need, and only from plans. This prevents drift and proves governance posture. 
