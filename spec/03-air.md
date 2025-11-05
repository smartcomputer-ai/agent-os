# AIR v1 Specification (Agent Intermediate Representation)

This document specifies AIR v1: a small, typed, canonical control‑plane IR that AgentOS loads, validates, diffs/patches, shadow‑simulates, and executes deterministically. AIR is not a general‑purpose language; heavy compute runs in deterministic WASM modules. AIR orchestrates modules, effects, capabilities, policies, and plans.

References to JSON Schemas (may evolve; kept in this repo):
- spec/schemas/common.schema.json
- spec/schemas/defschema.schema.json
- spec/schemas/defmodule.schema.json
- spec/schemas/defplan.schema.json
- spec/schemas/defcap.schema.json
- spec/schemas/defpolicy.schema.json
- spec/schemas/manifest.schema.json

We use these schemas for shape validation. Semantic checks (e.g., DAG acyclicity, type compatibility, name/hash resolution, capability bindings) are enforced by the kernel validator.

## Goals and Scope

- One canonical, typed control plane the kernel can load, validate, diff/patch, shadow‑run, and execute deterministically.
- Not Turing complete. Plans are finite DAGs with total predicates; no unbounded loops or recursion.
- Control‑plane only. Schemas, modules, plans, capabilities, policies, and the manifest. Application state lives in reducer state (deterministic WASM), encoded as canonical CBOR.
- Minimal policy engine. Ordered allow/deny/require‑approval rules; budgets enforced on receipts. Hooks reserved for richer policy later.
- Minimal effects set in v1: http.request, fs.blob.{put,get}, timer.set, llm.generate.
- No migrations in v1. defmigration is reserved.

## 1) Vocabulary and Identity

- Kind: one of defschema, defmodule, defplan, defcap, defpolicy, manifest. (defmigration reserved)
- Name: `namespace/name@version` where version is a positive integer, e.g., `com.acme/rss_fetch@1`.
- Hashing: SHA‑256 over canonical CBOR encoding of a node. Nodes embed their `$kind`; schema/value nodes bind type by including the schema hash (see Encoding).
- IDs: use Name or content hash `sha256:HEX`. The manifest maps Names to hashes; the mapping is immutable within a manifest.

## 2) Types (Schemas, IO, Effect Shapes)

Primitive
- bool, int (i64), nat (u64), dec128 (IEEE 754 decimal128 as 16‑byte bytes), bytes, text (UTF‑8), time (int ns since epoch), duration (int ns), hash (32‑byte SHA‑256), uuid (16‑byte RFC 4122).

Composite
- record { field: Type, … } (required fields only in v1)
- variant [ Alt: Type, … ] (closed sums)
- list Type; map { key: Type, value: Type } with key ∈ {int, nat, text, uuid, hash}; set Type for comparable domains; option Type; unit {}

Identity and compatibility
- schema_hash = sha256(cbor(fully expanded type AST)).
- A value claims a schema by including schema_hash during hashing; exact schema match in v1 (no subtyping except option sugar).

See: spec/schemas/common.schema.json and spec/schemas/defschema.schema.json

## 3) Encoding

Text
- JSON with explicit `$kind` and, where needed, `$type` tags for unions. Field order is irrelevant in text.

Binary
- Canonical CBOR (RFC 8949):
  - Deterministic map key ordering (bytewise of encoded keys); shortest integer form; definite lengths.
  - dec128 encoded as tagged byte string (tag 2000) of 16 bytes.
  - time/duration as int nanoseconds.

Node hash
- sha256 over canonical CBOR bytes of the node. Values bound to a schema include the schema_hash in the value node before hashing (prevents shape collisions).

## 4) Manifest

Shape
- `{ "$kind":"manifest", schemas:[{name,hash}], modules:[{name,hash}], plans:[{name,hash}], caps:[{name,hash}], policies:[{name,hash}], routing:{ events:[{event:SchemaRef, reducer:Name, key_field?:text}], inboxes:[{source:text, reducer:Name}] }, triggers:[{ event:SchemaRef, plan:Name, correlate_by?:text }], defaults:{ policy?:Name, cap_grants?:[CapGrant…] }, module_bindings?: { Name → { slots: { slot_name → CapGrantName } } } }`

