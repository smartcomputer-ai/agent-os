Below is a **self‑contained, authoritative update** to AIR that you can drop into the repo. It covers: (A) language & wire‑format clarifications; (B) schema changes (including the requested **`ExprOrValue`**); (C) updates to built‑in schemas; (D) implementation guidance; and (E) a testing matrix. Citations point to the current spec/architecture/schemas for continuity and justification.

---

# AIR v1 — Text Lenses, `ExprOrValue`, Built‑ins, and Canonicalization

**Status:** Draft for adoption (v1)
**Scope:** Text format, loader behavior, hashing, JSON Schemas, and built‑ins
**Motivation:** Keep authoring pleasant for humans, precise for agents, and deterministic for the kernel. All persisted identity remains **canonical CBOR**, as in v1. 

---

## A. Text Lenses & Canonicalization (Normative)

AIR values may be presented in **two JSON lenses** that are **interchangeable** at load time:

1. **Authoring sugar (default for humans)** — plain JSON interpreted **schema‑directedly** by the loader.
2. **Canonical JSON (tagged)** — a fully specified, lossless overlay that carries explicit type tags for every literal; ideal for inspection, diffs, and agent‑authored patches.

> The loader **MUST** accept either lens wherever a typed value is expected, convert to a typed internal value, and then emit **canonical CBOR** for hashing and execution. Hashing **MUST** remain bound to the value’s **schema hash**, as already specified.  

### A.1 Authoring sugar (schema‑directed)

Representative forms (the loader uses surrounding schema refs to disambiguate):

* **Primitives**

  * `bool`: `true|false`
  * `int|nat`: JSON number (or string for large values)
  * `dec128`: JSON string (valid decimal)
  * `text`: string
  * `bytes`: base64 string
  * `time|duration`: RFC 3339 / ISO strings, or integers in ns
  * `hash`: `"sha256:<64-hex>"`
  * `uuid`: RFC‑4122 string
    (Patterns/types as in `common.schema.json`.) 

* **Composites**

  * `record`: JSON object by field name
  * `list`: JSON array (order preserved)
  * `set`: JSON array (**dedupe + sort** during canonicalization)
  * `map<text,V>`: JSON object (key order irrelevant)
  * `map<K,V>` (non‑text keys): JSON array of `[key, value]` pairs
  * `variant`: `{ "Tag": <value?> }` (omit value/`null` for unit)
  * `option<T>`: nested value for `some`, `null` for `none`

### A.2 Canonical JSON (tagged overlay)

Every literal is explicitly tagged so values can round‑trip without schema context:

```
{ "text": "hello" }
{ "nat": 42 }
{ "dec128": "0.25" }
{ "time_ns": 1704067200000000000 }
{ "bytes_b64": "AAEC" }
{ "list": [ { "text": "a" }, { "text": "b" } ] }
{ "record": { "id": { "nat": 7 }, "flags": { "set": [ { "text": "paid" } ] } } }
{ "variant": { "tag": "Ok", "value": { "text": "done" } } }
{ "map": [ { "key": { "uuid": "…" }, "value": { "nat": 1 } } ] }
{ "option": null }   // none
{ "option": { "text": "some!" } } // some
```

> This tagged form mirrors the existing `ExprConst` tags, extended to composites, and is the preferred machine lens for **diffs/patches/inspector output**. The kernel still persists and signs **only canonical CBOR**.  

### A.3 Canonicalization rules (sugar ➝ typed ➝ CBOR)

The loader **MUST** enforce:

* **CBOR determinism**: deterministic map ordering (bytewise order of **encoded keys**), shortest ints, definite lengths; `dec128` as fixed tag + 16‑byte payload; `time|duration` as **int nanoseconds**. 
* **Sets**: dedupe by typed equality, then sort by the element’s **canonical CBOR bytes**; encode as CBOR array in that order.
* **Maps**:

  * `map<text,V>`: author as JSON object; encode as **CBOR map** with canonical key ordering.
  * `map<K,V>` (non‑text keys): author as `[[key,value], …]`; encode as **CBOR map** sorted by **encoded key bytes**.
* **Numeric domains**: accept numbers or strings in sugar; reject values outside domain; encode as shortest‑form CBOR ints.
* **`dec128`**: sugar is a string matching `DecimalString`; encode as tagged 16‑byte decimal128. 
* **`bytes`**: sugar is base64; encode as CBOR bytes. 
* **`hash`**: `"sha256:<64-hex>"` ➝ 32 bytes. 
* **`uuid`**: RFC‑4122 ➝ 16 bytes.
* **`variant`**: expand sugar `{ "Tag": … }` to a canonical envelope (implementation‑chosen but stable; the tagged JSON shown above is one such envelope).
* **Schema‑bound hashing**: when hashing a value, **include the value’s `schema_hash`** alongside the canonical value bytes. 

