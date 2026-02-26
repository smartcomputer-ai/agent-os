# AIR v1 Specification

AIR (Agent Intermediate Representation) is a small, typed, canonical control‑plane IR that AgentOS loads, validates, diffs/patches, shadow‑simulates, and executes deterministically. AIR is not a general‑purpose programming language; heavy computation runs in deterministic WASM modules. AIR orchestrates modules, effects, capabilities, policies, and routing—the control plane of a world.

**JSON Schema references** (may evolve; kept in this repo):
- spec/schemas/common.schema.json
- spec/schemas/defschema.schema.json
- spec/schemas/defmodule.schema.json
- spec/schemas/defplan.schema.json (legacy compatibility only)
- spec/schemas/defcap.schema.json
- spec/schemas/defpolicy.schema.json
- spec/schemas/defsecret.schema.json
- spec/schemas/manifest.schema.json

**Built-in catalogs** (data files; loaded by the kernel):
- spec/defs/builtin-schemas.air.json
- spec/defs/builtin-effects.air.json
- spec/defs/builtin-caps.air.json
- spec/defs/builtin-modules.air.json

These schemas validate structure. Semantic checks (type compatibility, name/hash resolution, routing compatibility, capability bindings) are enforced by the kernel validator.

## Goals and Scope

AIR v1 provides one canonical, typed control plane the kernel can load, validate, diff/patch, shadow‑run, and execute deterministically.

AIR is **control‑plane only**. It defines schemas, modules, effects, capabilities, policies, and the manifest. Application state lives in workflow module state (deterministic WASM), encoded as canonical CBOR.

The policy engine is minimal: ordered allow/deny rules. Hooks are reserved for richer policy later. The effects set in v1 includes `http.request`, `blob.{put,get}`, `timer.set`, `llm.generate`, `vault.{put,rotate}`, `workspace.*`, and `introspect.*`. Migrations are deferred; `defmigration` is reserved.

## 1) Vocabulary and Identity

**Kind**: One of `defschema`, `defmodule`, `defeffect`, `defcap`, `defpolicy`, `defsecret`, or `manifest`. (`defmigration` is reserved for future use.) Legacy `defplan` nodes may still be encountered in historical assets but are not part of active v1 manifest semantics.

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

The loader **MUST** accept either lens at every typed value position, resolve the schema from context (module IO, effect params, reducer schemas, capability params, etc.), and convert to a typed value before hashing.

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

When hashing a typed value (module IO, cap params, etc.), always bind the **schema hash** alongside the canonical bytes. This prevents two different schemas that serialize to the same JSON shape from colliding and keeps “schema + value” as the identity pair.

### 3.4 Event Payload Normalization (Journal Invariant)

DomainEvents and ReceiptEvents are treated exactly like effect params:

- **Ingress rule**: Every event payload is decoded against its declared `defschema` (from reducer ABI/routing subscription for module delivery, or built-in receipt schemas when synthesizing receipt events), validated, canonicalized, and re-encoded as canonical CBOR. If validation fails, the event is rejected. For reducer micro-effect receipts, the kernel **wraps** the receipt payload into the reducer’s ABI event schema before routing and journal append.
- **Journal rule**: The journal stores and replays only these canonical bytes; replay never rewrites payloads.
- **Routing/correlation**: Key extraction for routed/correlated events uses the schema-aware decoded value (not ExprValue tagging) and validates against the reducer’s `key_schema`.
- **Sources**: Reducer-emitted DomainEvents, adapter receipts synthesized as events, and externally injected/CLI events all flow through the same normalizer.

## 4) Manifest

The manifest is the root catalog of a world's control plane. It lists all schemas, modules, effects, capabilities, and policies by name and hash, defines event routing, and specifies defaults.

### Shape

```
{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [{name, hash}],
  "modules": [{name, hash}],
  "effects": [{name, hash}],
  "caps": [{name, hash}],
  "policies": [{name, hash}],
  "secrets"?: [{name, hash} | SecretDecl],
  "routing": {
    "subscriptions": [{event: SchemaRef, module: Name, key_field?: text}],
    "inboxes": [{source: text, reducer: Name}]
  },
  "defaults": {policy?: Name, cap_grants?: [CapGrant…]},
  "module_bindings"?: {Name → {slots: {slot_name → CapGrantName}}}
}
```

