# AIR v1 Specification

AIR (Agent Intermediate Representation) is a small, typed, canonical control‑plane IR that AgentOS loads, validates, diffs/patches, shadow‑simulates, and executes deterministically. AIR is not a general‑purpose programming language; heavy computation runs in deterministic WASM modules. AIR orchestrates modules, effects, capabilities, policies, and plans—the control plane of a world.

**JSON Schema references** (may evolve; kept in this repo):
- spec/schemas/common.schema.json
- spec/schemas/defschema.schema.json
- spec/schemas/defmodule.schema.json
- spec/schemas/defplan.schema.json
- spec/schemas/defcap.schema.json
- spec/schemas/defpolicy.schema.json
- spec/schemas/manifest.schema.json

These schemas validate structure. Semantic checks (DAG acyclicity, type compatibility, name/hash resolution, capability bindings) are enforced by the kernel validator.

## Goals and Scope

AIR v1 provides one canonical, typed control plane the kernel can load, validate, diff/patch, shadow‑run, and execute deterministically. It is intentionally **not Turing complete**: plans are finite DAGs with total predicates; no unbounded loops or recursion exist.

AIR is **control‑plane only**. It defines schemas, modules, plans, capabilities, policies, and the manifest. Application state lives in reducer state (deterministic WASM), encoded as canonical CBOR.

The policy engine is minimal: ordered allow/deny rules with budgets enforced on receipts. Hooks are reserved for richer policy later. The effects set in v1 is also minimal: `http.request`, `blob.{put,get}`, `timer.set`, `llm.generate`. Migrations are deferred; `defmigration` is reserved.

## 1) Vocabulary and Identity

**Kind**: One of `defschema`, `defmodule`, `defplan`, `defcap`, `defpolicy`, or `manifest`. (`defmigration` is reserved for future use.)

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

AIR nodes exist in two forms: human-readable text and canonical binary.

### Text Form

JSON with explicit `$kind` and, where needed, `$type` tags for unions. Field order is irrelevant in text form.

### Binary Form

