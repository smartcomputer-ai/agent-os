# AIR v1 Specification

AIR (Agent Intermediate Representation) is a small, typed, canonical control‑plane IR that AgentOS loads, validates, diffs/patches, shadow‑simulates, and executes deterministically. AIR is not a general‑purpose programming language; heavy computation runs in deterministic WASM modules. AIR orchestrates modules, effects, capabilities, policies, and plans—the control plane of a world.

**JSON Schema references** (may evolve; kept in this repo):
- spec/schemas/common.schema.json
- spec/schemas/defschema.schema.json
- spec/schemas/defmodule.schema.json
- spec/schemas/defplan.schema.json
- spec/schemas/defcap.schema.json
- spec/schemas/defpolicy.schema.json
- spec/schemas/defsecret.schema.json
- spec/schemas/manifest.schema.json

These schemas validate structure. Semantic checks (DAG acyclicity, type compatibility, name/hash resolution, capability bindings) are enforced by the kernel validator.

## Goals and Scope

AIR v1 provides one canonical, typed control plane the kernel can load, validate, diff/patch, shadow‑run, and execute deterministically. It is intentionally **not Turing complete**: plans are finite DAGs with total predicates; no unbounded loops or recursion exist.

AIR is **control‑plane only**. It defines schemas, modules, plans, capabilities, policies, and the manifest. Application state lives in reducer state (deterministic WASM), encoded as canonical CBOR.

The policy engine is minimal: ordered allow/deny rules. Hooks are reserved for richer policy later. The effects set in v1 is also minimal: `http.request`, `blob.{put,get}`, `timer.set`, `llm.generate`. Migrations are deferred; `defmigration` is reserved.

## 1) Vocabulary and Identity

**Kind**: One of `defschema`, `defmodule`, `defplan`, `defcap`, `defpolicy`, `defsecret`, `defeffect`, or `manifest`. (`defmigration` is reserved for future use.)

**Name**: A versioned identifier with the format `namespace/name@version`, where version is a positive integer. Example: `com.acme/rss_fetch@1`.

**Hashing**: SHA‑256 over the canonical CBOR encoding of a node. Nodes embed their `$kind`; schema/value nodes bind type by including the schema hash (see Encoding section).

**IDs**: References use either a Name or a content hash (`sha256:HEX`). The manifest maps Names to hashes; this mapping is immutable within a given manifest version.

## 2) Types (Schemas, IO, Effect Shapes)

AIR defines a small, typed language for data structures used throughout the system.

### Primitive Types

- **bool**, **int** (i64), **nat** (u64), **dec128** (IEEE 754 decimal128 as 16‑byte bytes)
- **bytes**, **text** (UTF‑8)
- **time** (int ns since epoch), **duration** (int ns)
- **hash** (32‑byte SHA‑256), **uuid** (16‑byte RFC 4122)

### Composite Types

- **record** `{ field: Type, … }` — required fields only in v1
- **variant** `[ Alt: Type, … ]` — closed sums (tagged unions)
- **list** `Type`, **map** `{ key: Type, value: Type }` where key ∈ {int, nat, text, uuid, hash}
- **set** `Type` for comparable domains
- **option** `Type`, **unit** `{}`

### Identity and Compatibility

A schema's identity is its **schema_hash** = `sha256(cbor(fully expanded type AST))`. A value claims a schema by including the schema_hash during hashing. Version 1 requires exact schema matches (no subtyping except option sugar).

See: spec/schemas/common.schema.json and spec/schemas/defschema.schema.json

## 3) Encoding

AIR nodes exist in two interchangeable JSON lenses plus one canonical binary form. All persisted identity and hashing stays bound to canonical CBOR; the dual JSON lenses exist purely for ergonomics and tooling.

### 3.1 JSON Lenses (Authoring vs. Canonical)

**Why**: Humans want concise, schema-directed JSON; agents and tools often need an explicit, lossless overlay. Accepting both lenses at load time keeps authoring pleasant without sacrificing determinism.

1. **Authoring sugar (default)** — plain JSON interpreted using the surrounding schema reference. Use natural literals (`true`, `42`, `"text"`, `{field: …}`, arrays) exactly as before.
2. **Canonical JSON (tagged)** — every literal carries an explicit type tag mirroring `ExprConst` (`{ "nat": 42 }`, `{ "list": [ { "text": "a" } ] }`, `{ "variant": { "tag": "Ok", "value": { "text": "done" } } }`, `{ "null": {} }`, etc.). This lens is ideal for diffs, automated patches, and inspector output because it round-trips without schema context.

The loader **MUST** accept either lens at every typed value position, resolve the schema from context (plan IO, effect params, reducer schemas, capability params, etc.), and convert to a typed value before hashing.

### 3.2 Canonicalization Rules (Sugar → Typed → CBOR)

Regardless of the JSON lens, the loader applies the same canonicalization before emitting CBOR:

- **Deterministic CBOR**: Canonical [CBOR](https://cbor.io/) (RFC 8949) with deterministic map key ordering (bytewise order of encoded keys), shortest integer encodings, and definite lengths.
- **Sets**: Deduplicate by typed equality, then sort elements by their canonical CBOR bytes. Encode as CBOR arrays in that order so `["b","a","a"]` and `["a","b"]` hash identically once typed.
- **Maps**:
  - `map<text,V>` may be authored as JSON objects; encode as CBOR maps sorted by canonical key bytes.
  - Maps with non-text keys are authored as `[[key,value], …]` pairs; encode as CBOR maps sorted by the key’s canonical bytes.
- **Numeric domains**: Accept numbers or string literals (for large ints/decimals); reject out-of-range values and always encode using the shortest CBOR int.
- **`dec128`**: Author as a decimal string; encode as the dedicated tag (2000) with a 16-byte payload.
- **`time` / `duration`**: Allow RFC 3339 strings or integer nanoseconds; encode as signed/unsigned nanosecond integers.
- **`bytes`**: Author as base64 strings; encode as CBOR byte strings.
- **`hash`**: Author as `"sha256:<64hex>"`; encode as raw 32-byte values.
- **`uuid`**: Author as RFC 4122 strings; encode as 16-byte values.
- **`variant`**: Sugar `{ "Tag": <value?> }` expands to a canonical envelope (e.g., `{ "variant": { "tag": "Tag", "value": … } }`) before CBOR.
- **`option<T>`**: Represent `none` as `null` in sugar or `{ "option": null }` in canonical JSON; `some` wraps the nested value.

**Nulls in Expr vs Value**: When an `ExprOrValue` slot is authored as a literal Value, raw JSON `null` denotes `none` for `option<T>`. When authored as an expression, use `{ "null": {} }` to produce a `null`/`none` value. Raw JSON `null` is **not** valid in expression ASTs and is only accepted on the literal path.

These rules make previously implicit loader behavior normative and testable.

### 3.3 Binary Form and Hashing

Canonical CBOR remains the storage and hashing format. The node hash is `sha256(cbor(node))`.

When hashing a typed value (plan IO, cap params, etc.), always bind the **schema hash** alongside the canonical bytes. This prevents two different schemas that serialize to the same JSON shape from colliding and keeps “schema + value” as the identity pair.

### 3.4 Event Payload Normalization (Journal Invariant)

DomainEvents and ReceiptEvents are treated exactly like effect params:

- **Ingress rule**: Every event payload is decoded against its declared `defschema` (from reducer ABI for reducer delivery, trigger entry for plan starts, or built-in receipt schema when synthesizing receipts), validated, canonicalized, and re-encoded as canonical CBOR. If validation fails, the event is rejected. For reducer micro-effect receipts, the kernel **wraps** the receipt payload into the reducer’s ABI event schema before routing and journal append.
- **Journal rule**: The journal stores and replays only these canonical bytes; replay never rewrites payloads.
- **Routing/correlation**: Key extraction for routed/correlated events uses the schema-aware decoded value (not ExprValue tagging) and validates against the reducer’s `key_schema`.
- **Sources**: Reducer-emitted DomainEvents, plan-raised events, adapter receipts synthesized as events, and externally injected/CLI events all flow through the same normalizer.

## 4) Manifest

The manifest is the root catalog of a world's control plane. It lists all schemas, modules, plans, capabilities, and policies by name and hash, defines event routing and triggers, and specifies defaults.

### Shape

```
{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [{name, hash}],
  "modules": [{name, hash}],
  "plans": [{name, hash}],
  "effects": [{name, hash}],
  "caps": [{name, hash}],
  "policies": [{name, hash}],
  "routing": {
    "events": [{event: SchemaRef, reducer: Name, key_field?: text}],
    "inboxes": [{source: text, reducer: Name}]
  },
  "triggers": [{event: SchemaRef, plan: Name, correlate_by?: text}],
  "defaults": {policy?: Name, cap_grants?: [CapGrant…]},
  "module_bindings"?: {Name → {slots: {slot_name → CapGrantName}}}
}
```

### Rules

Names must be unique per kind; all hashes must exist in the store. `air_version` is **required**; v1 manifests must set it to `"1"`. Supplying an unknown version or omitting the field is a validation error. `routing.events` maps DomainEvents on the bus to reducers; **the routed schema must equal the reducer’s `defmodule.abi.reducer.event`** (use a variant schema to accept multiple event shapes, including receipts). `routing.inboxes` maps external adapter inboxes (e.g., `http.inbox:contact_form`) to reducers for messages that skip the DomainEvent bus. For keyed reducers, include `key_field` to tell the kernel where to extract the key from the event payload (validated against the reducer's `key_schema`); when the event schema is a variant, `key_field` typically targets the wrapped value (e.g., `$value.note_id`). The `triggers` array maps DomainIntent events to plans: when a reducer emits an event matching a trigger's schema, the kernel starts the referenced plan with that event as input; a trigger's optional `correlate_by` copies that field into the run context for later `await_event` filters (for variant inputs, use `$value.<field>`). The `effects` list is the authoritative catalog of effect kinds for this world. **List every schema/effect your world uses**; built-ins are no longer auto-included. Tooling may still fill the canonical hash for built-ins when the name is present without a hash.

See: spec/schemas/manifest.schema.json

## 5) defschema

Defines a named type used for values, events, reducer state, and plan IO.

### Shape

```json
{
  "$kind": "defschema",
  "name": "namespace/name@version",
  "type": <TypeAST>
}
```

### Rules

No recursive types in v1. Field and variant names must be unique within a schema. The schema produces a `schema_hash` used to identify values of this type.

See: spec/schemas/defschema.schema.json

## 6) defmodule

Registers a WASM module with its interface contract.

### Module Kind

**reducer**: deterministic state machine

### Shape

```json
{
  "$kind": "defmodule",
  "name": "namespace/name@version",
  "module_kind": "reducer",
  "wasm_hash": <Hash>,
  "abi": {
    "reducer": {
      "state": <SchemaRef>,
      "event": <SchemaRef>,
      "annotations"?: <SchemaRef>,
      "effects_emitted"?: [<EffectKind>…],
      "cap_slots"?: {slot_name: <CapType>}
    }
  },
"key_schema"?: <SchemaRef>
}
```

`EffectKind` and `CapType` are namespaced strings. The schema no longer hardcodes an enum; v1 ships a built-in catalog listed in §7, and adapters can introduce additional kinds as runtime support lands.

The `key_schema` field (v1.1 addendum) documents the key type when this reducer is routed as keyed. The ABI remains a single `step` export; the kernel provides an envelope with optional key.
When routed as keyed, the kernel sets `ctx.cell_mode=true` and passes only the targeted cell's state; returning `state=null` deletes the cell instance.

### ABI

Reducer export: `step(ptr, len) -> (ptr, len)`

- **Input**: CBOR envelope including optional key (see Cells spec)
- **Output**: CBOR `{state, domain_events?, effects?, ann?}`

### Determinism

No WASI ambient syscalls, no threads, no clock. All I/O happens via the effect layer. Prefer `dec128` in values; normalize NaNs if floats are used internally.

**Note**: Pure modules (stateless, side-effect-free functions) are now supported as `module_kind: "pure"`.
Use reducers for stateful logic; use pure modules for deterministic transforms and authorizers.

See: spec/schemas/defmodule.schema.json

## 7) Effect Catalog (Built-in v1)

`EffectKind` is an open namespaced string; the core schema no longer freezes the list. The catalog is now **data-driven via `defeffect` nodes** listed in `manifest.effects` plus the built-in bundle (`spec/defs/builtin-effects.air.json`). Canonical parameter/receipt schemas live under `spec/defs/builtin-schemas.air.json` so plans, reducers, and adapters all hash the same shapes. Tooling can stay strict for these built-ins while leaving space for adapter-defined kinds in future versions by deriving enums from the `defeffect` set.

`origin_scope` on each `defeffect` gates who may emit it: reducers only for `reducer/both`, plans only for `plan/both`. “Micro-effects” are exactly those whose `origin_scope` allows reducers (currently `blob.put`, `blob.get`, `timer.set` in v1); others are plan-only.

Built-in kinds in v1:

**http.request**
- params: `{ method:text, url:text, headers: map{text→text}, body_ref?:hash }`
- receipt: `{ status:int, headers: map{text→text}, body_ref?:hash, timings:{start_ns:nat,end_ns:nat}, adapter_id:text }`

**blob.put**
- params: `{ namespace:text, blob_ref:hash }`
- receipt: `{ blob_ref:hash, size:nat }`

**blob.get**
- params: `{ namespace:text, key:text }`
- receipt: `{ blob_ref:hash, size:nat }`

**timer.set**
- params: `{ deliver_at_ns:nat, key?:text }`
- receipt: `{ delivered_at_ns:nat, key?:text }`

**llm.generate**
- params: `{ provider:text, model:text, temperature:dec128, max_tokens:nat, input_ref:hash, tools?:list<text>, api_key?:TextOrSecretRef }`
- receipt: `{ output_ref:hash, token_usage:{prompt:nat,completion:nat}, cost_cents:nat, provider_id:text }`

**vault.put**
- params: `{ alias:text, binding_id:text, value_ref:hash, expected_digest:hash }`
- receipt: `{ alias:text, version:nat, binding_id:text, digest:hash }`

**vault.rotate**
- params: `{ alias:text, version:nat, binding_id:text, expected_digest:hash }`
- receipt: `{ alias:text, version:nat, binding_id:text, digest:hash }`

**introspect.manifest / introspect.reducer_state / introspect.journal_head / introspect.list_cells** (plan-only, cap_type `query`)
- Read-only effects served by an internal kernel adapter; receipts include consistency metadata used by governance and self-upgrade flows.
- `introspect.manifest`: params `{ consistency: text }` (`head` | `exact:<h>` | `at_least:<h>`); receipt `{ manifest, journal_height, snapshot_hash?, manifest_hash }`
- `introspect.reducer_state`: params `{ reducer:text, key_b64?:text, consistency:text }`; receipt `{ state_b64?:text, meta:{ journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.journal_head`: params `{}`; receipt `{ journal_height, snapshot_hash?, manifest_hash }`
- `introspect.list_cells`: params `{ reducer:text }`; receipt `{ cells:[{ key_b64, state_hash, size, last_active_ns }], meta:{ journal_height, snapshot_hash?, manifest_hash } }`

Built-in capability types paired with these effects (v1): `http.out`, `blob`, `timer`, `llm.basic`, `secret`, and `query`. The schema stays open to future types even though the kernel ships this curated set today.

### Built-in reducer receipt events

Reducers that emit micro-effects rely on the kernel to translate adapter receipts into typed DomainEvents. AIR v1 reserves these `defschema` names so reducers can include them in their **ABI event variants** and count on stable payloads:

| Schema | Purpose | Fields |
| --- | --- | --- |
| **`sys/TimerFired@1`** | Delivery of a `timer.set` receipt back to the originating reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text` (always `"timer.set"` in v1), `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/TimerSetParams@1`, `receipt:sys/TimerSetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobPutResult@1`** | Delivery of a `blob.put` receipt to the reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobPutParams@1`, `receipt:sys/BlobPutReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobGetResult@1`** | Delivery of a `blob.get` receipt to the reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobGetParams@1`, `receipt:sys/BlobGetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |

Reducers should **reference these schemas from their ABI event variant** (e.g., `TimerEvent.Fired -> sys/TimerFired@1`) and route only the reducer’s ABI event schema. The kernel wraps receipt payloads into that variant before routing. Plans typically raise domain-specific result events instead of consuming these `sys/*` receipts. The shared `cost_cents` and `signature` fields exist today so future policy/cost analysis can trust the same structures without changing reducer code.

Canonical JSON definitions for these schemas (plus their parameter/receipt companions) live in `spec/defs/builtin-schemas.air.json` so manifests can hash and reference them directly.

## 8) Effect Intents and Receipts

### EffectIntent

An intent is a request to perform an external effect:

```
{
  kind: EffectKind,
  params: ValueCBORRef,
  cap: CapGrantName,
  idempotency_key: hash,
  intent_hash: hash
}
```

The `intent_hash` = `sha256(cbor(kind, params, cap, idempotency_key))` is computed by the kernel; adapters verify it.

**Canonical params**: Before hashing or enqueue, the kernel **decodes → schema‑checks → canonicalizes → re‑encodes** `params` using the effect kind's parameter schema (same AIR canonical rules as the loader: `$tag/$value` variants, canonical map/set/option shapes, numeric normalization). The canonical CBOR bytes become `params_cbor` and are the **only** form stored, hashed, and dispatched; non‑conforming params are rejected. This path runs for *every* origin (plans, reducers, injected tooling) so authoring sugar or reducer ABI quirks cannot change intent identity.

**Idempotency key**: plans and reducers may supply an explicit `idempotency_key`; when omitted, the kernel uses the all‑zero key. Re‑emitting an identical effect with the same key yields the same `intent_hash`.

### Receipt

A receipt is the signed result of executing an effect:

```
{
  intent_hash: hash,
  adapter_id: text,
  status: "ok" | "error",
  receipt: ValueCBORRef,
  cost?: nat,
  sig: bytes
}
```

The kernel validates the signature (ed25519/HMAC), binds the receipt to its intent, and appends it to the journal.

## 9) defcap (Capability Types) and Grants

Capabilities define scoped permissions for effects. A `defcap` declares a capability type; a `CapGrant` is a runtime instance of that capability with concrete constraints and optional expiry.

### defcap Definition

```json
{
  "$kind": "defcap",
  "name": "namespace/name@version",
  "cap_type": "http.out" | "blob" | "timer" | "llm.basic" | "secret" | "query",
  "schema": <SchemaRef>,
  "enforcer": { "module": "sys/CapAllowAll@1" }
}
```

The schema defines parameter constraints enforced at enqueue time. The enforcer is a deterministic module invoked by the kernel during authorization; `sys/CapAllowAll@1` is a built-in allow-all enforcer.

### Standard v1 Capability Types (built-in)

**sys/http.out@1**
- Schema: `{ hosts: set<text>, verbs: set<text>, path_prefixes?: set<text> }`
- At enqueue: `authority(url) ∈ hosts`; `method ∈ verbs`; path `starts_with` any `path_prefixes` if present.
- Terminology: `verbs` is the allowlist of HTTP methods for the capability; each request still supplies its concrete `method`.

**sys/llm.basic@1**
- Schema: `{ providers?: set<text>, models?: set<text>, max_tokens_max?: nat, temperature_max?: dec128, tools_allow?: set<text> }`
- At enqueue: `provider`/`model` ∈ allowlists if present; `max_tokens ≤ max_tokens_max`; `temperature ≤ temperature_max`; `tools ⊆ tools_allow`.

**sys/blob@1**
- Schema: `{ namespaces?: set<text> }` (minimal in v1)

**sys/timer@1**
- Schema: `{}` (no constraints in v1)

**sys/query@1**
- Schema: `{ scope: text }` (`scope` is optional/semantically freeform; empty string = all)
- Guards read-only `introspect.*` effects; policy may further restrict by reducer/effect kind.

### CapGrant (Runtime Instance)

A grant is kernel state referenced by name:

```
{
  name: text,
  cap: Name(defcap),
  params: Value,
  expiry_ns?: nat
}
```

The `params` must conform to the defcap's schema and encode concrete allowlists/ceilings.

### Enforcement

**At enqueue, the kernel checks:**
1. Grant exists and has not expired.
2. Capability type matches effect kind.
3. Effect params satisfy grant constraints (hosts, models, max_tokens_max, etc.).
4. Policy decision (see defpolicy).

Budget enforcement is deferred to a future milestone; see `roadmap/vX-future/p4-budgets.md`.

See: spec/schemas/defcap.schema.json

## 10) defeffect (Effect Catalog Entries)

`defeffect` declares an effect kind, its parameter/receipt schemas, the capability type that guards it, and which emitters may use it.

### Shape

```json
{
  "$kind": "defeffect",
  "name": "sys/http.request@1",
  "kind": "http.request",
  "params_schema": "sys/HttpRequestParams@1",
  "receipt_schema": "sys/HttpRequestReceipt@1",
  "cap_type": "http.out",
  "origin_scope": "plan",
  "description": "Optional human text"
}
```

### Fields

- `name`: Versioned Name of the effect definition (namespace/name@version)
- `kind`: EffectKind string referenced by plans/reducers (e.g., `http.request`)
- `params_schema`: SchemaRef for effect parameters
- `receipt_schema`: SchemaRef for effect receipts
- `cap_type`: Capability type that must guard this effect
- `origin_scope`: `"reducer" | "plan" | "both"`; reducers may emit only reducer/both, plans may emit plan/both
- `description?`: Optional prose

### Notes

- Built-in v1 effects live in `spec/defs/builtin-effects.air.json`; include the ones your world uses (hashes may be filled by tooling for built-ins).
- Unknown effect kinds (not declared in the manifest or built-ins) are rejected during plan normalization/dispatch.
- Reducer receipt translation remains limited to effects whose `origin_scope` allows reducers.
- (Future) Adapter binding stays out of `defeffect`; a manifest-level `effect_bindings` table can later map kinds to adapters without changing the defkind.

See: spec/schemas/defeffect.schema.json

## 11) defpolicy (Rule Pack)

Policies define ordered rules that allow or deny effects based on their characteristics and origin.

### Shape

```json
{
  "$kind": "defpolicy",
  "name": "namespace/name@version",
  "rules": [<Rule>…]
}
```

### Rule (v1)

```
{
  when: <Match>,
  decision: "allow" | "deny"
}
```

### Match Fields (v1)

- `effect_kind?: EffectKind` – namespaced effect kind (http.request, llm.generate, etc.)
- `cap_name?: text` – which CapGrant name
- `origin_kind?: "plan" | "reducer"` – whether the effect originates from a plan or a reducer
- `origin_name?: Name` – the specific plan or reducer Name

### Decision (v1)

**"allow"** or **"deny"** only. `"require_approval"` is reserved for v1.1+ (not implemented in v1).

### Semantics

**First match wins** at enqueue time; if no rule matches, the default is **deny**. The kernel populates `origin_kind` and `origin_name` on each EffectIntent from context (plan instance or reducer invocation). Policy is evaluated **after** capability constraint checks; both must pass for dispatch.

Policy matching works over open strings: custom effect kinds are allowed as long as the runtime has a catalog entry mapping that kind to a capability type and schemas. Unknown effect kinds (not in the built-in catalog or a registered adapter catalog) are rejected during validation/dispatch before policy evaluation.

Decisions are journaled: `PolicyDecisionRecorded { intent_hash, policy_name, rule_index, decision }`.

### Recommended Default Policy

- **Deny** `llm.generate`, `payment.*`, `email.*`, and `http.request` to non-allowlisted hosts from `origin_kind="reducer"`.
- **Allow** heavy effects only from specific plans under tightly scoped CapGrants.

### v1 Removals and Deferrals

- **Removed**: limits (rpm, daily_budget) from rules; rate limiting deferred to v1.1+.
- **Removed**: path_prefix from Match (use CapGrant.path_prefixes).
- **Removed**: principal from Match (identity/authn deferred to v1.1+).

See: spec/schemas/defpolicy.schema.json

## 12) defplan (Orchestration DAG)

Plans are finite DAGs of steps that orchestrate effects, wait for receipts, and coordinate with reducers via events. They have a typed environment, optional guard predicates on edges, and a deterministic scheduler.

### Scope and Purpose

Plans are the **orchestration layer**, NOT a compute runtime. They coordinate external effects under capabilities and policy, wait for receipts and human approvals, and raise events to reducers to advance domain state.

**Plans do NOT**:
- Perform heavy computation (use reducers)
- Mutate reducer state directly (only via raise_event)
- Make business logic decisions (that's the reducer's domain)

**Use plans when**:
- Coordinating multiple effects
- Requiring human gates/approvals
- Spanning long durations (minutes/hours)
- Needing centralized governance/audit

**Keep logic in reducers when**:
- Performing domain state transitions
- Enforcing business invariants
- Emitting simple micro-effects (timer, blob)

### v1 Scope and Future Extensions

Version 1.0 keeps plans minimal: `emit_effect`, `await_receipt`, `raise_event`, `await_event`, `assign`, `end`. `await_event` stays to let a single plan instance span multiple domain events without handing off through reducers; structured concurrency (sub-plans, fan-out/fan-in) is deferred to v1.1+ to validate real-world needs first.

See: spec/12-plans-v1.1.md for planned extensions (`spawn_plan`, `await_plan`, `spawn_for_each`, `await_plans_all`).

### Shape

```json
{
  "$kind": "defplan",
  "name": "namespace/name@version",
  "input": <SchemaRef>,
  "output"?: <SchemaRef>,
  "locals"?: {name: <SchemaRef>…},
  "steps": [<Step>…],
  "edges": [{from: StepId, to: StepId, when?: <Expr>}…],
  "required_caps": [<CapGrantName>…],  // derived from emit_effect.cap; optional in authoring
  "allowed_effects": [<EffectKind>…],  // derived from emit_effect.kind; optional in authoring
  "invariants"?: [<Expr>…]
}
```

### Steps (discriminated by `op`)

**raise_event**: Publish a bus event
- `{ id, op:"raise_event", event:SchemaRef, value:ExprOrValue }`
- The kernel validates/canonicalizes `value` against `event` before emitting. **Never embed `$schema` fields inside event or effect payloads; self-describing payloads are rejected.**

**emit_effect**: Request an external effect
- `{ id, op:"emit_effect", kind:EffectKind, params:ExprOrValue, cap:CapGrantName, bind:{effect_id_as:VarName} }`

**await_receipt**: Wait for an effect receipt
- `{ id, op:"await_receipt", for:Expr /*effect_id*/, bind:{as:VarName} }`

**await_event** (optional): Wait for a matching DomainEvent
- `{ id, op:"await_event", event:SchemaRef, where?:Expr, bind:{as:VarName} }`
- Semantics (v1):
  - **Future-only**: Plan registers the wait when the step first runs; only events appended afterwards are observed.
  - **Per-waiter first match**: The first event (by journal order) matching `event` and passing `where` resumes that plan instance.
  - **Broadcast**: Multiple plan instances waiting on the same schema all see the event; there is no consumption.
- **Predicate scope**: `where` evaluates with `@event` bound to the incoming event and may reference locals/steps/plan input; `@var:correlation_id` is available when the plan was started from a trigger with `correlate_by`.
- **One outstanding wait** per plan instance; the step blocks until satisfied, then normal DAG scheduling continues.
- **Correlation guard (required when correlated)**: If the plan was started via a trigger with `correlate_by`, every `await_event` must provide a `where` predicate that references the correlation key (e.g., `@event.<key> == @var:correlation_id`) to prevent cross-talk across concurrent runs; this is enforced at validation time.

**assign**: Bind a value to a variable
- `{ id, op:"assign", expr:ExprOrValue, bind:{as:VarName} }`

**Invariants (v1)**:
- Evaluated after each step completes and once more when the plan finishes.
- May reference plan input, locals, completed step outputs, and `@var:correlation_id`; must not reference `@event`.
- On first failure, the kernel ends the plan and records `PlanEnded { status:"error", error_code:"invariant_violation" }`; no further steps run.

**end**: Complete the plan
- `{ id, op:"end", result?:ExprOrValue }`
- Must match output schema if provided. The runtime canonicalizes/validates this result against `plan.output` before persisting it, and the canonical value is appended to the journal as a `plan_result` record so operators/shadow runs can see exactly what a plan produced.

### Literals vs. Expressions (`ExprOrValue`)

Authoring ergonomic insight: most plan fields already know the target schema (effect params, event payloads, locals, outputs). Requiring verbose `ExprRecord`/`ExprList` wrappers for simple literals made everyday plans noisy. In v1 we therefore allow `ExprOrValue` in four places (`emit_effect.params`, `raise_event.value`, `assign.expr`, `end.result`). Authors may provide:

1. A plain JSON value (in sugar or canonical lens), which the loader interprets via the declared schema.
2. A fully tagged `Expr` tree when dynamic computation or references are needed.

Disambiguation rule: Loaders first attempt to parse an `ExprOrValue` slot as an `Expr` (looking for `op`, `ref`, `record`, etc.). If that fails, the JSON is treated as a plain Value and interpreted using the surrounding schema.

Loaders may optionally lift literals into constant expressions internally so diagnostics stay consistent. Guards (`edges[].when`), `await_receipt.for`, and other predicate positions remain full `Expr` to keep intent clear: if branching logic or lookups are happening, you must be explicit.

### Expr and Predicates

**Expr** is side‑effect‑free over a typed Value: constants; refs (`@plan.input`, `@var:name`, `@step:ID.field…`); operators `len|get|has|eq|ne|lt|le|gt|ge|and|or|not|concat|add|sub|mul|div|mod|starts_with|ends_with|contains`.

**Predicates** are boolean Expr. Missing refs are errors (deterministic fail).

**Await/invariant references**: Authoring tools and the kernel validator now statically ensure that `await_receipt.for` references an emitted handle (`@var:<handle>`), that `await_event.where` predicates only reference declared locals/step outputs/plan input, and that plan `invariants[]` never reference `@event` or undeclared symbols. These checks surface manifest issues during validation rather than at runtime.

### Guards (Edge Predicates)

Edges can have optional `when` predicates called **guards**. A guard is a boolean expression that must evaluate to `true` for an edge to be traversable. This enables conditional branching in plan DAGs: a step becomes ready only when all predecessor edges are completed **and** all their guards evaluate to `true`. Duplicate `(from,to)` edges are invalid.

Example:
```json
{
  "from": "charge",
  "to": "reserve",
  "when": {
    "op": "eq",
    "args": [{"ref": "@var:charge_rcpt.status"}, {"text": "ok"}]
  }
}
```

This edge is only traversable if `charge_rcpt.status == "ok"`. Guards enable branching logic (success vs. failure paths, retries, compensations) without putting business logic in the plan—the plan remains declarative orchestration.

### Scheduling

A step is **ready** when predecessors are completed and its guard (if any) is true. The scheduler executes one ready step per tick; deterministic order by step id then insertion order.

`emit_effect` parks if nothing else is ready; `await_receipt` becomes ready when the matching receipt is appended.

The plan completes at `end` or when the graph has no outgoing edges (error if output declared but no end).

See: spec/schemas/defplan.schema.json (steps/Expr defined there)

## 13) StartPlan (Runtime API and Triggers)

Plans can start in two ways:

**Triggered start**: When a DomainIntent event with schema matching a manifest `triggers[].event` is appended, the kernel starts `triggers[].plan` with the event value as input. If `correlate_by` is provided, the kernel records that key for later correlation in await_event/raise_event flows.

**Manual start**: `{ plan:Name, input:ValueCBORRef, bind_locals?:{VarName:ValueCBORRef…} }`

The kernel pins the manifest hash for the instance, checks input/locals against schemas, and executes under the current policy and capability grants. Effects always check live grants at enqueue time.

## 14) Validation Rules (Semantic)

The kernel validator enforces these semantic checks:

**Manifest**: Names unique per kind; all references by name resolve to hashes present in the store.

**defmodule**: `wasm_hash` present; referenced schemas exist; `effects_emitted`/`cap_slots` (if present) are well‑formed.

**defplan**: DAG acyclic; step ids unique; Expr refs resolve; `required_caps`/`allowed_effects` are derived sets of `emit_effect.{cap,kind}` (if provided they must exactly match the derived values); `await_receipt.for` references an emitted handle; `await_event.where` only references declared locals/steps/input; `raise_event.value` must evaluate to a value conforming to the `raise_event.event` schema; invariants may only reference declared locals/steps/input (no `@event`); `end.result` is present iff `output` is declared and must match that schema (canonicalized + recorded as `plan_result`).

**defpolicy**: Rule shapes valid; referenced effect kinds known.

**defcap**: `cap_type` in built‑ins; parameter schema compatible.

## 15) Patch Format (AIR Changes)

Patches describe changes to the control plane (design-time modifications).

**Schema:** `spec/schemas/patch.schema.json` (also embedded in built-ins). Control/daemon paths validate PatchDocuments against this schema before compiling/applying. Authoring sugar is allowed (zero hashes, JSON lens), but payload shape must conform.

### Patch Document

```
{
  version: "1",
  base_manifest_hash: hash,
  patches: [<Patch>…]
  // v1: patches only cover defs, manifest refs, and defaults; routing/triggers/module_bindings/secrets cannot be patched yet.
}
```

`node` and `new_node` accept either JSON lens (authoring sugar or tagged canonical). The submission path:

1. **Schema validation** against `patch.schema.json` (structural only).
2. **Compile**: resolve `base_manifest_hash`, apply ops, canonicalize, compute hashes for new/updated defs, rewrite manifest refs.
3. **Store + hash**: store nodes/manifest in CAS, hash canonical CBOR to get `patch_hash`.
4. **Governance** uses `patch_hash` (and the compiled patch) for Proposed → Shadow → Approved → Applied.

### Operations

- **add_def**: `{ kind:Kind, node:NodeJSON }` — add a new definition
- **replace_def**: `{ kind:Kind, name:Name, new_node:NodeJSON, pre_hash:hash }` — optimistic swap
- **remove_def**: `{ kind:Kind, name:Name, pre_hash:hash }` — remove a definition
- **set_manifest_refs**: `{ add:[{kind,name,hash}], remove:[{kind,name}] }` — update manifest references
- **set_defaults**: `{ policy?:Name, cap_grants?:[CapGrant…] }` — update default policy and grants
- **set_routing_events**: `{ pre_hash:hash, events:[{event, reducer, key_field?}...] }` — replace routing.events block (empty list clears)
- **set_routing_inboxes**: `{ pre_hash:hash, inboxes:[{source, reducer}...] }` — replace routing.inboxes block
- **set_triggers**: `{ pre_hash:hash, triggers:[{event, plan, correlate_by?}...] }` — replace triggers block
- **set_module_bindings**: `{ pre_hash:hash, bindings:{ module → { slots:{slot→cap_grant} } } }` — replace module_bindings block
- **set_secrets**: `{ pre_hash:hash, secrets:[ SecretEntry… ] }` — replace manifest secrets block (refs/decls); no secret values carried in patches.
- **defsecret**: `add_def` / `replace_def` / `remove_def` now accept `defsecret`; `set_manifest_refs` can add/remove secret refs. Secret values still live outside patches; `set_secrets` only adjusts manifest entries.

### Application

Patches are applied transactionally to yield a new manifest; full re‑validation is required. The governance system turns patches into journal entries: Proposed → (Shadow) → Approved → Applied.

Authoring ergonomics:
- Zero hashes / missing manifest refs are allowed on input; the compiler fills them once nodes are hashed.
- CLI has `--require-hashes` to enforce explicit hashes for stricter flows.
- PatchDocuments can be submitted over control channel; kernel/daemon compiles them server-side so clients don't need to compute hashes.

## 16) Journal Entries (AIR‑Related)

The journal records both design-time (governance) and runtime (execution) events.

### Governance and Control Plane (Design Time)

- **Proposed** `{ proposal_id:u64, patch_hash, author, manifest_base, description? }`
- **ShadowReport** `{ proposal_id:u64, patch_hash, manifest_hash, effects_predicted:[EffectKind…], pending_receipts?:[PendingPlanReceipt], plan_results?:[PlanResultPreview], ledger_deltas?:[LedgerDelta] }`
- **Approved** `{ proposal_id:u64, patch_hash, approver, decision:"approve"|"reject" }`
- **Applied** `{ proposal_id:u64, patch_hash, manifest_hash_new }`

Notes:
- `proposal_id` is the world-local correlation key; `patch_hash` is the content key and may repeat if the same patch is resubmitted.
- `ShadowReport.manifest_hash` is the candidate manifest root produced by applying the patch (not the patch hash).
- `Applied.manifest_hash_new` is the new manifest root after apply (not the patch hash).
- Apply is only valid after an `Approved` record whose `decision` is `approve`; a `reject` decision halts the proposal.
- `GovProposeParams.manifest_base`, when supplied, **must** equal the patch document’s `base_manifest_hash`; handlers should reject proposals where they differ.

### Plan and Effect Lifecycle (Runtime)

Runtime journal entries are canonical CBOR enums; the important ones for AIR plans are:

- **DomainEvent** `{ schema, value, key? }` – emitted by reducers or plan raise_event; replay feeds reducers and triggers.
- **EffectIntent** `{ intent_hash, kind, cap_name, params_cbor, idempotency_key, origin }` – queued effects from reducers and plans.
- **EffectReceipt** `{ intent_hash, adapter_id, status, payload_cbor, cost_cents?, signature }` – adapters’ signed receipts; replay reproduces plan/resume behavior.
- **PlanResult** `{ plan_name, plan_id, output_schema, value_cbor }` – appended when an `end` step returns a value; shadow/governance tooling can surface outputs directly from the journal without re-running expressions.
- **Snapshot** `{ snapshot_ref, height }` – pointer to CAS snapshot blob; enables fast replay.
- **Governance** – proposal/shadow/approve/apply records (design-time control plane).

### Budget and Capability (Optional, for Observability)

- **BudgetExceeded** `{ grant_name, dimension:"tokens"|"bytes"|"cents", delta:nat, new_balance:int }`

## 17) Determinism and Replay

Deterministic plan execution, reducer invocations, and expression evaluation guarantee that same manifest + journal + receipts ⇒ identical state.

Effects occur only at the boundary; receipts bind non‑determinism. Replay reuses recorded receipts; shadow‑runs stub effects and report predicted intents/paths up to the first await.

## 18) Error Handling (v1)

**Validation**: Reject patch; journal Proposed → Rejected with reasons.

**Runtime**: Invalid module IO → instance error; `emit_effect` denied → step fails (v1: fail instance) unless guarded; no timeouts in v1 (await persists); cancellation is a governance action.

**Budgets**: Deferred to a future milestone; see `roadmap/vX-future/p4-budgets.md`.

## 19) On‑Disk Expectations

- **Store nodes**: `.aos/store/nodes/sha256/<hash>` (canonical CBOR bytes of AIR nodes)
- **Modules (WASM)**: `modules/<name>@<ver>-<hash>.wasm` (`wasm_hash` = content hash)
- **Blobs**: `.aos/store/blobs/sha256/<hash>`
- **Manifest roots**: `manifest.air.cbor` (binary) and `manifest.air.json` (text)

## 20) Security Model

**Object‑capabilities**: Effects require a CapGrant by name; grants live in kernel state and can be referenced in manifest defaults or `plan.required_caps`.

**No ambient authority**: Modules cannot perform I/O directly.

**Policy gate**: All effects pass through a policy gate before dispatch; decisions are journaled; receipts are signed and verified.

## 20) Examples (Abridged)

20.1 defschema (FeedItem)

```json
{ "$kind":"defschema", "name":"com.acme/FeedItem@1", "type": { "record": { "title": {"text":{}}, "url": {"text":{}} } } }
```

20.2 defcap (http.out@1)

```json
{ "$kind":"defcap", "name":"sys/http.out@1", "cap_type":"http.out", "schema": { "record": { "hosts": { "set": { "text": {} } }, "verbs": { "set": { "text": {} } }, "rpm": { "nat": {} } } }, "enforcer": { "module": "sys/CapEnforceHttpOut@1" } }
```

20.3 defpolicy (allow google rss; deny LLM from reducers)

```json
{ "$kind":"defpolicy", "name":"com.acme/policy@1", "rules": [ { "when": { "effect_kind":"http.request", "cap_name":"cap_http" }, "decision":"allow" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"reducer" }, "decision":"deny" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"plan" }, "decision":"allow" } ] }
```

20.4 defplan (daily_digest)

```json
{ "$kind":"defplan", "name":"com.acme/daily_digest@1", "input": {"unit":{}}, "steps": [
    { "id":"set_url", "op":"assign", "expr": { "text":"https://news.google.com/rss" }, "bind": { "as":"rss_url" } },
    { "id":"fetch", "op":"emit_effect", "kind":"http.request", "params": { "record": { "method": {"text":"GET"}, "url": { "ref":"@var:rss_url" }, "headers": { "map": [] } } }, "cap":"http_out_google", "bind": { "effect_id_as":"fetch_id" } },
    { "id":"wait_fetch", "op":"await_receipt", "for": { "ref":"@var:fetch_id" }, "bind": { "as":"fetch_rcpt" } },
    { "id":"summarize", "op":"emit_effect", "kind":"llm.generate", "params": { "record": { "provider": {"text":"openai"}, "model": {"text":"gpt-4o"}, "temperature": {"dec128":"0.2"}, "max_tokens": {"nat": 400 }, "input_ref": { "ref": "@var:fetch_rcpt.body_ref" } } }, "cap":"llm_basic", "bind": { "effect_id_as":"sum_id" } },
    { "id":"wait_sum", "op":"await_receipt", "for": { "ref":"@var:sum_id" }, "bind": { "as":"sum_rcpt" } },
    { "id":"send", "op":"emit_effect", "kind":"http.request", "params": { "record": { "method": {"text":"POST"}, "url": {"text":"https://api.mail/send"}, "headers": { "map": [] }, "body_ref": { "ref":"@var:sum_rcpt.output_ref" } } }, "cap":"mailer", "bind": { "effect_id_as":"send_id" } },
    { "id":"wait_send", "op":"await_receipt", "for": { "ref":"@var:send_id" }, "bind": { "as":"send_rcpt" } },
    { "id":"done", "op":"end" }
  ],
  "edges": [
    { "from":"set_url", "to":"fetch" },
    { "from":"fetch", "to":"wait_fetch" },
    { "from":"wait_fetch", "to":"summarize" },
    { "from":"summarize", "to":"wait_sum" },
    { "from":"wait_sum", "to":"send" },
    { "from":"send", "to":"wait_send" },
    { "from":"wait_send", "to":"done" }
  ],
  "required_caps": ["http_out_google","llm_basic","mailer"],
  "allowed_effects": ["http.request","llm.generate"]
 }
```

## 21) Implementation Guidance (Engineering Notes)

- Build order: canonical CBOR + hashing → store/loader/validator → Wasmtime reducer/pure ABIs + schema checks → effect manager + adapters (http/fs/timer/llm) + receipts → plan executor (DAG + Expr) → patcher + governance loop → shadow‑run.
- Determinism tests: golden “replay or die” snapshots; fuzz Expr evaluator and CBOR canonicalizer.
- Errors: precise validator diagnostics (name, step id, path). Journal policy decisions and validation failures with structured details for explainers.

## Non‑Goals (v1)

- No user‑defined functions/macros in AIR.
- No recursion/loops in plans (use multiple instances or reducers for iteration).
- No migrations/marks; defmigration reserved.
- No external policy engines (OPA/CEL); add behind a gate later.
- No WASM Component Model/WIT in v1 (define forward‑compatible WIT, adopt later).

## Conclusions

AIR v1 is a small, typed, canonical IR for the control plane: schemas, modules, plans, capabilities, policies, and the manifest. Plans are finite DAGs with a tiny, pure expression set, enabling deterministic validation, simulation, governance, and replay. Heavy compute lives in deterministic WASM modules; effects are explicit, capability‑gated intents reconciled by signed receipts.

Everything is canonical CBOR and content‑addressed, yielding auditability, portability, and safety without requiring a complex DSL. AIR's homoiconic nature—representing the control plane as data—is what enables AgentOS's design-time mode: the system can safely inspect, simulate, and modify its own definition using the same deterministic substrate it uses for runtime execution.
