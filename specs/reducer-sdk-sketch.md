# Reducer SDK v0 (Draft)

**Status**: draft • **Audience**: `aos-wasm-sdk` authors and reducer authors

This document proposes the **first shared SDK surface** for writing AgentOS reducers in Rust.
It removes the repeated `alloc`, `step`, CBOR plumbing, and ad-hoc intent/effect builders that
currently appear across the examples. It aligns with the reducer ABI (`step(ptr,len)->(ptr,len)`),
the micro-effects boundary, and manifest-driven orchestration.

> *Non-goals*: invent a new reducer programming model, change the reducer ABI, or grow the set of
> reducer-allowed effects. This is ergonomics and safety, not a new runtime.

---

## 0. Terms & Constraints (from the core specs)

- Reducers are deterministic WASM state machines. They consume events and return a new state plus
  optional domain events and **micro-effects** only. Heavy orchestration belongs in plans.  
- ABI export is **`step(ptr,len) -> (ptr,len)`**; inputs/outputs are canonical CBOR envelopes.
- Allowed micro-effects from reducers in v1: `blob.put`, `blob.get`, `timer.set`. Reducers must emit
  **at most one effect per step**.

*(The SDK bakes these rules in and makes the common path hard to misuse.)*

---

## 1) Proposed surface

We expose a single trait, a small context type, and a derive-style proc macro. The macro generates
the exported `step` function, all CBOR glue, and a per-call bump allocator so reducers stop
re-implementing the `alloc` export. **The macros live in `aos-wasm-sdk` itself** (no sibling crate).

### 1.1 `Reducer` trait

```rust
pub trait Reducer {
    /// Durable reducer state; canonicalized via serde_cbor at the ABI.
    type State: serde::de::DeserializeOwned + serde::Serialize + Default;

    /// The event family this reducer consumes.
    /// Recommend a closed enum that mirrors your defschema variant.
    type Event: for<'de> serde::Deserialize<'de>;

    /// Optional annotation payload (for observability). Most reducers set this to `serde_cbor::Value`.
    type Ann: serde::Serialize;

    /// Core reducer logic. Mutate `ctx.state`, and use `ctx.intent(..)` / `ctx.effects()` to emit.
    /// Return `Ok(())` for normal progress; return `Err(..)` only for irrecoverable contract bugs
    /// (schema mismatch, invariant violation). Runtime treats Err as a deterministic module fault.
    fn reduce(&mut self, event: Self::Event, ctx: &mut ReducerCtx<Self::State, Self::Ann>) -> Result<(), ReduceError>;
}
```

### 1.2 `aos_reducer!(MyReducer)` helper macro

Invoke the declarative macro with the reducer type after the impl. The macro expands to:

- `#[no_mangle] pub extern "C" fn step(ptr: i32, len: i32) -> (i32, i32)`
- An internal **bump allocator** for the output buffer, exported as `#[no_mangle] extern "C" fn alloc(len: i32) -> i32`
  so hosts can reuse it for inputs. (No more per-example alloc shims.)
- Canonical CBOR decode of `{ state, event }` into `R::State`/`R::Event`
- Invocation of `R::reduce`
- Enforcement of **“at most one effect per step”**
- Canonical CBOR encode of `{ state, domain_events?, effects?, ann? }`
- Return `(ptr,len)` for the host to read

```rust
pub struct OrderSm;

impl Reducer for OrderSm {
    type State = State;
    type Event = Event;
    type Ann = serde_cbor::Value;

    fn reduce(&mut self, event: Event, ctx: &mut ReducerCtx<State, Self::Ann>) -> Result<(), ReduceError> {
        match (ctx.state.pc, event) {
            (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
                ctx.state.order_id = order_id.clone();
                ctx.state.amount_cents = amount_cents;
                ctx.state.pc = Pc::AwaitingPayment;

                // Emit a DomainIntent to start a plan
                ctx.intent("com.acme/ChargeRequested@1")
                    .key_bytes(order_id.as_bytes())
                    .payload(&cbor::cbor!({ "order_id": order_id, "amount_cents": amount_cents }))
                    .send();
            }
            (Pc::AwaitingPayment, Event::PaymentResult { ok, txn_id }) => {
                if ok {
                    ctx.state.pc = Pc::Done;
                    ctx.state.txn_id = Some(txn_id);
                } else {
                    ctx.state.pc = Pc::Failed;
                }
            }
            _ => {} // idempotent ignore
        }
        Ok(())
    }
}

aos_reducer!(OrderSm);
```