### Rules

Names must be unique per kind; all hashes must exist in the store. `air_version` is **required**; v1 manifests must set it to `"1"`. Supplying an unknown version or omitting the field is a validation error. `routing.subscriptions` maps DomainEvents on the bus to workflow modules; **the routed schema must equal the reducer ABI event schema in `defmodule.abi.reducer.event`** (use a variant schema to accept multiple event shapes, including micro-effect receipts; reducers that never emit micro-effects may use a record event schema directly). `routing.inboxes` maps external adapter inboxes (e.g., `http.inbox:contact_form`) to reducers for messages that skip the DomainEvent bus. For keyed reducers, include `key_field` to tell the kernel where to extract the key from the event payload (validated against the reducer's `key_schema`); when the event schema is a variant, `key_field` typically targets the wrapped value (e.g., `$value.note_id`). The `effects` list is the authoritative catalog of effect kinds for this world. **List every schema/effect your world uses**; built-in schemas/effects are not auto-included. Built-in caps and modules are available even if omitted from `manifest.caps`/`manifest.modules`. Tooling may still fill the canonical hash for built-ins when the name is present without a hash.

See: spec/schemas/manifest.schema.json

## 5) defschema

Defines a named type used for values, events, reducer state, module IO, and effect payloads.

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

- **workflow**: deterministic state machine (legacy alias `"reducer"` is accepted by loaders)
- **pure**: deterministic, side‑effect‑free function

### Shape

```json
{
  "$kind": "defmodule",
  "name": "namespace/name@version",
  "module_kind": "workflow" | "pure",
  "wasm_hash": <Hash>,
  "abi": {
    "reducer": {
      "state": <SchemaRef>,
      "event": <SchemaRef>,
      "context"?: <SchemaRef>,
      "annotations"?: <SchemaRef>,
      "effects_emitted"?: [<EffectKind>…],
      "cap_slots"?: {slot_name: <CapType>}
    },
    "pure": {
      "input": <SchemaRef>,
      "output": <SchemaRef>,
      "context"?: <SchemaRef>
    }
  },
"key_schema"?: <SchemaRef>
}
```

`EffectKind` and `CapType` are namespaced strings. The schema no longer hardcodes an enum; v1 ships a built-in catalog listed in §7, and adapters can introduce additional kinds as runtime support lands.

**Built-in modules** live in `spec/defs/builtin-modules.air.json` (e.g., `sys/CapEnforceHttpOut@1`, `sys/CapEnforceLlmBasic@1`, `sys/Workspace@1`). The kernel ships the workspace workflow module (`sys/Workspace@1`) and its cap enforcer (`sys/CapEnforceWorkspace@1`) to provide a versioned tree registry. `sys/*` module names are reserved: external manifests may **reference** them, but may not define them; the kernel supplies the definitions and hashes.

The `key_schema` field (v1.1 addendum) documents the key type when this workflow module is routed as keyed. The ABI remains a single `step` export; the kernel provides an envelope with optional call context. When routed as keyed, the reducer context includes `cell_mode=true` and the keyed `key`; returning `state=null` deletes the cell instance.

### ABI

Reducer export: `step(ptr, len) -> (ptr, len)`

- **Input**: CBOR envelope including optional call context (see Call Context + Cells spec)
- **Output**: CBOR `{state, domain_events?, effects?, ann?}`

Reducer input envelope (canonical CBOR):

```
{
  version: 1,
  state: <bytes|null>,
  event: { schema: Name, value: bytes, key?: bytes },
  ctx?: <bytes>    // canonical CBOR for declared context schema
}
```

Pure export: `run(ptr, len) -> (ptr, len)`

Pure input envelope (canonical CBOR):

```
{
  version: 1,
  input: <bytes>,
  ctx?: <bytes>    // canonical CBOR for declared context schema
}
```

### Determinism

No WASI ambient syscalls, no threads, no ambient clock or randomness. All I/O happens via the effect layer. Deterministic time/entropy are supplied only via the optional call context. Prefer `dec128` in values; normalize NaNs if floats are used internally.

**Note**: Pure modules (stateless, side-effect-free functions) are supported as `module_kind: "pure"`.
Use workflow modules (reducer ABI) for stateful logic; use pure modules for deterministic transforms and authorizers.

### Call Context (optional)

Modules may declare a `context` schema in their ABI. If omitted, the kernel does **not**
send a context envelope to that module. Built-in contexts:

- `sys/ReducerContext@1`: reducer call context (now/logical time, entropy, journal metadata, reducer/key).
- `sys/PureContext@1`: pure module call context (logical time + journal/manifest metadata).

`sys/ReducerContext@1` fields include `now_ns`, `logical_now_ns`, `journal_height`, `entropy` (64 bytes),
`event_hash`, `manifest_hash`, `reducer`, `key`, and `cell_mode`. `sys/PureContext@1` includes
`logical_now_ns`, `journal_height`, `manifest_hash`, and `module`.

See: spec/schemas/defmodule.schema.json

## 7) Effect Catalog (Built-in v1)

`EffectKind` is an open namespaced string; the core schema no longer freezes the list. The catalog is now **data-driven via `defeffect` nodes** listed in `manifest.effects` plus the built-in bundle (`spec/defs/builtin-effects.air.json`). Canonical parameter/receipt schemas live under `spec/defs/builtin-schemas.air.json` so workflow modules, reducers, and adapters all hash the same shapes. Tooling can stay strict for these built-ins while leaving space for adapter-defined kinds in future versions by deriving enums from the `defeffect` set.

`origin_scope` on each `defeffect` gates who may emit it. The current schema uses compatibility labels: `"reducer"` means workflow-module reducer ABI emission, `"plan"` means non-reducer orchestration origins (system/governance/tooling), and `"both"` allows either. “Micro-effects” are those whose `origin_scope` allows reducers (currently `blob.put`, `blob.get`, `timer.set` in v1).

Built-in kinds in v1:

**http.request**
- params: `{ method:text, url:text, headers: map{text→text}, body_ref?:hash }`
- receipt: `{ status:int, headers: map{text→text}, body_ref?:hash, timings:{start_ns:nat,end_ns:nat}, adapter_id:text }`

**blob.put**
- params: `{ bytes:bytes, blob_ref?:hash, refs?:list<hash> }`
- receipt: `{ blob_ref:hash, edge_ref:hash, size:nat }`

**blob.get**
- params: `{ blob_ref:hash }`
- receipt: `{ blob_ref:hash, size:nat, bytes:bytes }`

**timer.set**
- params: `{ deliver_at_ns:nat, key?:text }` (`deliver_at_ns` uses logical time)
- receipt: `{ delivered_at_ns:nat, key?:text }`

**llm.generate**
- params: `{ provider:text, model:text, temperature:dec128, max_tokens?:nat, message_refs:list<hash>, tool_refs?:list<hash>, tool_choice?:sys/LlmToolChoice@1, api_key?:TextOrSecretRef }`
- receipt: `{ output_ref:hash, raw_output_ref?:hash, token_usage:{prompt:nat,completion:nat}, cost_cents:nat, provider_id:text }`

LLM secrets use `defsecret` + `SecretRef` so workflow manifests and module intents never carry plaintext. v0.9 resolvers read
`env:VAR_NAME` bindings from process env (and `.env` when loaded).

Example `defsecret` for an LLM API key:
```json
{
  "$kind": "defsecret",
  "name": "llm/api@1",
  "binding_id": "env:LLM_API_KEY",
  "allowed_caps": ["cap_llm"]
}
```

Example secret ref in `llm.generate` params:
```json
{
  "api_key": { "secret": { "alias": "llm/api", "version": 1 } }
}
```

**vault.put**
- params: `{ alias:text, binding_id:text, value_ref:hash, expected_digest:hash }`
- receipt: `{ alias:text, version:nat, binding_id:text, digest:hash }`

**vault.rotate**
- params: `{ alias:text, version:nat, binding_id:text, expected_digest:hash }`
- receipt: `{ alias:text, version:nat, binding_id:text, digest:hash }`

**workspace.resolve** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ workspace:text, version?:nat }`
- receipt: `{ exists:bool, resolved_version?:nat, head?:nat, root_hash?:hash }`

**workspace.empty_root** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ workspace:text }`
- receipt: `{ root_hash:hash }`

