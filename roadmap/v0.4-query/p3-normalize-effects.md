# Normalzie Effects

Question: how to handle events in the kernel and journal? Should we normalize them according to the plan or module schema? Should we store normalized events, or at least validate them, or if we normalize, do it only on the fly when passing it to plan or reducer? Wrinkle: Reducer returned events are nowhere specified, so we do not have that schema.

So the question is primarily regarding reducers, both keyed reducers and non-keyed ones. I do want it to be very ergonomic for reducers to write their code without too much parsing magic in every reducer (something we run into a lot right now). What should our approach be?

Very important: the system is in very early development phase. So we can make these changes without worrying that we break any existing, running system. In other words, we can make breaking changes without issues.

## Status (in progress)

- Added a shared value normalizer (`crates/aos-air-types/src/value_normalize.rs`) and wired kernel ingestion to canonicalize every `DomainEvent` by its schema before routing/journaling; events are stored/replayed as canonical CBOR.
- Routing/await now uses schema-aware decoding: keyed routes pull `key_field` via the declared key schema, plan `await_event` decodes by schema, and plan triggers build correlation from typed values.
- Plan `raise_event` keys are emitted as canonical CBOR; reducer-emitted/bad payloads are rejected early; tests updated/added around routing, await_event, and invalid payload rejection.
- Open: external injections/CLI should require explicit schema + run through the normalizer; receipts/events from adapters should be enforced via the same path; docs/spec updates to reflect “events like effect params” invariant.

## Context: reducers
The kernel never rewrites event payloads—keyed or non‑keyed. It only _reads_ them for routing/correlation by decoding as aos_air_exec::Value (externally‑tagged/“Value‑tagged” CBOR). After that, the original bytes are passed straight through to the reducer and stored in the journal unchanged. Because of that, payload shape matters:

- **Keyed routing & plan correlation**: route_event (world.rs) and extract_correlation_value decode the payload as Value to pull a field. If the payload isn’t Value‑tagged CBOR ({"Record":{...}}, {"Text":"foo"}, etc.), key extraction fails and the event is rejected. Same path is used for plan await event correlation.
    
- **Reducer execution**: handle_reducer_event wraps the _original_ event bytes in ReducerInput; no normalization to the event schema happens. A reducer must be able to deserialize whatever encoding the producer used.
    
- **Storage/journal**: events are recorded byte‑for‑byte as submitted; snapshots and replay just re‑inject those bytes.
    

How examples succeed:

- Reducers in examples/* accept struct‑shaped CBOR, and the example drivers encode events that way (they’re not keyed, so the router never tries to decode them as Value).
- Plans that raise events produce Value‑tagged CBOR, because plan expressions are evaluated to aos_air_exec::Value and then serde_cbor‑encoded—so correlation and routing work.

What changed for the catalog test:

- We injected events as ExprValue::Record, which is Value‑tagged. Key extraction worked, but the reducer expected struct‑shaped CBOR and panicked. Fix was to make the reducer accept the Value‑tagged form.

Takeaway:

- There is no hidden “normalize to schema” step for non‑keyed reducers either; normalization only exists for plan literals/effect params. If you send struct‑shaped CBOR, use reducers that deserialize structs. If you send Value‑tagged CBOR (common for plans and keyed routing), reducers must handle that shape or you need a translation layer at the producer.

If we want schema‑shaped events to work with keyed routing, we’d have to change the router (and plan correlation) to decode using the schema instead of Value, or re‑encode after key extraction.

## Context: Plans
How plans handle event payloads (and why they usually “just work”):

- **Raise event path** (plan.rs, PlanStepKind::RaiseEvent, lines ~360–420):
    
    - The plan evaluates the step expression to an ExprValue (the same enum used by the expression engine).
    - It then **canonicalizes and validates** that value against the reducer’s event schema (canonicalize_literal + validate_literal using metadata.event_schema from the manifest).
    - After canonicalization, it converts back to an ExprValue and then to CBOR via expr_value_to_cbor_value, and finally serializes to bytes with serde_cbor::to_vec.
    - Those bytes are sent unchanged to the kernel as the event payload.
    - Result: plans emit **schema-shaped, canonical CBOR** (not the externally-tagged {"Record":{…}} form), guaranteed to match the reducer’s event schema.
- **Await event path** (plan.rs::deliver_event, lines ~700): when a plan is waiting on an event, it tries to decode the incoming payload as ExprValue (externally tagged); if that fails it falls back to treating it as raw bytes. This is mainly for where-clauses; it doesn’t alter what the reducer saw.
    
- **Kernel routing**: the kernel still doesn’t re-encode events. For keyed routing/correlation it decodes with serde_cbor::from_slice::<ExprValue> to extract key_field (so this decode succeeds because the plan emitted schema-shaped CBOR that also deserializes into ExprValue’s primitives/records). After routing, it forwards the **original bytes** to the reducer unchanged.
    

Implication:

- When events originate from plans, payloads are already canonical to the schema, so reducers that expect the schema-shaped CBOR will decode fine—keyed or not.
- When events come from outside (CLI/tests), the kernel does no normalization; callers must send either schema-shaped CBOR (for reducers) and, if keyed, in a form that ExprValue can parse for the key. External injections using the tagged ExprValue form will route fine but will only decode in reducers that can handle that tagged shape.

So plans provide the “normalization” step: they canonicalize to the event schema before emitting, but the kernel itself never rewrites the payload en route to the reducer.

---

# Approach

Short version:
Treat **events like effect params**: normalize them once at the kernel boundary using their schema, store the normalized bytes in the journal, and always deliver those same bytes to reducers/plans. Use schema‑aware decoding for routing/correlation, not `Value`/`ExprValue` CBOR shapes.

Below I’ll walk through what that means concretely and how it helps reducer ergonomics.

---

## 1. What invariants we want for events

We already say:

* Journal events are canonical CBOR.
* All reducer state and event values should be canonical CBOR of their declared schemas.
* DomainEvents / ReceiptEvents should be declared as `defschema` entries.
* Effect params already go through a **decode → schema‑check → canonicalize → re‑encode** pipeline before being stored and hashed.

So the natural extension is:

> **Every DomainEvent / ReceiptEvent payload is canonicalized once, at ingestion, using its `defschema`. The journal stores *only* the canonical bytes.**

This gives you:

* deterministic replay (already required)
* a single, ergonomic event shape for reducers (no “Value vs struct” split)
* schema‑aware routing & correlation (for keyed reducers / `await_event`)

---

## 2. Where schemas come from (so we *can* normalize)

We actually do have schemas everywhere we need them:

* **Events *to reducers***: `defmodule.abi.reducer.event` is a `SchemaRef`. It’s explicitly “domain/receipt event type family”.
* **Domain events *from reducers***: they carry `schema: Name` and `value: Value`. The Name must correspond to a `defschema` in the manifest, and triggers reference those same Names as `triggers[].event`.
* **Receipt events**: built‑ins like `sys/TimerFired@1`, `sys/BlobPutResult@1`, etc., are also `defschema`s with fully specified shapes.

So the “wrinkle” about “reducer returned events are nowhere specified” is mostly historical: in the current spec, **all** DomainEvents/ReceiptEvents are supposed to be backed by `defschema`s. It’s reasonable to make that a hard requirement, not just “should”.

---

## 3. Proposed kernel semantics for events

### 3.1 Ingestion pipeline (one normalizer)

Introduce an **Event Normalizer** in the kernel, analogous to the Effect Manager’s param normalizer. For *every* runtime event that crosses the boundary into the world (and into the journal):

1. **Determine schema:**

   * If it is a *DomainEvent emitted by a reducer*:

     * Use `DomainEvent.schema` (Name) → look up `defschema` in manifest.
   * If it is a *ReceiptEvent synthesized by the kernel*:

     * Use its known built‑in schema (e.g. `sys/TimerFired@1`) from manifest.
   * If it is a *plan → reducer event (raise_event)*:

     * Use `defmodule.abi.reducer.event` for the target reducer. 
   * If it is *externally injected (CLI/tests)*:

     * The caller must specify which event schema it is (or we deduce it from routing/trigger config).

2. **Decode payload using its schema**
   Use the same type system & canonicalization rules as the loader (records, variants, maps, sets, numeric normalization, etc.).

3. **Canonicalize**
   Re‑encode to canonical CBOR per the AIR rules (deterministic map/set ordering, etc.).

4. **Persist canonical bytes**
   Those canonical bytes become **the only form** of the event payload that goes into the journal, and the only form delivered to reducers and plans.

If decoding/canonicalization fails → reject the event (same as we already do for mis‑shaped effect params).

> Effectively: **events get the same treatment as effect params and receipts.** Journal = canonical, schema‑checked world.

### 3.2 Delivery to reducers

Reducer ABI already says `event` is canonical CBOR of a DomainEvent or ReceiptEvent. Under the above rule:

* The host side of `StepInput<State, Event>` just deserializes those canonical bytes into the user’s `Event` enum/struct. No `ExprValue`/tagged funniness. 
* For **keyed reducers (cells)**, the kernel still wraps in a small envelope with optional key as defined in the cells spec; the payload inside that is already canonical.

From reducer authors’ POV, the rule becomes:

> “If your `Event` type matches the declared event schema, deserialization just works; you never see `Value` tags.”

That’s the ergonomics you want.

### 3.3 Delivery to plans (`await_event`)

Plans consume domain events as typed values too:

* When a plan does `await_event`, it registers interest in events with a particular schema (`event: SchemaRef`).
* When an event arrives:

  * Kernel decodes the already‑canonical payload to a typed value using that schema.
  * It then projects this into the expression engine’s value type for `@event` (much like we already do when evaluating constants/records).
  * The `where` predicate runs over that typed view; no need to re‑interpret ad‑hoc `ExprValue` CBOR.

We keep the same semantics (`await_event` is future‑only, broadcast, first‑match).

### 3.4 Routing / correlation for keyed reducers and triggers

Right now, routing uses “decode as `Value` and pluck a field”, which creates the Value‑tagged vs schema‑shaped mismatch you hit.

With canonical, schema‑aware events we can instead:

* Each `routing.events[]` entry says:
  `event: SchemaRef` and, for cells, `key_field: "order_id"` (or a dotted path).
* For a new event:

  1. Look up its schema (as above).
  2. Decode to typed value.
  3. Navigate the `key_field` path in the typed value.
  4. Assert that the resulting value is compatible with the reducer’s `key_schema`.
  5. Route to the appropriate cell.

This completely decouples routing from `ExprValue`’s CBOR shape. Event payloads are always “the schema‑shaped thing”, not “maybe Value‑tagged, maybe not”.

---

## 4. Where normalization happens vs where it *doesn’t*

To your specific questions:

> Should we normalize them according to the plan or module schema?

* **Yes, normalize according to the *event’s* `defschema` (or reducer event schema) – not the Value/Expr types.**
* For plan→reducer events, that’s the same thing we already say in the spec: kernel infers the payload schema from the reducer’s manifest entry and validates/canonicalizes the event before emitting.
* For reducer→plan / reducer→reducer / external injection, normalize using the event schema named in the DomainEvent (or the schema used by the trigger/routing entry).

> Should we store normalized events, or at least validate them, or if we normalize, do it only on the fly…?

Recommend:

* **Store normalized events in the journal.** Do the decode+canonicalize once, at ingestion, then persist that.

  * This matches what we already do for effect params: the canonical bytes are “the only form stored, hashed, and dispatched.”
  * It makes offline tooling simpler: anything that tails the journal sees one stable representation.
* **No on‑the‑fly rewriting on replay** (beyond decoding). Replay just reads canonical bytes and feeds them back into reducers/plans as‑is, just like state snapshots.
* Validation is implicitly part of canonicalization: if the value doesn’t match the schema, canonicalization fails and we reject the event.

If you ever care about “exact author bytes” for debugging, that’s a separate optional logging concern, not something the kernel needs to treat as authoritative.

---

## 5. Reducer ergonomics with this model

With the above invariants, reducer authors get a much nicer story:

1. **One event type:**

   * In manifest: `abi.reducer.event` points at a schema (often a variant of domain/receipt events).
   * In code, they declare a corresponding Rust enum `Event` with `#[derive(Deserialize)]`.
   * SDK glue (`StepInput<State, Event>`) deserializes the canonical CBOR into that enum directly.

2. **No awareness of Value/Expr CBOR:**
   They never have to know whether upstream sent Value‑tagged CBOR, JSON sugar, canonical JSON, etc. All of that is normalized away before it hits the reducer.

3. **Keyed vs non‑keyed doesn’t affect event shape**:
   Keyed reducers just get an extra key envelope; their `Event` type is the same for keyed and non‑keyed modes.

4. **DomainEvents they emit are also schema‑checked**:
   When a reducer returns `{ schema: "com.acme/ChargeRequested@1", value: <Value> }`, the kernel:

   * validates `value` against that `defschema`
   * canonicalizes it
   * appends the canonical bytes
   * triggers any matching plans.

   If they emit bogus payloads, they get a clear error instead of silently poisoning the journal.

5. **Versioning story stays clean**:
   Because DomainEvents/ReceiptEvents are versioned by Name and backed by `defschema`, reducers can pattern‑match on schema versions (e.g., support both `PaymentResult@1` and `@2`) while still counting on canonical payloads.

---

## 6. What happens to Value‑tagged CBOR?

With this design:

* **`ExprValue` / Value‑tagged CBOR becomes purely an *in‑memory* / authoring concern:**

  * Plans evaluate expressions to values; those are then canonicalized by schema before becoming events or effect params.
  * CLI/tests are allowed to use expression‑flavored JSON/CBOR as sugar, but the ingestion API treats it like any other “authoring lens”: decode via the declared schema, canonicalize, and write canonical bytes.

* Binary events in the journal are **never** Value‑tagged; they’re always schema‑shaped.

So the earlier failure mode (“we injected ExprValue::Record, routing worked, reducer expected struct CBOR and panicked”) simply can’t happen in a properly wired system: the normalizer would reject the mismatched shape before it’s routed, or canonicalize it into the struct shape the reducer expects.

---

## 7. TL;DR recommendation

**Kernel/journal behavior**

* Treat DomainEvents/ReceiptEvents exactly like effect params:

  * **Decode → validate against schema → canonicalize → re‑encode**, on ingress.
  * Store and replay **canonical** bytes only.
* Use **schema‑aware decoding** for:

  * keyed routing (`key_field` + `key_schema`)
  * plan triggers & `await_event` `where` predicates.

**Reducer ergonomics**

* Reducers always see canonical, schema‑shaped events that deserialize straight into their `Event` enum/struct, independent of how the event was authored or injected.
* DomainEvents they emit are also schema‑checked/canonicalized, so downstream consumers (plans or other reducers) see the same shape.

If you implement this normalizer once and push *all* event ingress paths through it (plans, reducers, external tools), the “too much parsing magic in every reducer” problem goes away: reducers just deal with typed events, and the kernel/journal guarantee the canonical, schema‑correct representation.

---

# Implementation Plan


All events should enter the world through a single schema-driven normalizer, then be stored and replayed as canonical CBOR. Current implementation status + remaining steps:

**Done**
- Shared normalizer: `value_normalize` added in `aos-air-types`; used by the kernel to canonicalize every `DomainEvent` on ingress.
- Ingestion wiring: `process_domain_event`/journal replay now store canonical bytes only; reducer outputs/plan raise emit canonical payloads.
- Routing/correlation: keyed routing pulls `key_field` via schema-aware decode; plan triggers use typed correlation; plan `await_event` decodes by schema for `@event`/where.
- Reducer ergonomics: reducers receive canonical schema-shaped events; bad payloads are rejected early; tests cover keyed routing, await_event predicates, and invalid payload rejection.

**Remaining**
- Enforce at manifest load: fail manifests where domain/receipt event schemas aren’t resolvable (loader + spec/docs update).
- External ingress: CLI/tests (`aos-host` helpers) should require explicit event schema and run through the normalizer; receipts synthesized by adapters should go through the same path.
- Spec/docs: update specs/AGENTS with “events like effect params” invariant and the canonical-journal rule.

**Stretch**
- Consider re-exporting the normalizer for external tools and adding richer diagnostics (author bytes vs canonical) if needed.