Rules
- Names unique per kind; all hashes must exist in the store. routing/events optional in v1. `triggers` map DomainIntent events to plans; when such an event is appended, the kernel starts the referenced plan with that event as input.

See: spec/schemas/manifest.schema.json

## 5) defschema

- `{ "$kind":"defschema", "name": Name, "type": TypeAST }`
- No recursive types in v1; unique field/variant names.
- Emits schema_hash.

See: spec/schemas/defschema.schema.json

## 6) defmodule

Kinds
- reducer: deterministic state machine
- pure: deterministic function (no effects, no state)

Shape
- `{ "$kind":"defmodule", "name": Name, "module_kind":"reducer"|"pure", "wasm_hash": Hash, "abi": { "reducer"?: { "state": SchemaRef, "event": SchemaRef, "annotations"?: SchemaRef, "effects_emitted"?: [EffectKind…], "cap_slots"?: { slot_name: CapType } }, "pure"?: { "input": SchemaRef, "output": SchemaRef } }, "key_schema"?: SchemaRef }`
  - `key_schema` (v1.1 addendum): documents the key type when this reducer is routed as keyed; ABI remains a single `step` with a context that may include a key.

ABI
- Reducer export: `step(ptr,len) -> (ptr,len)`; input CBOR envelope includes optional key (see Cells spec); output CBOR `{state, domain_events?, effects?, ann?}`
- Pure export: `run(ptr,len) -> (ptr,len)`; input/output CBOR per declared schemas.

Determinism
- No WASI ambient syscalls; no threads; no clock; all I/O via effect layer. Prefer dec128 in values; normalize NaNs if floats used internally.

See: spec/schemas/defmodule.schema.json

## 7) Effect Catalog (Built‑in v1)

Each effect kind has parameter and receipt schemas (shipped as built‑in defschema):
- http.request
  - params: `{ method:text, url:text, headers: map{text→text}, body_ref?:hash }`
  - receipt: `{ status:int, headers: map{text→text}, body_ref?:hash, timings:{start_ns:nat,end_ns:nat}, adapter_id:text }`
- fs.blob.put
  - params: `{ ns:text, blob_ref:hash }`; receipt: `{ stored_ref:hash, size:nat }`
- fs.blob.get
  - params: `{ ns:text, key:text }`; receipt: `{ blob_ref:hash, size:nat }`
- timer.set
  - params: `{ deliver_at_ns:nat, key?:text }`; receipt: `{ delivered_at_ns:nat, key?:text }`
- llm.generate
  - params: `{ provider:text, model:text, temperature:dec128, max_tokens:nat, input_ref:hash, tools?:list<text> }`
  - receipt: `{ output_ref:hash, token_usage:{prompt:nat,completion:nat}, cost_cents:nat, provider_id:text }`

## 8) Effect Intents and Receipts

EffectIntent
- `{ kind:EffectKind, params: ValueCBORRef, cap: CapGrantName, idempotency_key:hash, intent_hash:hash }`
- `intent_hash = sha256(cbor(kind, params, cap, idempotency_key))` computed by kernel; adapters verify.

Receipt
- `{ intent_hash:hash, adapter_id:text, status:"ok"|"error", receipt: ValueCBORRef, cost?:nat, sig:bytes }`
- Kernel validates signature (ed25519/HMAC), binds to intent, and appends to the journal.

## 9) defcap (Capability Types) and Grants

- defcap: `{ "$kind":"defcap", "name":Name, "cap_type":"http.out"|"fs.blob"|"timer"|"llm.basic", "schema": SchemaRef }`
- CapGrant (kernel state; referenced by name): `{ name:text, cap: Name(defcap), params: Value, expiry_ns?:nat, budget?:{ tokens?:nat, bytes?:nat, cents?:nat } }`
- At emit, kernel checks grant existence/type/scope/expiry/budget.

See: spec/schemas/defcap.schema.json

## 10) defpolicy (Rule Pack)

- `{ "$kind":"defpolicy", "name":Name, "rules":[ Rule… ] }`
- Rule: `{ when: Match, decision: "allow"|"deny"|"require_approval", limits?:{ rpm?:nat, daily_budget?:{ tokens?:nat, bytes?:nat, cents?:nat } } }`
- Match (subset): `{ effect_kind?:EffectKind, cap_name?:text, host?:glob, path_prefix?:text, method?:text, principal?:text }`
- First match wins; default deny. Policy evaluated on enqueue; decisions journaled. Budgets settle on receipt.