### 1.3 `ReducerCtx`

```rust
pub struct ReducerCtx<S, A = serde_cbor::Value> {
    pub state: S,
    pub ann: Option<A>,

    // read-only metadata for future-proofing (Cells, tracing, etc.)
    key: Option<Vec<u8>>,
    reducer_name: &'static str,

    // private: output buffers
    domain_events: Vec<DomainEvent>,
    effects: Vec<ReducerEffect>,
    effect_used: bool, // guard: at most one effect per step
}

impl<S, A> ReducerCtx<S, A> {
    /// Access the optional cell key (reserved for v1.1 keyed reducers).
    pub fn key(&self) -> Option<&[u8]> { self.key.as_deref() }

    /// Builder for a domain intent/event.
    pub fn intent(&mut self, schema: &'static str) -> IntentBuilder<'_> { /* ... */ }

    /// Typed helpers for micro-effects; enforces "one effect" at emit time.
    pub fn effects(&mut self) -> Effects<'_> { /* ... */ }

    /// Attach structured annotations (observability).
    pub fn annotate(&mut self, a: A) { self.ann = Some(a); }
}
```

### 1.4 Builders

Minimal, zero-cost builders around CBOR values so reducers don’t hand-roll envelopes:

```rust
pub struct IntentBuilder<'a> { /* … */ }
impl<'a> IntentBuilder<'a> {
    pub fn key_bytes(self, k: &[u8]) -> Self { /* … */ }
    pub fn payload<T: serde::Serialize>(self, v: &T) -> Self { /* … */ }
    pub fn send(self) { /* pushes DomainEvent */ }
}

pub struct Effects<'a> { /* … */ }
impl<'a> Effects<'a> {
    pub fn timer_set(self, params: &TimerSetParams, cap_slot: &str) -> Result<(), TooManyEffects>;
    pub fn blob_put(self, params: &BlobPutParams, cap_slot: &str) -> Result<(), TooManyEffects>;
    pub fn blob_get(self, params: &BlobGetParams, cap_slot: &str) -> Result<(), TooManyEffects>;
    /// Escape hatch for future micro-effects:
    pub fn emit_raw(self, kind: &'static str, params: &serde_cbor::Value, cap_slot: Option<&str>) -> Result<(), TooManyEffects>;
}
```

The guard trips if a reducer tries to emit more than one effect in a single step.

---

## 2) Event typing (and when routers enter)

- **Recommendation**: Model `type Event` as a Rust enum mirroring your `defschema` variant for the
  reducer’s event family. The macro does a single `serde_cbor` decode to that enum.
- For reducers that truly need to handle heterogeneous schemas, define an explicit enum with
  variants named after schema tags (e.g., `SysTimerFired`, `PaymentResultV1`, …).
- Keep v0 simple: most reducers can (and should) rely on a single typed enum for compile-time
  exhaustiveness and minimal runtime dispatch. See §10 for the router extension we can add once a
  reducer legitimately needs to juggle many unrelated schemas or multiple schema versions at once.

---

## 3) Safety rails baked into the SDK

1. **Micro-effects only**: Provide typed helpers for `timer.set`, `blob.put`, `blob.get`. There is no
   helper for network/LLM effects; these must be raised via a plan.
2. **One effect per step**: `ReducerCtx` enforces this at the call site (first wins; subsequent calls
   error with `TooManyEffects` and the macro turns it into a deterministic module fault).
3. **Canonical CBOR everywhere**: Decode/encode via `serde_cbor` in the macro; reducers never touch
   raw byte envelopes directly.
4. **Cell-key future-proofing**: `ReducerCtx::key()` exposes the optional key the kernel will provide
   once keyed reducers (Cells) are enabled; no API churn later.
5. **Allocator built-in**: The proc macro exports an `alloc` that backs both input copies and output
   buffers, so reducer crates delete their bespoke allocator glue immediately.

---

## 4) End-to-end example (abridged)