Canonical [CBOR](https://cbor.io/) (RFC 8949) with strict determinism:
- Deterministic map key ordering (bytewise of encoded keys)
- Shortest integer form
- Definite lengths
- `dec128` encoded as tagged byte string (tag 2000) of 16 bytes
- `time` and `duration` as int nanoseconds

### Node Hash

`sha256` over canonical CBOR bytes of the node. Values bound to a schema include the schema_hash in the value node before hashing, which prevents shape collisions.

## 4) Manifest

The manifest is the root catalog of a world's control plane. It lists all schemas, modules, plans, capabilities, and policies by name and hash, defines event routing and triggers, and specifies defaults.

### Shape

```
{
  "$kind": "manifest",
  "schemas": [{name, hash}],
  "modules": [{name, hash}],
  "plans": [{name, hash}],
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

Names must be unique per kind; all hashes must exist in the store. The `routing.events` field is optional in v1. The `triggers` array maps DomainIntent events to plans: when a reducer emits an event matching a trigger's schema, the kernel starts the referenced plan with that event as input.

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

The `key_schema` field (v1.1 addendum) documents the key type when this reducer is routed as keyed. The ABI remains a single `step` export; the kernel provides an envelope with optional key.

### ABI

Reducer export: `step(ptr, len) -> (ptr, len)`

- **Input**: CBOR envelope including optional key (see Cells spec)
- **Output**: CBOR `{state, domain_events?, effects?, ann?}`

### Determinism

No WASI ambient syscalls, no threads, no clock. All I/O happens via the effect layer. Prefer `dec128` in values; normalize NaNs if floats are used internally.

**Note**: Pure modules (stateless, side-effect-free functions) are deferred to v1.1+. Use reducers for all computation in v1.

See: spec/schemas/defmodule.schema.json

## 7) Effect Catalog (Built‑in v1)

AgentOS ships with four built-in effect types. Each has parameter and receipt schemas defined as built-in defschema.

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
- params: `{ provider:text, model:text, temperature:dec128, max_tokens:nat, input_ref:hash, tools?:list<text> }`
- receipt: `{ output_ref:hash, token_usage:{prompt:nat,completion:nat}, cost_cents:nat, provider_id:text }`

### Built-in reducer receipt events

Reducers that emit micro-effects rely on the kernel to translate adapter receipts into typed DomainEvents. AIR v1 reserves these `defschema` names so manifests can declare routing and reducers can count on stable payloads:

| Schema | Purpose | Fields |
| --- | --- | --- |
| **`sys/TimerFired@1`** | Delivery of a `timer.set` receipt back to the originating reducer. | `intent_hash:hash`, `reducer:Name`, `effect_kind:text` (always `"timer.set"` in v1), `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/TimerSetParams@1`, `receipt:sys/TimerSetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobPutResult@1`** | Delivery of a `blob.put` receipt to the reducer. | `intent_hash:hash`, `reducer:Name`, `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobPutParams@1`, `receipt:sys/BlobPutReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobGetResult@1`** | Delivery of a `blob.get` receipt to the reducer. | `intent_hash:hash`, `reducer:Name`, `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobGetParams@1`, `receipt:sys/BlobGetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |

Reducers should add routing entries for these schemas (e.g., `routing.events[].event = sys/TimerFired@1`). Plans typically raise domain-specific result events instead of consuming these `sys/*` receipts. The shared `cost_cents` and `signature` fields exist today so future policy/budget enforcement can trust the same structures without changing reducer code.

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

Capabilities define scoped permissions for effects. A `defcap` declares a capability type; a `CapGrant` is a runtime instance of that capability with concrete constraints and budgets.

### defcap Definition

```json
{
  "$kind": "defcap",
  "name": "namespace/name@version",
  "cap_type": "http.out" | "blob" | "timer" | "llm.basic",
  "schema": <SchemaRef>
}
```

The schema defines parameter constraints enforced at enqueue time.

### Standard v1 Capability Types (built-in)

**sys/http.out@1**
- Schema: `{ hosts: set<text>, verbs: set<text>, path_prefixes?: set<text> }`
- At enqueue: `authority(url) ∈ hosts`; `method ∈ verbs`; path `starts_with` any `path_prefixes` if present.

**sys/llm.basic@1**
- Schema: `{ providers?: set<text>, models?: set<text>, max_tokens_max?: nat, temperature_max?: dec128, tools_allow?: set<text> }`
- At enqueue: `provider`/`model` ∈ allowlists if present; `max_tokens ≤ max_tokens_max`; `temperature ≤ temperature_max`; `tools ⊆ tools_allow`.

**sys/blob@1**
- Schema: `{ namespaces?: set<text> }` (minimal in v1)

**sys/timer@1**
- Schema: `{}` (no constraints in v1)

### CapGrant (Runtime Instance)

A grant is kernel state referenced by name:

```
{
  name: text,
  cap: Name(defcap),
  params: Value,
  expiry_ns?: nat,
  budget?: {tokens?: nat, bytes?: nat, cents?: nat}
}
```

The `params` must conform to the defcap's schema and encode concrete allowlists/ceilings.

### Enforcement

**At enqueue, the kernel checks:**
1. Grant exists and has not expired.
2. Capability type matches effect kind.
3. Effect params satisfy grant constraints (hosts, models, max_tokens_max, etc.).
4. Conservative budget pre-check for variable-cost effects:
   - `llm.generate`: if `max_tokens` declared, check `max_tokens ≤ remaining tokens budget`; deny if insufficient.
   - `blob.put`: if blob_ref size known from CAS, check `size ≤ remaining bytes budget`; deny if insufficient.
5. Policy decision (see defpolicy).

**At receipt, the kernel settles budgets:**
- Decrements actual usage (`token_usage`, blob `size`, `cost_cents`) from grant.
- If a dimension goes negative, mark grant exhausted; future enqueues using that grant are denied until replenished.

See: spec/schemas/defcap.schema.json

## 10) defpolicy (Rule Pack)

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

- `effect_kind?: EffectKind` – which effect kind (http.request, llm.generate, etc.)
- `cap_name?: text` – which CapGrant name
- `host?: text` – host suffix or glob (prefer using CapGrant.hosts instead)
- `method?: text` – HTTP method
- `origin_kind?: "plan" | "reducer"` – whether the effect originates from a plan or a reducer
- `origin_name?: Name` – the specific plan or reducer Name

### Decision (v1)

**"allow"** or **"deny"** only. `"require_approval"` is reserved for v1.1+ (not implemented in v1).

### Semantics

**First match wins** at enqueue time; if no rule matches, the default is **deny**. The kernel populates `origin_kind` and `origin_name` on each EffectIntent from context (plan instance or reducer invocation). Policy is evaluated **after** capability constraint checks; both must pass for dispatch.

Decisions are journaled: `PolicyDecisionRecorded { intent_hash, policy_name, rule_index, decision }`.

### Recommended Default Policy

- **Deny** `llm.generate`, `payment.*`, `email.*`, and `http.request` to non-allowlisted hosts from `origin_kind="reducer"`.
- **Allow** heavy effects only from specific plans under tightly scoped CapGrants.

### v1 Removals and Deferrals

- **Removed**: limits (rpm, daily_budget) from rules; rate limiting deferred to v1.1+.
- **Removed**: path_prefix from Match (use CapGrant.path_prefixes).
- **Removed**: principal from Match (identity/authn deferred to v1.1+).

See: spec/schemas/defpolicy.schema.json

## 11) defplan (Orchestration DAG)

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

Version 1.0 keeps plans minimal: `emit_effect`, `await_receipt`, `raise_event`, `await_event`, `assign`, `end`. Structured concurrency (sub-plans, fan-out/fan-in) is deferred to v1.1+ to validate real-world needs first.

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
  "required_caps": [<CapGrantName>…],
  "allowed_effects": [<EffectKind>…],
  "invariants"?: [<Expr>…]
}
```

### Steps (discriminated by `op`)

**raise_event**: Publish an event to a reducer
- `{ id, op:"raise_event", reducer:Name, key?:Expr, event:Expr }`
- If target reducer is keyed, `key` is required and must typecheck to its key schema

**emit_effect**: Request an external effect
- `{ id, op:"emit_effect", kind:EffectKind, params:Expr, cap:CapGrantName, bind:{effect_id_as:VarName} }`

**await_receipt**: Wait for an effect receipt
- `{ id, op:"await_receipt", for:Expr /*effect_id*/, bind:{as:VarName} }`

**await_event** (optional): Wait for a matching DomainEvent
- `{ id, op:"await_event", event:SchemaRef, where?:Expr, bind:{as:VarName} }`
- Waits until a matching DomainEvent appears; `where` is a boolean predicate over the event value

**assign**: Bind a value to a variable
- `{ id, op:"assign", expr:Expr, bind:{as:VarName} }`

**end**: Complete the plan
- `{ id, op:"end", result?:Expr }`
- Must match output schema if provided

### Expr and Predicates

**Expr** is side‑effect‑free over a typed Value: constants; refs (`@plan.input`, `@var:name`, `@step:ID.field…`); operators `len|get|has|eq|ne|lt|le|gt|ge|and|or|not|concat|add|sub|mul|div|mod|starts_with|ends_with|contains`.

**Predicates** are boolean Expr. Missing refs are errors (deterministic fail).

### Guards (Edge Predicates)

Edges can have optional `when` predicates called **guards**. A guard is a boolean expression that must evaluate to `true` for an edge to be traversable. This enables conditional branching in plan DAGs: a step becomes ready only when all predecessor edges are completed **and** all their guards evaluate to `true`.

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

## 12) StartPlan (Runtime API and Triggers)

Plans can start in two ways:

**Triggered start**: When a DomainIntent event with schema matching a manifest `triggers[].event` is appended, the kernel starts `triggers[].plan` with the event value as input. If `correlate_by` is provided, the kernel records that key for later correlation in await_event/raise_event flows.

**Manual start**: `{ plan:Name, input:ValueCBORRef, bind_locals?:{VarName:ValueCBORRef…} }`

The kernel pins the manifest hash for the instance, checks input/locals against schemas, and executes under the current policy/cap ledger. Effects always check live grants at enqueue time.

## 13) Validation Rules (Semantic)

The kernel validator enforces these semantic checks:

**Manifest**: Names unique per kind; all references by name resolve to hashes present in the store.

**defmodule**: `wasm_hash` present; referenced schemas exist; `effects_emitted`/`cap_slots` (if present) are well‑formed.

**defplan**: DAG acyclic; step ids unique; Expr refs resolve; `emit_effect.kind` ∈ `allowed_effects`; `emit_effect.cap` ∈ `required_caps` or defaults; `await_receipt.for` references earlier emit; `raise_event.event` must evaluate to a value conforming to a declared schema; if the target reducer is keyed (by routing or by `key_schema`), `raise_event.key` is required and must typecheck to that key schema; if `await_event` present, `event` must be a known schema; `end.result` matches output schema.

**defpolicy**: Rule shapes valid; referenced effect kinds known.

**defcap**: `cap_type` in built‑ins; parameter schema compatible.

## 14) Patch Format (AIR Changes)

Patches describe changes to the control plane (design-time modifications).

### Patch Document

```
{
  base_manifest_hash: hash,
  patches: [<Patch>…]
}
```

### Operations

- **add_def**: `{ kind:Kind, node:NodeJSON }` — add a new definition
- **replace_def**: `{ kind:Kind, name:Name, new_node:NodeJSON, pre_hash:hash }` — optimistic swap
- **remove_def**: `{ kind:Kind, name:Name, pre_hash:hash }` — remove a definition
- **set_manifest_refs**: `{ add:[{kind,name,hash}], remove:[{kind,name}] }` — update manifest references
- **set_defaults**: `{ policy?:Name, cap_grants?:[CapGrant…] }` — update default policy and grants

### Application

Patches are applied transactionally to yield a new manifest; full re‑validation is required. The governance system turns patches into journal entries: Proposed → (Shadow) → Approved → Applied.

## 15) Journal Entries (AIR‑Related)

The journal records both design-time (governance) and runtime (execution) events.

### Governance and Control Plane (Design Time)

- **Proposed** `{ patch_hash, author, manifest_base }`
- **ShadowReport** `{ patch_hash, effects_predicted:[EffectKind…], diffs:[typed summary] }`
- **Approved** `{ patch_hash, approver }`
- **Applied** `{ manifest_hash_new }`

### Plan and Effect Lifecycle (Runtime)

- **PlanStarted** `{ plan_name, instance_id, input_hash }`
- **EffectQueued** `{ instance_id, intent_hash, origin_kind, origin_name }`
- **PolicyDecisionRecorded** `{ intent_hash, policy_name, rule_index, decision }`
- **ReceiptAppended** `{ intent_hash, status, receipt_ref }`
- **PlanEnded** `{ instance_id, status:"ok"|"error", result_ref? }`

### Budget and Capability (Optional, for Observability)

- **BudgetExceeded** `{ grant_name, dimension:"tokens"|"bytes"|"cents", delta:nat, new_balance:int }`
  - Appended when a receipt settlement drives a budget dimension negative; grant marked exhausted

## 16) Determinism and Replay

Deterministic plan execution, reducer invocations, and expression evaluation guarantee that same manifest + journal + receipts ⇒ identical state.

Effects occur only at the boundary; receipts bind non‑determinism. Replay reuses recorded receipts; shadow‑runs stub effects and report predicted intents/paths up to the first await.

## 17) Error Handling (v1)

**Validation**: Reject patch; journal Proposed → Rejected with reasons.

**Runtime**: Invalid module IO → instance error; `emit_effect` denied → step fails (v1: fail instance) unless guarded; no timeouts in v1 (await persists); cancellation is a governance action.

**Budgets**: Decrement on receipts; over‑budget → policy denial at enqueue.

## 18) On‑Disk Expectations

- **Store nodes**: `.store/nodes/sha256/<hash>` (canonical CBOR bytes of AIR nodes)
- **Modules (WASM)**: `modules/<name>@<ver>-<hash>.wasm` (`wasm_hash` = content hash)
- **Blobs**: `.store/blobs/sha256/<hash>`
- **Manifest roots**: `manifest.air.cbor` (binary) and `manifest.air.json` (text)

## 19) Security Model

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
{ "$kind":"defcap", "name":"sys/http.out@1", "cap_type":"http.out", "schema": { "record": { "hosts": { "set": { "text": {} } }, "verbs": { "set": { "text": {} } }, "rpm": { "nat": {} } } } }
```

20.3 defpolicy (allow google rss; deny LLM from reducers)

```json
{ "$kind":"defpolicy", "name":"com.acme/policy@1", "rules": [ { "when": { "effect_kind":"http.request", "host":"news.google.com" }, "decision":"allow" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"reducer" }, "decision":"deny" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"plan" }, "decision":"allow" } ] }
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

- Build order: canonical CBOR + hashing → store/loader/validator → Wasmtime reducer/pure ABIs + schema checks → effect manager + adapters (http/fs/timer/llm) + cap ledger + receipts → plan executor (DAG + Expr) → patcher + governance loop → shadow‑run.
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