See: spec/schemas/defpolicy.schema.json

## 11) defplan (Orchestration DAG)

High level
- Finite DAG of steps producing named outputs in a typed environment. Edges have optional guard predicates. Deterministic scheduler.

Shape
- `{ "$kind":"defplan", "name":Name, "input":SchemaRef, "output"?:SchemaRef, "locals"?:{ name:SchemaRef… }, "steps":[ Step… ], "edges":[ {from:StepId, to:StepId, when?:Expr }… ], "required_caps":[CapGrantName…], "allowed_effects":[EffectKind…], "invariants"?:[Expr…] }`

Steps (discriminated by `op`)
- raise_event: `{ id, op:"raise_event", reducer:Name, key?:Expr, event:Expr }` // if target reducer is keyed, `key` is required and must typecheck to its key schema
- emit_effect: `{ id, op:"emit_effect", kind:EffectKind, params:Expr, cap:CapGrantName, bind:{ effect_id_as:VarName } }`
- await_receipt: `{ id, op:"await_receipt", for:Expr /*effect_id*/, bind:{ as:VarName } }`
- await_event (optional): `{ id, op:"await_event", event:SchemaRef, where?:Expr, bind:{ as:VarName } }` // waits until a matching DomainEvent appears; `where` is a boolean predicate over the event value
- assign: `{ id, op:"assign", expr:Expr, bind:{ as:VarName } }`
- end: `{ id, op:"end", result?:Expr }` (must match output schema if provided)

Expr and Predicate
- Expr is side‑effect‑free over a typed Value: constants; refs (`@plan.input`, `@var:name`, `@step:ID.field…`); operators `len|get|has|eq|ne|lt|le|gt|ge|and|or|not|concat|add|sub|mul|div|mod|starts_with|ends_with|contains`.
- Predicates are boolean Expr. Missing refs are errors (deterministic fail).

Scheduling
- Ready = predecessors completed and guard true. Execute one ready step per tick; deterministic order by step id then insertion order.
- emit_effect parks if nothing else is ready; await_receipt becomes ready when matching receipt is appended.
- Plan completes at `end` or when graph has no outgoing edges (error if output declared but no end).

See: spec/schemas/defplan.schema.json (steps/Expr defined there)

## 12) StartPlan (Runtime API and Triggers)

- Triggered start: when a DomainIntent event with schema matching a manifest `triggers[].event` is appended, the kernel starts `triggers[].plan` with the event value as input. If `correlate_by` is provided, the kernel records that key for later correlation in await_event/raise_event flows.
- Manual start: `{ plan:Name, input:ValueCBORRef, bind_locals?:{ VarName:ValueCBORRef… } }`
- Kernel pins the manifest hash for the instance; checks input/locals; executes under current policy/cap ledger. Effects always check live grants at enqueue time.

## 13) Validation Rules (Semantic)

- Manifest: names unique per kind; all references by name resolve to hashes present in the store.
- defmodule: wasm_hash present; referenced schemas exist; effects_emitted/cap_slots (if present) are well‑formed.
- defplan: DAG acyclic; step ids unique; Expr refs resolve; emit_effect.kind ∈ allowed_effects; emit_effect.cap ∈ required_caps or defaults; await_receipt.for references earlier emit; raise_event.event must evaluate to a value conforming to a declared schema; if the target reducer is keyed (by routing or by `key_schema`), raise_event.key is required and must typecheck to that key schema; if await_event present, `event` must be a known schema; end.result matches output schema.
- defpolicy: rule shapes valid; referenced effect kinds known.
- defcap: cap_type in built‑ins; parameter schema compatible.

## 14) Patch Format (AIR Changes)

Patch document
- `{ base_manifest_hash:hash, patches:[ Patch… ] }`

Operations
- add_def: `{ kind:Kind, node:NodeJSON }` (new name)
- replace_def: `{ kind:Kind, name:Name, new_node:NodeJSON, pre_hash:hash }` (optimistic swap)
- remove_def: `{ kind:Kind, name:Name, pre_hash:hash }`
- set_manifest_refs: `{ add:[{kind,name,hash}], remove:[{kind,name}] }`
- set_defaults: `{ policy?:Name, cap_grants?:[CapGrant…] }`