```rust
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct State { pc: Pc, order_id: String, amount_cents: u64, txn_id: Option<String> }

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy)]
enum Pc { Idle, AwaitingPayment, Done, Failed }
impl Default for Pc { fn default() -> Self { Pc::Idle } }

#[derive(serde::Deserialize)]
enum Event {
    OrderCreated { order_id: String, amount_cents: u64 },
    PaymentResult { order_id: String, ok: bool, txn_id: Option<String> },
}

struct OrderSm;

impl Reducer for OrderSm {
    type State = State;
    type Event = Event;

    fn reduce(&mut self, ev: Event, ctx: &mut ReducerCtx<State>) -> Result<(), ReduceError> {
        match (ctx.state.pc, ev) {
            (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
                ctx.state.order_id = order_id.clone();
                ctx.state.amount_cents = amount_cents;
                ctx.state.pc = Pc::AwaitingPayment;

                ctx.intent("com.acme/ChargeRequested@1")
                    .key_bytes(order_id.as_bytes())
                    .payload(&cbor::cbor!({ "order_id": order_id, "amount_cents": amount_cents }))
                    .send();
            }
            (Pc::AwaitingPayment, Event::PaymentResult { ok, txn_id, .. }) => {
                if ok { ctx.state.pc = Pc::Done; ctx.state.txn_id = txn_id; }
                else   { ctx.state.pc = Pc::Failed; }
            }
            _ => {}
        }
        Ok(())
    }
}

aos_reducer!(OrderSm);
```

---

## 5) Crate layout

- `aos-wasm-sdk`  
  Core types: `Reducer`, `ReducerCtx`, `DomainEvent`, `ReducerEffect`, builders, error types, and the
  `aos_reducer!` macro + allocator shim (implemented with declarative macros inside the crate to
  avoid yet another package).

---

## 6) Open questions (intentionally left for v0+1)

- **Event routers**: If we see recurring “match schema string, then decode” patterns, add an opt-in
  `Router<T>` helper that maps schema → decode fn.
- **Custom alloc export tweaks**: If hosts eventually prefer a separate `alloc` signature or sizing
  hint, we can adjust the macro-backed allocator without touching reducer code.
- **Typed cell keys**: When key schemas land, consider `type Key` on the trait with `ctx.key::<K:Deserialize>()`.
- **Annotations ergonomics**: Add a tiny `ctx.trace(k,v)` facility if structured logs become common.

---

## 7) Why this over ad-hoc shims? (design notes)

- **Ergonomics**: No more per-crate `step` wrappers or CBOR dance; reducers implement one trait method.
- **Correctness**: Single enforcement point for micro-effects + single-effect rule.
- **Future-proofing**: Cell keys and richer annotations won’t require a trait break.
- **Explicit typing**: Events are enums, not untyped `Value`; decoding happens once per call.
- **Allocator solved**: Every reducer exports the same `alloc` automatically, so hosts and examples
  stop copying the existing shim.

---

## 8) Migration of examples (tracking for this repo)

- `00-counter`: adopt macro + trait; zero effects, no intents.
- `02-blob-echo`: use `ctx.effects().blob_put/get(..)`; remove manual `Value` builders.
- `03-fetch-notify`: emit intent via `ctx.intent(..)`; no hand-rolled envelopes.
- `04-aggregator`: model input/result as enums; drop bespoke event visitors.

(Ports are mechanical once the SDK is in place.)

---

## 9) Appendix: minimal SDK types (sketch)

```rust
pub struct DomainEvent {
    pub schema: &'static str,
    pub value: serde_cbor::Value,
    pub key: Option<Vec<u8>>,
}

pub struct ReducerEffect {
    pub kind: &'static str,                 // "timer.set" | "blob.put" | "blob.get"
    pub params: serde_cbor::Value,          // canonical value
    pub cap_slot: Option<&'static str>,     // abstract slot name
}

#[derive(Debug)]
pub struct ReduceError { pub msg: &'static str }
impl ReduceError { pub const fn new(msg: &'static str) -> Self { Self { msg } } }

pub struct TooManyEffects;
```

---

## 10) Router extension (v0+1 preview)

The v0 SDK deliberately omits a router to keep the first trait/macro surface small. Most reducers can
express their full input family as **one typed enum** (mirroring a `variant` schema) and rely on
serde to pick the right alternative. Still, there are reducers that must juggle **many unrelated
schemas** or accept **multiple schema versions** at once. For those, we can ship an opt-in router in
the next phase. This section records the guidance so implementers know when to reach for it.

### TL;DR

- **Don’t use a router** when you can define one typed event enum and decode directly into it. That’s
  the default in v0 and matches AIR’s typed, canonical boundaries.
