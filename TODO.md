# TODO

When done with a todo, mark it as done, and a quick note what you achieved.

## Next Steps


## Context

## 1. What common.schema.json is actually doing

If you look at it, there are three kinds of things mixed together: 

1. **Core vocabulary / type system**

   * `Name`, `Hash`, `SchemaRef`
   * `TypeExpr`, `TypePrimitive`, `TypeComposite`
   * `Expr`, `ExprOrValue`, `Value`, etc.

2. **Reusable “shapes” for fields that repeat**

   * `VarName`, `StepId`, `CapGrantName`, `BytesB64`, `DecimalString`
   * `CapGrant` itself

3. **Built‑in catalog stuff**

   * `EffectKind` (enum of `"http.request" | "blob.put" | "blob.get" | "timer.set" | "llm.generate"`)
   * `CapType` (enum of `"http.out" | "blob" | "timer" | "llm.basic"`)

Only (3) is really contentious. The rest is just “please don’t copy/paste regexes in six schemas.”

---

## 2. Are things like StepId / CapGrantName “necessary”?

Strictly speaking: no. They’re just **named string patterns**:

* `StepId` / `VarName` / `CapGrantName` are all `"^[A-Za-z_][A-Za-z0-9_.-]{0,63}$"`. 
* You *could* inline that regex everywhere in defplan/manifest instead of factoring it out.

Reasons *to* keep them:

* They give semantic names to what would otherwise be anonymous regexes.
* If you ever change the naming rule (e.g., allow `:`), you change it once.
* They make the higher‑level schemas cleaner and more readable (`StepId` is self‑explanatory).

They don’t really couple you to anything deep; they’re just DRY + documentation. I’d personally keep those in *some* shared schema file.

---

## 3. The real “iffy” part: EffectKind & CapType

You’re specifically worried about:

> "EffectKind", CapType … some of them, like effect kind will change too much, no? why even build this in?

Your concern is very legit: **Effect kinds and capability types are “catalog entries,” not fundamentals of the type system.** They’re the part most likely to grow or evolve (especially once you add WASM-based adapters).

Why they were baked in:

* They’re referenced all over:

  * `defplan.allowed_effects[]` uses `EffectKind` 
  * `defmodule.abi.reducer.effects_emitted[]` uses `EffectKind` 
  * `defpolicy.rules[].when.effect_kind` uses `EffectKind` 
  * `defcap.cap_type` uses `CapType` 
* The v1 story is “there is a small, fixed built‑in set”:

  * Effects: `http.request`, `blob.put`, `blob.get`, `timer.set`, `llm.generate` 
  * Capability types: `http.out`, `blob`, `timer`, `llm.basic` 
* The kernel / adapters / policy gate all *assume* these names to attach special semantics (budgets, params, receipts).

So in v1, `EffectKind` and `CapType` are effectively **“keys into the kernel’s hard-coded catalog.”** From that perspective, making them enums is a way to:

* Catch typos at JSON‑schema level instead of at runtime.
* Make it crystal‑clear that *custom effect kinds are not yet a thing*; there is only the built‑in set.

But the downside you’re feeling is also real:

* As soon as you want **user‑defined effect kinds or WASM-based adapters**, the `enum` in common.schema.json becomes wrong or needs to be weakened.
* It pollutes the “foundational” schema with something that’s really “the v1 built-in catalog”.

---

## 4. So… should EffectKind / CapType live here at all?

If you want **common.schema.json to be a clean, future-proof foundation**, I’d do this split:

### A. Keep common.schema.json purely “structural”

Leave only:

* Identity & hashing (`Name`, `Hash`, `SchemaRef`)
* Type system (`TypeExpr`, `TypePrimitive`, `TypeComposite`)
* Expression system (`Expr`, `ExprOrValue`, `Value`, etc.)
* Generic string types (`VarName`, `StepId`, `CapGrantName`, …)
* Common structs (`CapGrant`, etc.)

And change:

* `EffectKind` → **plain string or pattern**, not an enum.
* `CapType` → same.

For example:

```json
"EffectKind": {
  "title": "Effect kind",
  "type": "string",
  "pattern": "^[a-z][a-z0-9_.-]*(\\.[a-z0-9_.-]+)*$"
}
```

```json
"CapType": {
  "title": "Capability type",
  "type": "string",
  "pattern": "^[a-z][a-z0-9_.-]*(\\.[a-z0-9_.-]+)*$"
}
```

Then you move the **closed list of built‑ins** into:

* The prose spec (already there), and/or
* A separate `builtin-*.schema.json`, or
* Semantic validation in the kernel: “if effect_kind is one of the five built‑ins, enforce these param schemas; otherwise treat as adapter-defined.”

That way:

* `common.schema.json` is stable and version‑agnostic.
* The “effect catalog” becomes **data** (like `builtin-schemas.air.json`) and kernel semantics, not baked into the core schema. 

### B. Or, if you want a hybrid

If you want to keep tight checking for built‑ins *and* leave space for future custom kinds, you can schema it as:

```json
"EffectKind": {
  "anyOf": [
    { "enum": ["http.request","blob.put","blob.get","timer.set","llm.generate"] },
    { "pattern": "^[a-z][a-z0-9_.-]*(\\.[a-z0-9_.-]+)*$" }
  ]
}
```

Then:

* v1 tooling can still special‑case the five built‑ins.
* Future adapters can mint their own effect kinds without changing common.schema.json again.

Same pattern for `CapType`.

---

## 5. Why bother with these at all instead of raw strings?

Short answer: **authoring and tooling ergonomics.**

Without shared defs like `StepId`, `CapGrantName`, etc.:

* You end up copy-pasting regexes into `defplan`, `manifest`, `defpolicy` schemas.
* Any change to naming rules becomes scattered and brittle.

Without *some* notion of EffectKind / CapType:

* You either give up schema‑level checking entirely, or you have to push all validation into custom code.
* Tools like `air fmt`, `air diff`, or UIs can’t easily know what effect kinds exist or offer autocomplete / refactor help.

So there is value in naming these things; the question is only **at what level** you freeze the set of possible values.

---

## 6. My recommendation

If your goal is “clean long‑term foundation”:

1. **Keep**

   * `Name`, `Hash`, `SchemaRef`, `TypeExpr`, `Expr`, `Value`
   * `VarName`, `StepId`, `CapGrantName`, `CapGrant`, etc.
     These are just structural vocabulary and aren’t really a problem.

2. **Relax / move** the catalog bits:

   * Change `EffectKind` and `CapType` from closed enums to “well-formed string”, as above.
   * Treat the five built‑in effects and four cap types as:

     * documented in the AIR spec, and
     * backed by canonical `defschema`s in `builtin-schemas.air.json`, and
     * enforced semantically in the kernel / adapters.

3. Optionally, introduce a **separate** schema file like `builtin.catalog.schema.json` that has enums of the current built‑ins for tools that want the strict list, without polluting the core foundation.

That keeps v1 fully working, but it stops `common.schema.json` from pretending that “the set of effects in the universe is {http.request, …}”.