**workspace.list** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path?:text, scope?:text, cursor?:text, limit:nat }`
- receipt: `{ entries:[{ path, kind, hash?, size?, mode? }], next_cursor?:text }`

**workspace.read_ref** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path:text }`
- receipt: `{ kind, hash, size, mode }` or `null` when missing

**workspace.read_bytes** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path:text, range?:{ start:nat, end:nat } }`
- receipt: `bytes`

**workspace.write_bytes** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path:text, bytes:bytes, mode?:nat }`
- receipt: `{ new_root_hash:hash, blob_hash:hash }`

**workspace.remove** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path:text }`
- receipt: `{ new_root_hash:hash }`

**workspace.diff** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_a:hash, root_b:hash, prefix?:text }`
- receipt: `{ changes:[{ path, kind, old_hash?, new_hash? }] }`

**workspace.annotations_get** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path?:text }`
- receipt: `{ annotations?:map<text,hash> }`

**workspace.annotations_set** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `workspace`)
- params: `{ root_hash:hash, path?:text, annotations_patch:map<text,option<hash>> }`
- receipt: `{ new_root_hash:hash, annotations_hash:hash }`

Workspace paths are URL-safe relative paths: segments match `[A-Za-z0-9._~-]`, no empty segments, `.` or `..`, and no leading or trailing `/`. Tree nodes are `sys/WorkspaceTree@2`/`sys/WorkspaceEntry@2` (annotations stored via optional `annotations_hash` on directories and entries). Entries are lexicographically sorted, file modes are `0644`/`0755`, and directory mode is `0755`. `workspace.remove` errors on non-empty directories; `workspace.read_bytes.range` uses `[start,end)` and errors if `end` exceeds file size.

**introspect.manifest / introspect.reducer_state / introspect.journal_head / introspect.list_cells** (system/governance tooling in workflow runtime; `origin_scope: "plan"`, cap_type `query`)
- Read-only effects served by an internal kernel adapter; receipts include consistency metadata used by governance and self-upgrade flows.
- `introspect.manifest`: params `{ consistency: text }` (`head` | `exact:<h>` | `at_least:<h>`); receipt `{ manifest, journal_height, snapshot_hash?, manifest_hash }`
- `introspect.reducer_state`: params `{ reducer:text, key_b64?:text, consistency:text }`; receipt `{ state_b64?:text, meta:{ journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.journal_head`: params `{}`; receipt `{ journal_height, snapshot_hash?, manifest_hash }`
- `introspect.list_cells`: params `{ reducer:text }`; receipt `{ cells:[{ key_b64, state_hash, size, last_active_ns }], meta:{ journal_height, snapshot_hash?, manifest_hash } }`

Built-in capability types paired with these effects (v1): `http.out`, `blob`, `timer`, `llm.basic`, `secret`, `query`, and `workspace`. The schema stays open to future types even though the kernel ships this curated set today.

### Built-in workflow receipt events

Workflow modules that emit effects receive normalized receipt events. AIR v1 reserves these `defschema` names so reducer ABI event variants can include them and count on stable payloads:

| Schema | Purpose | Fields |
| --- | --- | --- |
| **`sys/EffectReceiptEnvelope@1`** | Canonical receipt envelope for workflow-origin effects. | `origin_module_id:text` (Name format), `origin_instance_key?:bytes`, `intent_id:text`, `effect_kind:text`, `params_hash?:text`, `receipt_payload:bytes`, `status:"ok" \| "error" \| "timeout"`, `emitted_at_seq:nat`, `adapter_id:text`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/EffectReceiptRejected@1`** | Receipt fault envelope emitted when receipt payload/schema normalization fails. | `origin_module_id:text` (Name format), `origin_instance_key?:bytes`, `intent_id:text`, `effect_kind:text`, `params_hash?:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `error_code:text`, `error_message:text`, `payload_hash:text`, `payload_size:nat`, `emitted_at_seq:nat` |
| **`sys/TimerFired@1`** | Delivery of a `timer.set` receipt back to the originating reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text` (always `"timer.set"` in v1), `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/TimerSetParams@1`, `receipt:sys/TimerSetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobPutResult@1`** | Delivery of a `blob.put` receipt to the reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobPutParams@1`, `receipt:sys/BlobPutReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobGetResult@1`** | Delivery of a `blob.get` receipt to the reducer. | `intent_hash:hash`, `reducer:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobGetParams@1`, `receipt:sys/BlobGetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |

Workflow reducers should include `sys/EffectReceiptEnvelope@1` in ABI event variants as the primary receipt path. `sys/EffectReceiptRejected@1` is optional; if absent and a receipt is malformed, the kernel settles that receipt, marks the instance failed, and drops remaining pending receipts for that instance to avoid clogging execution. Legacy typed receipt events (`sys/TimerFired@1`, `sys/BlobPutResult@1`, `sys/BlobGetResult@1`) are compatibility fallbacks when reducer event schemas still expect those shapes.

Canonical JSON definitions for these schemas (plus their parameter/receipt companions) live in `spec/defs/builtin-schemas.air.json` so manifests can hash and reference them directly.

## 8) Effect Intents and Receipts

### EffectIntent

An intent is a request to perform an external effect:

```
{
  kind: EffectKind,
  params: ValueCBORRef,
  cap: CapGrantName,
  idempotency_key?: bytes,
  intent_hash: hash
}
```

The `intent_hash` = `sha256(cbor(kind, params, cap, idempotency_key))` is computed by the kernel; adapters verify it.

**Canonical params**: Before hashing or enqueue, the kernel **decodes → schema‑checks → canonicalizes → re‑encodes** `params` using the effect kind's parameter schema (same AIR canonical rules as the loader: `$tag/$value` variants, canonical map/set/option shapes, numeric normalization). The canonical CBOR bytes become `params_cbor` and are the **only** form stored, hashed, and dispatched; non‑conforming params are rejected. This path runs for *every* origin (workflow modules, system/governance flows, injected tooling) so authoring sugar or reducer ABI quirks cannot change intent identity.

**Idempotency key**: workflow modules and system/governance flows may supply an explicit `idempotency_key`; when omitted, the kernel uses the all‑zero key. Re‑emitting an identical effect with the same key yields the same `intent_hash`.

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
  "cap_type": <CapType>,
  "schema": <SchemaRef>,
  "enforcer"?: { "module": "sys/CapAllowAll@1" }
}
```

The schema defines parameter constraints enforced at enqueue time. The enforcer is a deterministic module invoked by the kernel during authorization; if omitted, the kernel defaults to `sys/CapAllowAll@1` (a built-in allow-all enforcer).

### Standard v1 Capability Types (built-in)

Built-in `defcap` entries live in `spec/defs/builtin-caps.air.json` and are auto-available; manifests may omit them from `manifest.caps` as long as grants reference them by name.

**sys/http.out@1**
- Schema: `{ hosts?: set<text>, schemes?: set<text>, methods?: set<text>, ports?: set<nat>, path_prefixes?: set<text> }`
- At enqueue: enforce allowlists when present (host/scheme/method/port/path_prefix).

**sys/llm.basic@1**
- Schema: `{ providers?: set<text>, models?: set<text>, max_tokens?: nat, tools_allow?: set<text> }`
- At enqueue: `provider`/`model` ∈ allowlists if present; `max_tokens` ≤ cap `max_tokens`; `tools ⊆ tools_allow`.

**sys/blob@1**
- Schema: `{ namespaces?: set<text> }` (minimal in v1)

**sys/timer@1**
- Schema: `{}` (no constraints in v1)

**sys/query@1**
- Schema: `{ scope?: text }` (`scope` is optional/semantically freeform; empty/none = all)
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
Grants are canonicalized at load; kernels may compute a stable `grant_hash` from
`{defcap_ref, cap_type, params_cbor, expiry_ns}` for auditing.

### Enforcement

**At enqueue, the kernel checks:**
1. Grant exists and has not expired (expiry checked against `logical_now_ns`).
2. Capability type matches effect kind.
3. Effect params satisfy grant constraints via the enforcer module.
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
- `kind`: EffectKind string referenced by workflow modules or orchestration origins (e.g., `http.request`)
- `params_schema`: SchemaRef for effect parameters
- `receipt_schema`: SchemaRef for effect receipts
- `cap_type`: Capability type that must guard this effect
- `origin_scope`: `"reducer" | "plan" | "both"`; reducers/workflow modules may emit reducer/both; non-reducer orchestration origins emit plan/both
- `description?`: Optional prose

### Notes

- Built-in v1 effects live in `spec/defs/builtin-effects.air.json`; include the ones your world uses (hashes may be filled by tooling for built-ins).
- Unknown effect kinds (not declared in the manifest or built-ins) are rejected during normalization/dispatch.
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
- `cap_type?: CapType` – capability type of the resolved grant (http.out, llm.basic, etc.)
- `origin_kind?: "workflow" | "system" | "governance"` – effect origin category (legacy `"plan"`/`"reducer"` aliases map to `"workflow"`)
- `origin_name?: Name` – the specific workflow/system/governance origin Name

### Decision (v1)

**"allow"** or **"deny"** only. `"require_approval"` is reserved for v1.1+ (not implemented in v1).

### Semantics

**First match wins** at enqueue time; if no rule matches, the default is **deny**. The kernel populates `origin_kind` and `origin_name` on each EffectIntent from context (workflow module invocation or system/governance flow). Policy is evaluated **after** capability constraint checks; both must pass for dispatch.

Policy matching works over open strings: custom effect kinds are allowed as long as the runtime has a catalog entry mapping that kind to a capability type and schemas. Unknown effect kinds (not in the built-in catalog or a registered adapter catalog) are rejected during validation/dispatch before policy evaluation.

Decisions are journaled as `policy_decision { intent_hash, policy_name, rule_index, decision }` and `cap_decision { intent_hash, effect_kind, cap_name, cap_type, grant_hash, enforcer_module, decision, deny?, expiry_ns?, logical_now_ns }`.

### Recommended Default Policy

- **Deny** `llm.generate`, `payment.*`, `email.*`, and `http.request` to non-allowlisted hosts from `origin_kind="workflow"`.
- **Allow** heavy effects only from specific workflow/system origins under tightly scoped CapGrants.

### v1 Removals and Deferrals

- **Removed**: limits (rpm, daily_budget) from rules; rate limiting deferred to v1.1+.
- **Removed**: path_prefix from Match (use CapGrant.path_prefixes).
- **Removed**: principal from Match (identity/authn deferred to v1.1+).

See: spec/schemas/defpolicy.schema.json

## 12) Legacy defplan (Compatibility Only)

`defplan` is no longer part of active v1 manifest semantics. Current kernels run workflow orchestration through workflow modules (`defmodule` with `module_kind: "workflow"`) plus event routing and receipt delivery.

Notes:
- Legacy `defplan` assets may still exist in historical worlds; loaders may preserve compatibility behavior (for example, ignore-or-translate paths) but new control-plane authoring should not depend on `defplan`.
- Manifest-level `triggers` are removed in favor of `routing.subscriptions`.
- For workflow patterns and architecture guidance, use [spec/05-workflows.md](05-workflows.md).

## 13) Legacy StartPlan API (Removed)

The public `StartPlan` API and trigger-based plan starts are removed from active workflow runtime semantics. Runtime entry is event-driven:

- Domain events are appended and normalized.
- `routing.subscriptions` delivers events to workflow modules.
- Workflow modules emit effects and consume normalized receipt events.

## 14) Validation Rules (Semantic)

The kernel validator enforces these semantic checks:

**Manifest**: Names unique per kind; all references by name resolve to hashes present in the store.

**defmodule**: `wasm_hash` present; referenced schemas exist; `module_kind` is `workflow` or `pure` (legacy `reducer` alias accepted); keyed reducer routes enforce `key_schema`; `effects_emitted`/`cap_slots` (if present) are well‑formed.

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
  // v1: patches cover defs plus manifest refs/defaults/routing/module_bindings/secrets.
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
- **set_routing_events**: `{ pre_hash:hash, subscriptions:[{event, module, key_field?}...] }` — replace routing.subscriptions block (empty list clears)
- **set_routing_inboxes**: `{ pre_hash:hash, inboxes:[{source, reducer}...] }` — replace routing.inboxes block
- **set_module_bindings**: `{ pre_hash:hash, bindings:{ module → { slots:{slot→cap_grant} } } }` — replace module_bindings block
- **set_secrets**: `{ pre_hash:hash, secrets:[ SecretEntry… ] }` — replace manifest secrets block (refs/decls); no secret values carried in patches.
- **defsecret**: `add_def` / `replace_def` / `remove_def` now accept `defsecret`; `set_manifest_refs` can add/remove secret refs. Secret values still live outside patches; `set_secrets` only adjusts manifest entries.

**System defs are immutable**: Patch compilation rejects any `sys/*` definition edits (add/replace/remove) and any manifest ref updates for `sys/*`. Built-in `sys/*` schemas/effects/caps/modules are provided by the kernel and are not patchable. External manifests/assets may reference `sys/*` entries but may not define them.

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
- **ShadowReport** `{ proposal_id:u64, patch_hash, manifest_hash, effects_predicted:[EffectKind…], pending_workflow_receipts?:[PendingWorkflowReceipt], workflow_instances?:[WorkflowInstancePreview], module_effect_allowlists?:[ModuleEffectAllowlist], ledger_deltas?:[LedgerDelta] }`
- **Approved** `{ proposal_id:u64, patch_hash, approver, decision:"approve"|"reject" }`
- **Applied** `{ proposal_id:u64, patch_hash, manifest_hash_new }`
- **Manifest** `{ manifest_hash }`

Notes:
- `proposal_id` is the world-local correlation key; `patch_hash` is the content key and may repeat if the same patch is resubmitted.
- `ShadowReport.manifest_hash` is the candidate manifest root produced by applying the patch (not the patch hash).
- `Applied.manifest_hash_new` is the new manifest root after apply (not the patch hash).
- Apply is only valid after an `Approved` record whose `decision` is `approve`; a `reject` decision halts the proposal.
- `GovProposeParams.manifest_base`, when supplied, **must** equal the patch document’s `base_manifest_hash`; handlers should reject proposals where they differ.
- `Manifest` is appended whenever the active manifest changes (initial boot, governance apply, or `aos push`); replay applies these in-order to swap manifests without emitting new entries.

### Workflow and Effect Lifecycle (Runtime)

Runtime journal entries are canonical CBOR enums; the important ones for AIR workflows are:

- **DomainEvent** `{ schema, value, key?, now_ns, logical_now_ns, journal_height, entropy, event_hash, manifest_hash }` – emitted by workflow modules/system ingress; replay feeds reducers via routing subscriptions.
- **EffectIntent** `{ intent_hash, kind, cap_name, params_cbor, idempotency_key, origin }` – queued effects from workflow modules and system/governance flows.
- **EffectReceipt** `{ intent_hash, adapter_id, status, payload_cbor, cost_cents?, signature, now_ns, logical_now_ns, journal_height, entropy, manifest_hash }` – adapters’ signed receipts; replay reproduces workflow receipt progression.
- **cap_decision** `{ intent_hash, effect_kind, cap_name, cap_type, grant_hash, enforcer_module, decision, deny?, expiry_ns?, logical_now_ns }` – capability checks recorded at enqueue time for audit/replay explainability.
- **policy_decision** `{ intent_hash, policy_name, rule_index?, decision }` – policy allow/deny decision recorded at enqueue time.
- **PlanStarted / PlanResult / PlanEnded** – legacy-named journal records still used for workflow-runtime tracing compatibility.
- **Snapshot** `{ snapshot_ref, height, logical_time_ns, receipt_horizon_height?, manifest_hash? }` – baseline snapshot record used as a restore root; replay loads the active baseline and replays tail entries with `height >= baseline.height`.
- **Governance** – proposal/shadow/approve/apply records (design-time control plane).

Ingress-stamped fields (`now_ns`, `logical_now_ns`, `journal_height`, `entropy`, `manifest_hash`, and `event_hash` for DomainEvent) are sampled by the kernel at ingress and replayed verbatim. `event_hash` is the sha256 of the canonical DomainEvent envelope (`schema`, `value`, `key`).

### Budget and Capability (Optional, for Observability)

Budgets are deferred; no budget events are emitted in v1.

## 17) Determinism and Replay

Deterministic workflow module execution, reducer invocations, and canonical expression/value evaluation guarantee that same manifest + journal + receipts ⇒ identical state.

Effects occur only at the boundary; receipts bind non‑determinism. Replay reuses recorded receipts; shadow‑runs stub effects and report predicted intents and receipt progression.

## 18) Error Handling (v1)

**Validation**: Reject patch; journal Proposed → Rejected with reasons.

**Runtime**: Invalid module IO → workflow instance error; denied `emit_effect` intents fail deterministically at enqueue; adapter timeouts are represented as timeout receipts; cancellation is a governance action.

**Budgets**: Deferred to a future milestone; see `roadmap/vX-future/p4-budgets.md`.

## 19) On‑Disk Expectations

- **Store nodes**: `.aos/store/nodes/sha256/<hash>` (canonical CBOR bytes of AIR nodes)
- **Modules (WASM)**: `modules/<name>@<ver>-<hash>.wasm` (`wasm_hash` = content hash)
- **Blobs**: `.aos/store/blobs/sha256/<hash>`
- **Manifest roots**: `manifest.air.cbor` (binary) and `manifest.air.json` (text)

## 20) Security Model

**Object‑capabilities**: Effects require a CapGrant by name; grants live in kernel state and are referenced via manifest defaults or module/system bindings.

**No ambient authority**: Modules cannot perform I/O directly.

**Policy gate**: All effects pass through a policy gate before dispatch; decisions are journaled; receipts are signed and verified.

## 20) Examples (Abridged)

20.1 defschema (FeedItem)

```json
{ "$kind":"defschema", "name":"com.acme/FeedItem@1", "type": { "record": { "title": {"text":{}}, "url": {"text":{}} } } }
```

20.2 defcap (http.out@1)

```json
{ "$kind":"defcap", "name":"sys/http.out@1", "cap_type":"http.out", "schema": { "record": { "hosts": { "set": { "text": {} } }, "schemes": { "set": { "text": {} } }, "methods": { "set": { "text": {} } }, "ports": { "set": { "nat": {} } }, "path_prefixes": { "set": { "text": {} } } } }, "enforcer": { "module": "sys/CapEnforceHttpOut@1" } }
```

20.3 defpolicy (allow google rss; deny LLM from workflow modules)

```json
{ "$kind":"defpolicy", "name":"com.acme/policy@1", "rules": [ { "when": { "effect_kind":"http.request", "cap_name":"cap_http" }, "decision":"allow" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"workflow" }, "decision":"deny" }, { "when": { "effect_kind":"llm.generate", "origin_kind":"system" }, "decision":"allow" } ] }
```

20.4 defmodule (workflow) excerpt

```json
{
  "$kind":"defmodule",
  "name":"com.acme/daily_digest@1",
  "module_kind":"workflow",
  "wasm_hash":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "abi":{
    "reducer":{
      "state":"com.acme/DigestState@1",
      "event":"com.acme/DigestEvent@1",
      "effects_emitted":["http.request","llm.generate"]
    }
  }
}
```

## 21) Implementation Guidance (Engineering Notes)

- Build order: canonical CBOR + hashing → store/loader/validator → Wasmtime workflow/pure ABIs + schema checks → effect manager + adapters (http/fs/timer/llm) + receipts → patcher + governance loop → shadow‑run.
- Determinism tests: golden “replay or die” snapshots; fuzz Expr evaluator and CBOR canonicalizer.
- Errors: precise validator diagnostics (name, step id, path). Journal policy decisions and validation failures with structured details for explainers.

## Non‑Goals (v1)

- No user‑defined functions/macros in AIR.
- No user-defined orchestration DSL in AIR; orchestration is implemented in workflow modules + routing.
- No migrations/marks; defmigration reserved.
- No external policy engines (OPA/CEL); add behind a gate later.
- No WASM Component Model/WIT in v1 (define forward‑compatible WIT, adopt later).

## Conclusions

AIR v1 is a small, typed, canonical IR for the control plane: schemas, modules, effects, capabilities, policies, and the manifest. Workflow orchestration is event-driven via module routing and receipt delivery, enabling deterministic validation, simulation, governance, and replay. Heavy compute lives in deterministic WASM modules; effects are explicit, capability‑gated intents reconciled by signed receipts.

Everything is canonical CBOR and content‑addressed, yielding auditability, portability, and safety without requiring a complex DSL. AIR's homoiconic nature—representing the control plane as data—is what enables AgentOS's design-time mode: the system can safely inspect, simulate, and modify its own definition using the same deterministic substrate it uses for runtime execution.