- **Add a router** when the reducer must accept many unrelated schemas (including built-in receipt
  events) or multiple schema versions simultaneously and you either cannot or do not want to express
  that union as a single `variant` schema. A router maps `schema: Name` → decoder/handler so each
  schema can have custom fallbacks/migrations before reaching `Reducer::reduce`.

### What problem does the router solve?

Reducers expose a single `step(ptr,len)->(ptr,len)` ABI where inputs/outputs are canonical CBOR.
AIR currently pins one event schema per reducer; values must match it exactly. A router gives reducers
an optional library helper that:

1. Looks at the incoming **schema name** (e.g., `com.acme/PaymentResult@1`, `sys/TimerFired@1`).
2. Chooses the right decoder/handler for that schema (including fallback logic) and converts it into
   the reducer’s internal event type before invoking `reduce`.

Because AIR already enforces canonical forms, most reducers will never need this indirection—but it
helps in the edge cases below.

### When you **don’t** need a router

Stick with a typed enum (the v0 default) when:

- You control the event family and can model it as a **closed sum** (`variant`) referenced by
  `defmodule.abi.reducer.event`.
- You are not straddling incompatible schema versions simultaneously.
- You are not aggregating a zoo of unrelated system/domain events.
- You want **exhaustiveness checking** at compile time with zero runtime dispatch.

### When a router **is warranted**

Reach for the router if any of these apply:

1. **Multi-source, heterogeneous events**: The reducer consumes built-in receipt schemas
   (`sys/TimerFired@1`, `sys/BlobPutResult@1`, …) plus multiple domain schemas and you want modular
   handler registration instead of one mega-enum.
2. **Version straddling during migrations**: You must accept `@1` and `@2` of an event concurrently
   and normalize them into a unified internal shape. AIR’s v1 exact-schema matching makes this easier
   with a router than with umbrella variants.
3. **Third-party or cross-package schemas**: Producers live elsewhere and you cannot easily maintain
   a central union schema. The router lets you plug handlers per schema without owning a variant.
4. **Fallback decoding / legacy shapes**: Even though runtime inputs are canonical, you might need to
   ingest historical CBOR or apply special-case transforms. A router can attempt multiple decoders per
   schema.
5. **Plugin-style registration**: Feature-gated or optional crates can register handlers via a fluent
   API without editing a central enum.

> The router does **not** change reducer responsibilities: reducers stay deterministic, emit only
> micro-effects (still capped at one per step), and kick heavier work to plans via intents.

### Decision checklist

Choose **typed enum only** if:

- You can describe all inputs as one `variant` schema.
- You do not need version shims or fallback decode paths.
- You prefer compile-time exhaustiveness and zero extra machinery.

Choose **router** if **any** are true:

- Many unrelated schemas feed the reducer (including system receipts).
- You must accept multiple versions simultaneously and normalize them.
- Maintaining an umbrella `variant` schema is impractical.
- You need fallback decoders or migration transforms.

### Minimal sketches

**No router (preferred default)**

```rust
#[derive(Deserialize)]
enum Event {
    PaymentResultV1(PaymentResultV1),
    Timer(sys::TimerFiredV1),
}

struct MyReducer;

impl Reducer for MyReducer {
    type State = State;
    type Event = Event;

    fn reduce(&mut self, ev: Event, ctx: &mut ReducerCtx<State>) -> Result<(), ReduceError> {
        match ev {
            Event::PaymentResultV1(p) => { /* … */ }
            Event::Timer(t) => { /* … */ }
        }
        Ok(())
    }
}

aos_reducer!(MyReducer);
```

**With a router (opt-in helper for v0+1)**

```rust
let mut router = Router::new()
    .on("com.acme/PaymentResult@1", |v| decode_v1(v).map(Event::Payment))
    .on("com.acme/PaymentResult@2", |v| decode_v2_then_convert(v).map(Event::Payment))
    .on("sys/TimerFired@1",        |v| serde_cbor::from_value(v).map(Event::Timer));

let event = router.route(input_schema_name, input_value_cbor)?;
self.reduce(event, ctx)?;
```

This keeps migration logic localized while preserving the reducer ABI and the SDK’s safety rails.
We can land the helper behind a feature flag once a real reducer meets the router criteria.

---

**Call for review**: If we like this direction, the next step is to land `aos-wasm-sdk`
scaffolding and port the numbered examples. Feedback welcome on trait shape, builder names,
and whether the embedded allocator needs additional knobs before we codify it.
