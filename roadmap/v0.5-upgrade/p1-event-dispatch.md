# Typed Bus + Event-Family Dispatch

**Complete**

Here’s the design I would lock in. It keeps the system **fully typed end‑to‑end**, removes the current ambiguity/mismatch, and still gives you “bigger refactor” headroom without committing you to self‑describing payloads or schema‑aware reducers.

---

## Decision

### Make routing *strictly type-checked* against the reducer’s declared event ABI, and have the kernel **wrap/convert** bus events into the reducer’s **event-family** schema at dispatch time.

Concretely:

* The journal/bus carries **DomainEvents** as: **(schema_name, canonical_value_bytes, optional_key)**. This matches the AIR “journal invariant” model: *events are canonicalized against the schema they claim* and the journal stores only canonical bytes. 
* `manifest.routing.events` remains the **wiring table** (who gets what).  
* `defmodule.abi.reducer.event` becomes what it always *wanted* to be: the reducer’s **event-family** schema (one schema), but typically a **variant of refs** to multiple event schemas (domain + receipt).  
* The kernel **must** enforce at load/validation time that every routing entry is compatible with the reducer’s event family, and at runtime it **must never** deliver an event the reducer can’t decode according to its ABI.
* Routing may target **either the bus schema E or the reducer family schema F**. If the route uses F directly, dispatch is identity (no wrapping).

This resolves your “Purpose vs routing” tension by turning it into a clean two-layer model:

* **ABI** answers: “What can this reducer decode?” (`defmodule.abi.reducer.event`)
* **Routing** answers: “Which events are actually delivered?” (`manifest.routing.events`)

…and the validator ensures they never disagree.

---

## Why this is the “perfect” shape

### 1) It preserves the strongest invariant you want

Reducers run in deterministic WASM and should deserialize events into an enum/struct reliably; they should not need dynamic dispatch on “schema name” at runtime. Your docs already set up that expectation: reducers consume canonical CBOR “matching the declared schema,” and `abi.reducer.event` is described as the “domain/receipt event type family.” 

### 2) It eliminates today’s foot-gun

Right now, routing can deliver *any schema* to *any reducer* if listed, and the reducer ABI doesn’t constrain dispatch. That’s exactly how you got the confirmed mismatches (routing `sys/BlobPutResult@1` to a reducer that declares `demo/BlobEchoEvent@1`, etc.). This design makes those worlds **fail fast at manifest validation**.

### 3) It keeps event payloads non–self-describing

You explicitly want to reject `$schema` embedded in payloads (“self-describing payloads are rejected”).  
This design keeps schema identity where it belongs: **outside** the value bytes, in the event envelope and control plane.

### 4) It supports “one schema per reducer” *and* multi-event inputs

You don’t need to expand `defmodule.abi.reducer.event` to an array. You keep one schema per reducer, but that schema is a variant “family.” This is already consistent with the spec language around unions/variants for reducer input. 

---

## The key semantic change: dispatch performs a typed conversion **[DONE]**

### Bus event (journal) stays:

```
DomainEvent {
  schema: E,
  value_cbor: bytes(E),
  key?: bytes(key_schema)   // optional
}
```

(That’s the model described in AIR’s runtime/journal sections.) 

### Reducer ABI still expects one schema F = `defmodule.abi.reducer.event`

So the kernel converts:

* If `F == E` (or `F` is a ref to `E`): deliver `value_cbor` directly.
* If `F` is a `variant` family that contains a member that references `E`:

  * wrap into `F` as `variant(tag_for_E, value)` and canonicalize as `F`
  * deliver those bytes

This is exactly the missing glue implied by calling `abi.reducer.event` a “type family,” while routing is per-schema.  

---

## Lock-in rules (these become validator rules)

### Rule A — Routing compatibility (the big fix) **[DONE]**

For every `manifest.routing.events[] = { event: E, reducer: R, ... }`:

1. Load reducer `R`’s ABI event schema `F = defmodule(R).abi.reducer.event`.  
2. Require `E ∈ family(F)` where `family(F)` is:

   * `{E}` if `F` is a ref to `E`, or
   * `{E_i}` if `F` is a variant and each variant arm is a `ref` to some schema `E_i`
3. If not, **manifest is invalid**.