These rules already align with the current Encoding/Hashing sections; we are making the sugar→CBOR path explicit and normative. 

---

## B. JSON Schema Changes (v1)

We aim for **zero friction** authoring while keeping machine precision available.

### B.1 `common.schema.json` — add `ExprOrValue`

Add a helper union so positions that usually hold constants can accept plain values **or** full expressions:

```json
// + In common.schema.json $defs
"ExprOrValue": {
  "oneOf": [
    { "$ref": "#/$defs/Expr" },
    { "$ref": "#/$defs/Value" }
  ]
}
```

Rationale: today `Value` is explicitly “unrestricted JSON structure validated elsewhere,” which suits authoring sugar. `Expr` remains for refs/ops/constructors; `ExprOrValue` keeps authoring readable without losing power. 

> **No change** to the existing `Expr` or `ExprConst` shapes. The canonical JSON (tagged) lens remains acceptable because it is valid `Value` (objects/arrays/strings). 

### B.2 `defplan.schema.json` — use `ExprOrValue` in literal‑heavy slots

Replace `Expr` with `ExprOrValue` in four places:

* `StepEmitEffect.params`
* `StepRaiseEvent.event`
* `StepAssign.expr`
* `StepEnd.result`

All other fields (e.g., guards like `edges[].when`, `await_receipt.for`) remain `Expr`. 

This change lets authors write natural JSON where a type is known from context (schema of effect params/event/result/locals), while preserving full expression power where needed.

### B.3 No changes required to `defpolicy` and `defschema`

* `defpolicy` uses a dedicated `Match` structure; no general value positions exist. **No change.** 
* `defschema` remains the same (type AST). **No change.** 

---

## C. Built‑ins: add HTTP & LLM schemas to `builtin-schemas.air.json`

The v1 spec already documents these effect kinds and their shapes; this formalizes them as built‑in schemas alongside `blob.*` and `timer.*`.  

Append the following **defschema** entries:

```json
{
  "$kind": "defschema",
  "name": "sys/HttpRequestParams@1",
  "type": {
    "record": {
      "method": { "text": {} },
      "url": { "text": {} },
      "headers": { "map": { "key": { "text": {} }, "value": { "text": {} } } },
      "body_ref": { "option": { "hash": {} } }
    }
  }
},
{
  "$kind": "defschema",
  "name": "sys/HttpRequestReceipt@1",
  "type": {
    "record": {
      "status": { "int": {} },
      "headers": { "map": { "key": { "text": {} }, "value": { "text": {} } } },
      "body_ref": { "option": { "hash": {} } },
      "timings": {
        "record": { "start_ns": { "nat": {} }, "end_ns": { "nat": {} } }
      },
      "adapter_id": { "text": {} }
    }
  }
},
{
  "$kind": "defschema",
  "name": "sys/LlmGenerateParams@1",
  "type": {
    "record": {
      "provider": { "text": {} },
      "model": { "text": {} },
      "temperature": { "dec128": {} },
      "max_tokens": { "nat": {} },
      "input_ref": { "hash": {} },
      "tools": { "option": { "list": { "text": {} } } }
    }
  }
},
{
  "$kind": "defschema",
  "name": "sys/LlmGenerateReceipt@1",
  "type": {
    "record": {
      "output_ref": { "hash": {} },
      "token_usage": {
        "record": { "prompt": { "nat": {} }, "completion": { "nat": {} } }
      },
      "cost_cents": { "nat": {} },
      "provider_id": { "text": {} }
    }
  }
}
```

They complement the existing `sys/{Timer*,Blob*}` entries already present in the file. 

> **Note (reducers):** delivery of micro‑effect receipts to reducers uses the standard receipt‑event shapes already described in the reducers chapter; those entries remain as‑is. Plans typically raise their own domain result events rather than consuming raw receipts. 

---

## D. Implementation Guidance (Loader/Validator/Tools)

### D.1 Loader (normative behavior)

* **Input acceptance:** at every typed value position, accept **either** authoring sugar **or** tagged canonical JSON. Resolve the expected type from the **schema reference in context** (plan IO, effect params, event schemas, local types, cap schemas, etc.).  
* **Canonicalization:** convert to a typed value; apply rules in §A.3; produce **canonical CBOR**; **bind schema hash** when hashing values. 
* **Expression sugar:** in any `ExprOrValue` position where the JSON value is not an object with `"op"`/`"ref"`/`"variant"` etc., treat it as a constant/structured literal and lift to the appropriate `Expr*` internally (optional, but recommended for consistent diagnostics).  