Application
- Apply transactionally to yield new manifest; full re‑validation required. Governance turns this into Proposed → (Shadow) → Approved → Applied journal entries.

## 15) Journal Entries (AIR‑Related)

- Proposed { patch_hash, author, manifest_base }
- ShadowReport { patch_hash, effects_predicted:[EffectKind…], diffs:[typed summary] }
- Approved { patch_hash, approver }
- Applied { manifest_hash_new }
- PlanStarted { plan_name, instance_id, input_hash }
- EffectQueued { instance_id, intent_hash }
- PolicyDecision { instance_id, intent_hash, rule_index, decision }
- ReceiptAppended { intent_hash, status }
- PlanEnded { instance_id, status:"ok"|"error", result_ref? }

## 16) Determinism and Replay

- Deterministic plan execution, reducer invocations, and expression evaluation: same manifest + journal + receipts ⇒ identical state.
- Effects occur only at the boundary; receipts bind non‑determinism. Replay reuses receipts; shadow‑run stubs effects and reports predicted intents/paths up to first await.

## 17) Error Handling (v1)

- Validation: reject patch; journal Proposed → Rejected with reasons.
- Runtime: invalid module IO → instance error; emit_effect denied → step fails (v1: fail instance) unless guarded; no timeouts in v1 (await persists); cancellation is a governance action.
- Budgets: decrement on receipts; over‑budget → policy denial at enqueue.

## 18) On‑Disk Expectations

- Store nodes: `.store/nodes/sha256/<hash>` (canonical CBOR bytes of AIR nodes)
- Modules (WASM): `modules/<name>@<ver>-<hash>.wasm` (wasm_hash = content hash)
- Blobs: `.store/blobs/sha256/<hash>`
- Manifest roots: `manifest.air.cbor` (binary) and `manifest.air.json` (text)

## 19) Security Model

- Object‑capabilities: effects require a CapGrant by name; grants live in kernel state and can be referenced in manifest defaults or plan.required_caps.
- No ambient authority: modules cannot perform I/O.
- Policy gate in front of dispatch; decisions journaled; receipts signed and verified.

## 20) Examples (Abridged)

20.1 defschema (FeedItem)
- `{ "$kind":"defschema", "name":"com.acme/FeedItem@1", "type": { "record": { "title": {"text":{}}, "url": {"text":{}} } } }`

20.2 defmodule (rss_fetch@1, pure)
- `{ "$kind":"defmodule", "name":"com.acme/rss_fetch@1", "module_kind":"pure", "wasm_hash":"sha256:…", "abi": { "pure": { "input": { "record": { "url": {"text":{}} } }, "output": { "record": { "items": { "list": { "ref": "com.acme/FeedItem@1" } } } } } } }`

20.3 defcap (http.out@1)
- `{ "$kind":"defcap", "name":"sys/http.out@1", "cap_type":"http.out", "schema": { "record": { "hosts": { "set": { "text": {} } }, "verbs": { "set": { "text": {} } }, "rpm": { "nat": {} } } } }`

20.4 defpolicy (allow google rss)
- `{ "$kind":"defpolicy", "name":"com.acme/policy@1", "rules": [ { "when": { "effect_kind":"http.request", "host":"news.google.com" }, "decision":"allow", "limits": { "rpm":60 } }, { "when": { "effect_kind":"llm.generate" }, "decision":"require_approval" } ] }`

20.5 defplan (daily_digest)
- `{ "$kind":"defplan", "name":"com.acme/daily_digest@1", "input": {"unit":{}}, "steps": [
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
 }`

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

AIR v1 is a small, typed, canonical IR for the control plane: schemas, modules, plans, capabilities, policies, and the manifest. Plans are finite DAGs with a tiny, pure expression set, enabling deterministic validation, simulation, governance, and replay. Heavy compute lives in deterministic WASM modules; effects are explicit, capability‑gated intents reconciled by signed receipts. Everything is canonical CBOR and content‑addressed, yielding auditability, portability, and safety without a big DSL.