Notes:
* Routes may specify **E or F**. If `event == F`, the route is trivially compatible and no wrapping is needed.
* Keying (`key_field`) is interpreted against the **routed event schema E**, even if the reducer receives wrapped `F`.

This alone prevents your current inconsistency class.

### Rule B — Event-family schema shape (make it machine-checkable) **[DONE]**

To keep conversion deterministic and tooling-friendly, constrain event families:

* `F` must be either:

  * `ref: SomeSchema@v` (single-event reducer), **or**
  * `variant: { Tag1: {ref:E1}, Tag2:{ref:E2}, ... }` (multi-event)
* No two tags may reference the same `Ei` (avoid ambiguous mapping).

This is a purely semantic validation on top of the JSON Schema shape system.  

### Rule C — Keying must be coherent **[DONE]**

You already have `key_field` on routing entries for keyed reducers (cells) and mention kernel validation around key usage.  

Make it strict:

* If reducer `R` has `key_schema`, then:

  * the routing entry **must** provide `key_field`
  * and schema `E` **must** contain that field with type exactly equal to `R.key_schema`
* At dispatch:

  * if the DomainEvent envelope carries `key`, prefer it (after typecheck)
  * else extract from `key_field`

This keeps both v1 and your v1.1 “cells” direction consistent.  

---

## Plan `raise_event`: I would refactor it now **[DONE]**

Your current ambiguity comes from “plan raise_event payload schema is inferred from reducer ABI schema.” That only works if the reducer ABI event is one schema and not a family/variant.

To make event families first-class *without* making plans build variant envelopes manually, I’d change the plan step to publish a **bus event** explicitly:

### New `raise_event` shape (recommended)

```json
{ "id":"x", "op":"raise_event", "event": "com.acme/PaymentResult@1", "value": { ... } }
```

* `event` is the schema name (SchemaRef)
* `value` is `ExprOrValue` typed against that schema

This aligns plan emission with reducer emission: reducers already emit `{schema, value}` for domain events. 
And it aligns with the AIR plan philosophy that payload schemas are known from context and canonicalized before journaling. 

### What I would remove

I would remove `reducer: Name` from `raise_event`. It’s redundant with routing and causes hacks like “EventBus@1” in workflows just to get an event into the system. Your workflows doc literally shows that pattern. 

With this change:

* a plan can publish an event **even if no reducer is routed** (useful for plan-to-plan choreography via triggers / await_event)
* reducers only see it if routing says so

This cleanly separates “publish” from “deliver.”

### Required spec + schema updates

* Update `spec/schemas/defplan.schema.json` `StepRaiseEvent`:

  * replace `{ reducer, event }` with `{ event, value }`
     

* Update the AIR prose in `03-air.md` accordingly. 

---

## Micro-effect receipts: make them fit naturally **[DONE]**

Built-in receipt event schemas already exist (`sys/TimerFired@1`, `sys/BlobPutResult@1`, `sys/BlobGetResult@1`).  
And built-in micro-effects are reducer-origin scoped (`blob.put/get`, `timer.set`). 

Under this design:

* These `sys/*` receipt schemas are just **ordinary bus event schemas**.
* Reducers that emit micro-effects should include the corresponding `sys/*` receipt schemas in their event family variant (and the validator can enforce that if `effects_emitted` includes those micro-effect kinds).  

I would also tweak the docs to stop implying you must manually route them; instead:

* The kernel **always** appends the receipt-derived event (so replay is deterministic)
* Delivery is via routing like any other event
* Tooling should auto-suggest/auto-add routing entries for these `sys/*` schemas when a reducer declares a micro-effect kind (lint/fix)

This avoids “micro-effects silently never resume the reducer because routing forgot a sys event.”

---

## How this fixes your flagged inconsistencies

### Example: `sys/BlobPutResult@1` routed to `demo/BlobEchoSM@1` but ABI says `demo/BlobEchoEvent@1`

With strict validation, the manifest would be rejected unless:

* `demo/BlobEchoEvent@1` is a variant family including a member referencing `sys/BlobPutResult@1`, **or**
* `demo/BlobEchoEvent@1` directly refs `sys/BlobPutResult@1` (unlikely)

So you’d fix it by defining:

```json
{
  "$kind": "defschema",
  "name": "demo/BlobEchoEvent@1",
  "type": {
    "variant": {
      "BlobPutResult": { "ref": "sys/BlobPutResult@1" },
      "BlobGetResult": { "ref": "sys/BlobGetResult@1" },
      "EchoRequested": { "ref": "demo/EchoRequested@1" }
    }
  }
}
```

…and the kernel will wrap `sys/BlobPutResult@1` into `demo/BlobEchoEvent@1` before calling the reducer.

### Example: wrapper schemas like `demo/TimerEvent@1` vs routing `sys/TimerFired@1`

You no longer need “wrapper event schemas” purely for reducer ergonomics. If you want a friendly enum variant name, it’s *the event family tag*. The bus schema remains `sys/TimerFired@1`, routing stays `sys/TimerFired@1 → SomeReducer`, and the reducer sees `TimerFired(...)` inside its family variant.

---

## Exact code changes (by subsystem)

I’ll describe this as a refactor plan you can implement in one pass.

### 1) Loader/validator: build a compiled dispatch map **[DONE]**

When loading the manifest, compute:

* `event_schema_map: SchemaName → SchemaHash/type`
* `reducer_event_family: ReducerName → EventFamilyInfo`

  * `EventFamilyInfo` includes `family_schema = F`
  * and a mapping `member_event_schema E → wrapper_kind` where `wrapper_kind` is:

    * `Identity` if `F == E`
    * `Variant { tag }` if `F` is a variant and `tag` is the arm referencing `E`

Then compile:

* `dispatch_table: EventSchema E → [DispatchTarget]`

  * each `DispatchTarget` includes:

    * `reducer: R`
    * `wrapper_kind` (Identity or Variant tag)
    * `keying`: `{ unkeyed }` or `{ keyed: { key_schema, key_field } }`

This is basically turning `manifest.routing.events` into an executable plan, but with full type info.

### 2) Validation: enforce the lock-in rules **[PARTIAL]**

In the validator (what you called `validate.rs`), add:

* Routing entry compatibility check (Rule A)
* Event family shape check (Rule B)
* Key coherency check (Rule C)

Fail the world load with a precise diagnostic:

* which routing entry failed
* expected family members vs actual
* if `key_field` missing/mistyped

This is the critical piece that makes misconfig impossible.

### 3) Dispatch: wrap on delivery **[DONE]**

In `world.rs route_event` (your mention), use `dispatch_table[E]`:

For each target:

* compute `key` (if keyed; prefer envelope key else extract via key_field)
* wrap `value_cbor(E)` into `value_cbor(F)` if needed
* invoke reducer with ABI event bytes = canonical CBOR of F

### 4) Plans: change `raise_event` to publish (schema, value) **[DONE]**

In `plan.rs` (your mention), change `raise_event` so it canonicalizes `value` against the explicit `event` schema (not reducer ABI). This matches the AIR “event payload normalization” model. 

Then append a DomainEvent (schema, bytes, maybe key) and let routing deliver.

### 5) Tooling/fixtures **[DONE]**

Anywhere that assumed “a reducer has exactly one event schema” should be updated to treat `defmodule.abi.reducer.event` as a family:

* Ensure all family member schemas exist in `manifest.schemas`
* If reducer declares micro-effects in `effects_emitted`, ensure the appropriate `sys/*` receipt schemas exist and (optionally) recommend routing entries

The effect catalog already defines which effects are reducer-origin scoped. 

---

## Spec/doc updates I would make **[DONE]**

1. **03-air.md**: make the routing/ABI relationship explicit and normative (validator rule). 
2. **04-reducers.md**: clarify that reducers receive their declared event family, and the kernel wraps bus events into that family. 
3. **05-workflows.md**: remove “EventBus@1” workaround; show plan-to-plan choreography by publishing events and triggering on them. 
4. Update **defplan.schema.json** to the new `raise_event` shape. 
5. Optionally add a short “Event Families” subsection to the architecture doc under Router/Inbox. 

---

## What you get after this refactor

* **No silent misroutes**: impossible to route an event schema to a reducer that can’t decode it.
* **Reducers stay simple**: still one ABI event type; still deterministic decoding.
* **Plans become clearer**: `raise_event` is symmetric with reducer-emitted events and supports plan→plan choreography without hacks.
* **Routing is wiring, ABI is contract**: no conflation, no ambiguity.
* **Cells stay compatible**: `key_field` remains meaningful and can be strictly validated.  