### D.2 Validator

The existing semantic checks remain unchanged (DAG acyclicity, effect allowlists, capability/policy checks, schema compatibility). The only addition is recognizing that `ExprOrValue` positions can arrive as plain `Value`. 

### D.3 CLI & inspector (DX)

* `air fmt --sugar|--canon` — pretty‑print any node or subtree in either lens (no behavior change; formatting only).
* `air diff A B --view=sugar|canon` — **compute diff over canonical CBOR**, render in requested lens for review.
* `air patch apply` — accept either lens inside `node`/`new_node` (AIR §14), normalize at load. 

### D.4 Storage contract (unchanged)

Worlds continue to ship `manifest.air.json` (text) and `manifest.air.cbor` (canonical). Tools **may** also write `*.air.canon.json` as an auxiliary artifact for debugging; it is not part of the kernel contract. 

---

## E. Spec Text to Add/Modify

> All section numbers refer to the current **AIR v1 Specification**.

1. **§3 Encoding — add “3.1 Authoring lenses and canonicalization” (normative):**

   * Define **two JSON lenses** (sugar, canonical JSON) and require the loader to accept either.
   * List the canonicalization rules in §A.3 (sets/maps ordering, numeric shortest‑form, `dec128` tagging, `time/duration` in ns).
   * Re‑affirm “**only CBOR is hashed/persisted**; value hashing binds the `schema_hash`.” 

2. **§7 Effect Catalog — make HTTP and LLM schemas explicit as built‑ins and point to `spec/defs/builtin-schemas.air.json`.** (The kinds are already listed; this adds their canonical `defschema` entries.) 

3. **§11 Plans — authoring quality of life:**

   * Call out that `params`, `event`, `assign.expr`, and `end.result` **accept `ExprOrValue`**: either a full `Expr` or a plain `Value` interpreted by schema.
   * Clarify that guards (`edges[].when`) remain full `Expr`. 

4. **Architecture (§AIR Loader/Validator):** explicitly state that the loader parses either JSON lens and always exposes **typed values** to the kernel after canonicalization to CBOR. (This is already implied; we are making it explicit.) 

5. **§14 Patch Format:** note that `node`/`new_node` **may** be provided in either JSON lens; canonicalization happens before hashing/validation. 

---

## F. Concrete JSON‑Schema Diffs (minimal)

> Shown conceptually; apply in the repo’s `spec/schemas/*.json`.

**1) `common.schema.json` — add `ExprOrValue`:**

```diff
   "$defs": {
+    "ExprOrValue": {
+      "oneOf": [
+        { "$ref": "#/$defs/Expr" },
+        { "$ref": "#/$defs/Value" }
+      ]
+    },
     "Expr": { … },
     "Value": { … }
```

(Existing `Expr`/`Value` remain unchanged.) 

**2) `defplan.schema.json` — swap selected fields to `ExprOrValue`:**

```diff
- "event": { "$ref": "common.schema.json#/$defs/Expr" },
+ "event": { "$ref": "common.schema.json#/$defs/ExprOrValue" },

- "params": { "$ref": "common.schema.json#/$defs/Expr" },
+ "params": { "$ref": "common.schema.json#/$defs/ExprOrValue" },

- "expr": { "$ref": "common.schema.json#/$defs/Expr" },
+ "expr": { "$ref": "common.schema.json#/$defs/ExprOrValue" },

- "result": { "$ref": "common.schema.json#/$defs/Expr" }
+ "result": { "$ref": "common.schema.json#/$defs/ExprOrValue" }
```

All other `Expr` uses stay as‑is. 

**3) `spec/defs/builtin-schemas.air.json` — append HTTP & LLM** as shown in §C (keep current Timer/Blob entries intact). 

---

## G. Testing Scenarios (what to test)

> You asked for **what** to test; you’ll fill in the actual JSON. Below is a matrix that exercises authoring, canonicity, typing, plans, and governance.

### G.1 Canonicalization & Hashing

1. **Sugar vs Canon JSON equivalence**

   * For each primitive/composite type, author a value in **sugar** and in **canonical JSON**; assert **identical canonical CBOR bytes** and identical **value hash** (with the same `schema_hash`). 
2. **Set dedupe/order**

   * Provide a sugar set with duplicates and various orders; verify CBOR order is by element **CBOR byte order** and duplicates removed.
