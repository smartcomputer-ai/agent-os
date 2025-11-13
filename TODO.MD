# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Implementation Plan — Examples Ladder

1. **[x] Scaffold `examples/` layout + workspace plumbing** — `examples/0x-*` directories exist with README + subfolders, new `aos-examples` crate registers numbered demos, CLI listing/subcommands/all-mode wired up.

2. **[x] Implement shared example runner + FS-backed harness** — Harness code now builds manifests via `aos-store`, runs `aos-kernel` with FsStore/FsJournal in `.aos/`, exposes metadata-driven CLI subcommands, and replays after each demo.

3. **[x] Example 00 — CounterSM** — Reducer crate emits `alloc/step`, demo seeds Start/Tick events, prints state transitions, and verifies replay hash.

4. **[x] Example 01 — Hello Timer** — Rust manifest builders cover schemas/cap/policy, timer reducer emits `timer.set`, harness drains effects and synthesizes receipts, CLI shows Start → timer receipt → replay hash.

5. **Example 02 — Blob Echo**  
   - Clone the directory pattern, add blob schemas/caps, and implement the reducer that issues `blob.put` then `blob.get` with state fences and CAS validation.  
   - Add a blob adapter in the harness that writes payloads to the filesystem store, enforces byte budgets, and feeds receipts back to the reducer.  
   - Update the CLI to include this example in `--all`, ensuring its journal + replay checks pass using the same FS-backed environment.

> Once these three examples are stable, replicate the same structure for the remaining ladder rungs (plans, fan-out, governance, LLM) by adding numbered directories plus scenario implementations under the shared CLI.

- [ ] Cache compiled reducer modules/instances in `aos-wasm` so each reducer tick doesn’t recompile the WASM module (current per-event latency is dominated by fresh instantiation).

# Program Exampels

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
* **Policy & caps**: The kernel checks capability constraints and policy **at enqueue time**, and settles budgets **on receipt**; we didn’t add budgets here, but your grant shape supports them if you want to turn on negative‑tests. 
* **Journal invariants**: Use these demos to validate `EffectIntent`/`Receipt` lifecycles and `PolicyDecisionRecorded` entries for the timer allow rule. 

---

## What’s next (optional quick follow‑ups)

* **Test App 2 — Blob Echo**: same pattern as Hello Timer but with `blob.put/get` receipts mapped to `sys/Blob*Result@1` and state fences (exercise CAS + bytes budgets). 
* **Single‑Plan demo** (“Fetch & Notify”): introduce one plan (`emit_effect` → `await_receipt` → `raise_event`) to verify the plan engine, required_caps, and `allowed_effects` checks.  

If you want, I can also produce **ready‑to‑paste test fixtures**: (1) a tiny journal appender that feeds the exact Start/Bump events, (2) canned receipts for a fake timer adapter to exercise replay, and (3) a minimal “manifest‑builder” script that hashes nodes and fills those `sha256:000…` placeholders for you. 