3. **Map order**

   * `map<text,V>`: author keys in arbitrary order; assert CBOR map key order is canonical.
   * `map<uuid,nat>`: author as `[[k,v]…]`; assert CBOR map ordering by key bytes (and that sugar object form is **rejected** for non‑text keys).
4. **Numeric domains**

   * `int|nat` using boundary values; author some as strings; assert numeric domain checks + shortest CBOR ints.
   * `dec128` values including exponents; assert parsing and 16‑byte payload stability (round‑trip).
5. **Time/Duration**

   * Author as RFC‑3339/ISO; assert CBOR **ns** exactness and tagged JSON overlay printing as `{ "time_ns": … }` / `{ "duration_ns": … }`.

### G.2 Loader & Typing

6. **`ExprOrValue` positions**

   * In a plan, author `emit_effect.params`, `raise_event.event`, `assign.expr`, `end.result` as:
     (a) plain sugar, (b) canonical JSON, (c) full `ExprRecord`/`ExprVariant` trees. All three should typecheck to the same schema and produce the same CBOR. 
7. **Disallowed sugar shapes**

   * For `map<K,V>` with non‑text keys, author an object; expect a loader error.
   * For an incorrect `variant` tag, expect a loader error naming the allowed tags.
8. **Ambiguity resolution**

   * Where container type is not obvious, require an adjacent schema ref (e.g., `{ "$schema": "X@1", "value": … }` in tools) or reject with a precise diagnostic. (Tooling behavior test; not kernel.)

### G.3 Built‑ins / Effect Path

9. **HTTP params/receipt**

   * Author `sys/HttpRequestParams@1` in sugar and canonical JSON; hash, enqueue under a scoped `http.out` CapGrant; verify policy gates and adapter receipt schema. 
10. **LLM params/receipt**

* Author `sys/LlmGenerateParams@1`; check `max_tokens` budget pre‑check; receipt settles `token_usage` and `cost_cents`. 

11. **Reducer receipt events**

* For a micro‑effect (timer/blob) path, assert reducer receives the documented receipt‑event shapes; plans normally raise domain result events. 

### G.4 Plans & Execution

12. **Plan DAG invariants**

* Acyclic; missing refs; guard evaluation; `await_receipt.for` must point to prior emit; `kind ∈ allowed_effects`; `cap ∈ required_caps`. Expect specific validator diagnostics.  

13. **End result typing**

* With `output` declared, author `end.result` as sugar and canonical JSON; enforce type match.

### G.5 Patch/Diff/Replay

14. **`air diff` stability**

* Diff two nodes that only differ by sugar formatting (e.g., map key order). Diff **must be empty** when computed over canonical CBOR; verify renders as empty in both text lenses. 

15. **Patch with either lens**

* `add_def`/`replace_def` where `node/new_node` is authored in sugar vs canonical JSON; both apply to the same new manifest hash. 

16. **Replay determinism**

* Journal with effects/receipts; re‑load same manifest and replay; assert identical final snapshots. (Architecture determinism.) 

---

## H. Rationale & Fit

* **Authoring stays pleasant** (sugar), **agents stay precise** (tagged canon JSON), and the kernel stays **deterministic** (CBOR + schema‑bound hashing). This matches AIR’s existing encoding and the architecture’s loader/validator boundary.  
* The **`ExprOrValue`** tweak solves the largest ergonomics pain in plans without weakening the type system or expression semantics. 
* Materializing HTTP/LLM in **built‑ins** aligns the file set with the spec’s effect catalog and strengthens type‑directed authoring across the toolchain.  

---

### Appendix: Pointers to existing definitions

* **Common defs & `Value`/`Expr`** — current JSON types and tags. 
* **defplan** — where `ExprOrValue` now applies. 
* **defpolicy** — no changes required. 
* **defschema** — unchanged. 
* **Built‑in schemas (Timer/Blob)** — extended with HTTP/LLM. 
* **Reducers, receipt events** — context for receipt delivery and domain intents. 
* **Architecture (Loader/Validator; packaging)** — loader produces canonical CBOR; on‑disk layout. 
* **AIR Spec (Encoding/Hashing; Patch format; Plans)** — unchanged core rules; this update makes lenses/canonicalization explicit. 
* **Overview** — homoiconic goal of co‑authoring and self‑modification. 

---

**Adoption note:** You can merge the schema diffs and built‑ins immediately. Tooling (`fmt|diff|patch`) can roll out incrementally since the kernel contract (canonical CBOR) and the JSON acceptance surface (now explicit) are stable.
