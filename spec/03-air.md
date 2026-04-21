# AIR v1 Specification

AIR (Agent Intermediate Representation) is a small, typed, canonical control‑plane IR that AgentOS loads, validates, diffs/patches, shadow‑simulates, and executes deterministically. AIR is not a general‑purpose programming language; heavy computation runs in deterministic WASM modules. AIR orchestrates schemas, modules, effects, routing, secrets, and receipts: the control plane of a world.

**JSON Schema references** (may evolve; kept in this repo):
- spec/schemas/common.schema.json
- spec/schemas/defschema.schema.json
- spec/schemas/defmodule.schema.json
- spec/schemas/defeffect.schema.json
- spec/schemas/defsecret.schema.json
- spec/schemas/manifest.schema.json

**Built-in catalogs** (data files; loaded by the kernel):
- spec/defs/builtin-schemas.air.json
- spec/defs/builtin-schemas-sdk.air.json
- spec/defs/builtin-schemas-host.air.json
- spec/defs/builtin-effects.air.json
- spec/defs/builtin-modules.air.json

These schemas validate structure. Semantic checks (type compatibility, name/hash resolution, routing compatibility, effect allowlists, effect catalog emitter constraints, and effect payload schemas) are enforced by the kernel validator.

## Goals and Scope

AIR v1 provides one canonical, typed control plane the kernel can load, validate, diff/patch, shadow‑run, and execute deterministically.

AIR is **control‑plane only**. It defines schemas, modules, effects, secrets, routing, and the manifest. Application state lives in workflow module state (deterministic WASM), encoded as canonical CBOR.

The public v0.22 surface has no caps, cap grants, cap slots, or policy language. A workflow may emit an effect when the effect is declared, listed in that workflow's `effects_emitted` contract, and allowed by the effect catalog's emitter constraints. This is an authoring/runtime contract, not a hosted security boundary. The effects set in v1 includes `http.request`, `blob.{put,get}`, `timer.set`, `llm.generate`, `vault.{put,rotate}`, `workspace.*`, and `introspect.*`. Migrations are deferred; `defmigration` is reserved.

## 1) Vocabulary and Identity

**Kind**: One of `defschema`, `defmodule`, `defeffect`, `defsecret`, or `manifest`. (`defmigration` is reserved for future use.)

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

The loader **MUST** accept either lens at every typed value position, resolve the schema from context (module IO, effect params, workflow schemas, event payloads, etc.), and convert to a typed value before hashing.

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

When hashing a typed value (module IO, effect params, event payloads, etc.), always bind the **schema hash** alongside the canonical bytes. This prevents two different schemas that serialize to the same JSON shape from colliding and keeps “schema + value” as the identity pair.

### 3.4 Event Payload Normalization (Journal Invariant)

DomainEvents and ReceiptEvents are treated exactly like effect params:

- **Ingress rule**: Every event payload is decoded against its declared `defschema` (from workflow ABI/routing subscription for module delivery, or built-in receipt schemas when synthesizing receipt events), validated, canonicalized, and re-encoded as canonical CBOR. If validation fails, the event is rejected. For workflow-origin receipts, the kernel **wraps** the receipt payload into the workflow's ABI event schema before routing and journal append.
- **Journal rule**: The journal stores and replays only these canonical bytes; replay never rewrites payloads.
- **Routing/correlation**: Key extraction for routed/correlated events uses the schema-aware decoded value (not ExprValue tagging) and validates against the workflow's `key_schema`.
- **Sources**: Workflow-emitted DomainEvents, adapter receipts synthesized as events, and externally injected/CLI events all flow through the same normalizer.

## 4) Manifest

The manifest is the root catalog of a world's control plane. It lists all schemas, modules, effects, and secrets by name and hash, and defines event routing.

### Shape

```
{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [{name, hash}],
  "modules": [{name, hash}],
  "effects": [{name, hash}],
  "effect_bindings"?: [{kind: EffectKind, adapter_id: text}],
  "secrets"?: [{name, hash} | SecretDecl],
  "routing": {
    "subscriptions": [{event: SchemaRef, module: Name, key_field?: text}],
    "inboxes": [{source: text, workflow: Name}]
  }
}
```

### Rules

Names must be unique per kind; all hashes must exist in the store. `air_version` is **required**; v1 manifests must set it to `"1"`. Supplying an unknown version or omitting the field is a validation error. `routing.subscriptions` maps DomainEvents on the bus to workflow modules; **the routed schema must equal the workflow ABI event schema in `defmodule.abi.workflow.event`** (use a variant schema to accept multiple event shapes, including receipt envelopes; workflows that never emit effects may use a record event schema directly). `routing.inboxes` maps external adapter inboxes (e.g., `http.inbox:contact_form`) to workflows for messages that skip the DomainEvent bus. For keyed workflows, include `key_field` to tell the kernel where to extract the key from the event payload (validated against the workflow's `key_schema`); when the event schema is a variant, `key_field` typically targets the wrapped value (e.g., `$value.note_id`). The `effects` list is the authoritative catalog of effect kinds for this world. **List every schema/effect your world uses**; built-in schemas/effects are not auto-included. Tooling may still fill the canonical hash for built-ins when the name is present without a hash. `effect_bindings` maps external effect `kind` to logical `adapter_id`; bindings must reference declared effect kinds, must not duplicate kinds, and must not include internal effect kinds (`workspace.*`, `introspect.*`, `governance.*`).

See: spec/schemas/manifest.schema.json

## 5) defschema

Defines a named type used for values, events, workflow state, module IO, and effect payloads.

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

- **workflow**: deterministic state machine
- **pure**: deterministic, side‑effect‑free function

### Shape

```json
{
  "$kind": "defmodule",
  "name": "namespace/name@version",
  "module_kind": "workflow" | "pure",
  "wasm_hash": <Hash>,
  "abi": {
    "workflow": {
      "state": <SchemaRef>,
      "event": <SchemaRef>,
      "context"?: <SchemaRef>,
      "annotations"?: <SchemaRef>,
      "effects_emitted"?: [<EffectKind>…]
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

`EffectKind` is a namespaced string. The schema no longer hardcodes an enum; v1 ships a built-in catalog listed in §7, and adapters can introduce additional kinds as runtime support lands.

**Built-in modules** live in `spec/defs/builtin-modules.air.json` (for example, `sys/Workspace@1`). The kernel ships the workspace workflow module (`sys/Workspace@1`) to provide a versioned tree registry. `sys/*` module names are reserved: external manifests may **reference** them, but may not define them; the kernel supplies the definitions and hashes.

The `key_schema` field documents the key type when this workflow module is routed as keyed. The ABI remains a single `step` export; the kernel provides an envelope with optional call context. When routed as keyed, `sys/WorkflowContext@1` includes `cell_mode=true` and the keyed `key`; returning `state=null` deletes the cell instance.

### ABI

Workflow export: `step(ptr, len) -> (ptr, len)`

- **Input**: CBOR envelope including optional call context (see [spec/04-workflows.md](04-workflows.md))
- **Output**: CBOR `{state, domain_events?, effects?, ann?}`

Workflow input envelope (canonical CBOR):

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
Use workflow modules for stateful logic; use pure modules for deterministic transforms and validators.

### Call Context (optional)

Modules may declare a `context` schema in their ABI. If omitted, the kernel does **not**
send a context envelope to that module. Built-in contexts:

- `sys/WorkflowContext@1`: workflow call context (now/logical time, entropy, journal metadata, workflow/key).
- `sys/PureContext@1`: pure module call context (logical time + journal/manifest metadata).

`sys/WorkflowContext@1` fields include `now_ns`, `logical_now_ns`, `journal_height`, `entropy` (64 bytes),
`event_hash`, `manifest_hash`, `workflow`, `key`, and `cell_mode`. `sys/PureContext@1` includes
`logical_now_ns`, `journal_height`, `manifest_hash`, and `module`.

See: spec/schemas/defmodule.schema.json

## 7) Effect Catalog (Built-in v1)

`EffectKind` is an open namespaced string; the core schema no longer freezes the list. The catalog is now **data-driven via `defeffect` nodes** listed in `manifest.effects` plus the built-in bundle (`spec/defs/builtin-effects.air.json`). Canonical effect parameter/receipt schemas live under `spec/defs/builtin-schemas.air.json` and `spec/defs/builtin-schemas-host.air.json` so workflow modules and adapters all hash the same shapes. Workflow SDK/runtime support schemas live under `spec/defs/builtin-schemas-sdk.air.json`. Tooling can stay strict for these built-ins while leaving space for adapter-defined kinds in future versions by deriving enums from the `defeffect` set.

`origin_scope` on each `defeffect` gates who may emit it. In active semantics, effects may be emitted by workflow modules, by system/governance flows, or by both. “Micro-effects” are those whose `origin_scope` allows workflow modules (currently `blob.put`, `blob.get`, `timer.set` in v1).

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
  "binding_id": "env:LLM_API_KEY"
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

**workspace.resolve** (system/governance tooling in workflow runtime)
- params: `{ workspace:text, version?:nat }`
- receipt: `{ exists:bool, resolved_version?:nat, head?:nat, root_hash?:hash }`

**workspace.empty_root** (system/governance tooling in workflow runtime)
- params: `{ workspace:text }`
- receipt: `{ root_hash:hash }`

**workspace.list** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path?:text, scope?:text, cursor?:text, limit:nat }`
- receipt: `{ entries:[{ path, kind, hash?, size?, mode? }], next_cursor?:text }`

**workspace.read_ref** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path:text }`
- receipt: `{ kind, hash, size, mode }` or `null` when missing

**workspace.read_bytes** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path:text, range?:{ start:nat, end:nat } }`
- receipt: `bytes`

**workspace.write_bytes** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path:text, bytes:bytes, mode?:nat }`
- receipt: `{ new_root_hash:hash, blob_hash:hash }`

**workspace.write_ref** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path:text, blob_hash:hash, mode?:nat }`
- receipt: `{ new_root_hash:hash, blob_hash:hash }`

**workspace.remove** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path:text }`
- receipt: `{ new_root_hash:hash }`

**workspace.diff** (system/governance tooling in workflow runtime)
- params: `{ root_a:hash, root_b:hash, prefix?:text }`
- receipt: `{ changes:[{ path, kind, old_hash?, new_hash? }] }`

**workspace.annotations_get** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path?:text }`
- receipt: `{ annotations?:map<text,hash> }`

**workspace.annotations_set** (system/governance tooling in workflow runtime)
- params: `{ root_hash:hash, path?:text, annotations_patch:map<text,option<hash>> }`
- receipt: `{ new_root_hash:hash, annotations_hash:hash }`

Workspace paths are URL-safe relative paths: segments match `[A-Za-z0-9._~-]`, no empty segments, `.` or `..`, and no leading or trailing `/`. Tree nodes are `sys/WorkspaceTree@2`/`sys/WorkspaceEntry@2` (annotations stored via optional `annotations_hash` on directories and entries). Entries are lexicographically sorted, file modes are `0644`/`0755`, and directory mode is `0755`. `workspace.remove` errors on non-empty directories; `workspace.read_bytes.range` uses `[start,end)` and errors if `end` exceeds file size.

**introspect.manifest / introspect.workflow_state / introspect.journal_head / introspect.list_cells** (system/governance tooling in workflow runtime)
- Read-only effects served by an internal kernel adapter; receipts include consistency metadata used by governance and self-upgrade flows.
- `introspect.manifest`: params `{ consistency: text }` (`head` | `exact:<h>` | `at_least:<h>`); receipt `{ manifest:bytes, meta:{ journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.workflow_state`: params `{ workflow:text, key?:bytes, consistency:text }`; receipt `{ state?:bytes, meta:{ journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.journal_head`: params `{}`; receipt `{ meta:{ journal_height, snapshot_hash?, manifest_hash } }`
- `introspect.list_cells`: params `{ workflow:text }`; receipt `{ cells:[{ key, state_hash, size, last_active_ns }], meta:{ journal_height, snapshot_hash?, manifest_hash } }`

There are no public capability types in the v0.22 AIR surface. Future hosted authority may add a smaller authority profile model driven by concrete tenant, secret, network, and budget requirements.

### Built-in workflow receipt events

Workflow modules that emit effects receive normalized receipt events. AIR v1 reserves these `defschema` names so workflow ABI event variants can include them and count on stable payloads:

| Schema | Purpose | Fields |
| --- | --- | --- |
| **`sys/EffectReceiptEnvelope@1`** | Canonical receipt envelope for workflow-origin effects. | `origin_module_id:text` (Name format), `origin_instance_key?:bytes`, `intent_id:text`, `effect_kind:text`, `params_hash?:text`, `issuer_ref?:text`, `receipt_payload:bytes`, `status:"ok" \| "error" \| "timeout"`, `emitted_at_seq:nat`, `adapter_id:text`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/EffectReceiptRejected@1`** | Receipt fault envelope emitted when receipt payload/schema normalization fails. | `origin_module_id:text` (Name format), `origin_instance_key?:bytes`, `intent_id:text`, `effect_kind:text`, `params_hash?:text`, `issuer_ref?:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `error_code:text`, `error_message:text`, `payload_hash:text`, `payload_size:nat`, `emitted_at_seq:nat` |
| **`sys/TimerFired@1`** | Delivery of a `timer.set` receipt back to the originating workflow. | `intent_hash:hash`, `workflow:text` (Name format), `effect_kind:text` (always `"timer.set"` in v1), `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/TimerSetParams@1`, `receipt:sys/TimerSetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobPutResult@1`** | Delivery of a `blob.put` receipt to the workflow. | `intent_hash:hash`, `workflow:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobPutParams@1`, `receipt:sys/BlobPutReceipt@1`, `cost_cents?:nat`, `signature:bytes` |
| **`sys/BlobGetResult@1`** | Delivery of a `blob.get` receipt to the workflow. | `intent_hash:hash`, `workflow:text` (Name format), `effect_kind:text`, `adapter_id:text`, `status:"ok" \| "error" \| "timeout"`, `requested:sys/BlobGetParams@1`, `receipt:sys/BlobGetReceipt@1`, `cost_cents?:nat`, `signature:bytes` |

Workflow modules should include `sys/EffectReceiptEnvelope@1` in ABI event variants as the primary receipt path. `sys/EffectReceiptRejected@1` is optional; if absent and a receipt is malformed, the kernel settles that receipt, marks the instance failed, and drops remaining pending receipts for that instance to avoid clogging execution. Legacy typed receipt events (`sys/TimerFired@1`, `sys/BlobPutResult@1`, `sys/BlobGetResult@1`) are compatibility fallbacks when workflow event schemas still expect those shapes.

Canonical JSON definitions for built-in effect parameter/receipt schemas live in `spec/defs/builtin-schemas.air.json` and `spec/defs/builtin-schemas-host.air.json`. Workflow continuation envelopes, runtime contexts, and reusable SDK state schemas live in `spec/defs/builtin-schemas-sdk.air.json`.

## 8) Effect Intents and Receipts

### EffectIntent

An intent is a request to perform an external effect:

```
{
  kind: EffectKind,
  params: ValueCBORRef,
  idempotency_key?: bytes,
  intent_hash: hash
}
```

The `intent_hash` is computed by the kernel over the effect kind, canonical params, and effective idempotency input; adapters verify it.

**Canonical params**: Before hashing or enqueue, the kernel **decodes → schema‑checks → canonicalizes → re‑encodes** `params` using the effect kind's parameter schema (same AIR canonical rules as the loader: `$tag/$value` variants, canonical map/set/option shapes, numeric normalization). The canonical CBOR bytes become `params_cbor` and are the **only** form stored, hashed, and dispatched; non‑conforming params are rejected. This path runs for *every* origin (workflow modules, system/governance flows, injected tooling) so authoring sugar or workflow ABI quirks cannot change intent identity.

**Idempotency key**: system/governance flows may supply an explicit `idempotency_key`; when
omitted, the kernel uses the all-zero key. For workflow-origin effects, the kernel derives the
effective idempotency input from workflow origin identity and emission position, including the
workflow/module identity, instance key, emitted sequence, effect index, and any workflow-requested
idempotency value. This is why `intent_hash` already behaves as the per-emission open-work id in
the active workflow runtime, rather than only as a pure content hash of effect params.

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

## 9) Simplified Authority Model

The v0.22 public AIR surface has no `defcap`, `defpolicy`, manifest cap grants, module cap slots, or policy defaults. Runtime admission is permissive after structural checks:

- the effect kind must exist in the loaded/built-in effect catalog
- workflow-origin effects must be listed in `defmodule.abi.workflow.effects_emitted`
- the effect catalog's emitter constraints must allow the origin kind
- effect params and receipt payloads remain schema-validated and canonicalized
- open work, idempotency, receipt binding, and replay invariants remain unchanged

This model intentionally does not claim to be a hosted security boundary. Secrets are resolved by explicit world/runner configuration; workflows cannot read ambient secrets.

## 10) defeffect (Effect Catalog Entries)

`defeffect` declares an effect kind, its parameter/receipt schemas, and which emitters may use it.

### Shape

```json
{
  "$kind": "defeffect",
  "name": "sys/http.request@1",
  "kind": "http.request",
  "params_schema": "sys/HttpRequestParams@1",
  "receipt_schema": "sys/HttpRequestReceipt@1",
  "origin_scope": "both",
  "description": "Optional human text"
}
```

### Fields

- `name`: Versioned Name of the effect definition (namespace/name@version)
- `kind`: EffectKind string referenced by workflow modules or system/governance orchestration origins (e.g., `http.request`)
- `params_schema`: SchemaRef for effect parameters
- `receipt_schema`: SchemaRef for effect receipts
- `origin_scope`: schema-defined emitter scope for this effect; semantically this distinguishes workflow-module origins, system/governance origins, or both
- `description?`: Optional prose

### Notes

- Built-in v1 effects live in `spec/defs/builtin-effects.air.json`; include the ones your world uses (hashes may be filled by tooling for built-ins).
- Unknown effect kinds (not declared in the manifest or built-ins) are rejected during normalization/dispatch.
- Workflow receipt translation remains limited to effects whose `origin_scope` allows workflow modules.
- Adapter binding stays out of `defeffect`; routing intent lives in manifest `effect_bindings` so logical adapter ids can evolve without redefining effect kinds.

See: spec/schemas/defeffect.schema.json

For workflow patterns and architecture guidance, use [spec/04-workflows.md](04-workflows.md).

## 11) Validation Rules (Semantic)

The kernel validator enforces these semantic checks:

**Manifest**: Names unique per kind; all references by name resolve to hashes present in the store.

**defmodule**: `wasm_hash` present; referenced schemas exist; `module_kind` is `workflow` or `pure`; keyed workflow routes enforce `key_schema`; `effects_emitted` entries are known effect kinds.

**defeffect**: referenced parameter and receipt schemas exist; `origin_scope` is well-formed.

**Routing**: routed events resolve to schemas and are compatible with workflow ABI event schemas.

## 12) Patch Format (AIR Changes)

Patches describe changes to the control plane (design-time modifications).

**Schema:** `spec/schemas/patch.schema.json` (also embedded in built-ins). Control/daemon paths validate PatchDocuments against this schema before compiling/applying. Authoring sugar is allowed (zero hashes, JSON lens), but payload shape must conform.

### Patch Document

```
{
  version: "1",
  base_manifest_hash: hash,
  patches: [<Patch>…]
  // v1: patches cover defs plus manifest refs/routing/secrets.
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
- **set_routing_events**: `{ pre_hash:hash, subscriptions:[{event, module, key_field?}...] }` — replace routing.subscriptions block (empty list clears)
- **set_routing_inboxes**: `{ pre_hash:hash, inboxes:[{source, workflow}...] }` — replace routing.inboxes block
- **set_secrets**: `{ pre_hash:hash, secrets:[ SecretEntry… ] }` — replace manifest secrets block (refs/decls); no secret values carried in patches.
- **defsecret**: `add_def` / `replace_def` / `remove_def` now accept `defsecret`; `set_manifest_refs` can add/remove secret refs. Secret values still live outside patches; `set_secrets` only adjusts manifest entries.

**System defs are immutable**: Patch compilation rejects any `sys/*` definition edits (add/replace/remove) and any manifest ref updates for `sys/*`. Built-in `sys/*` schemas/effects/modules are provided by the kernel and are not patchable. External manifests/assets may reference `sys/*` entries but may not define them.

### Application

Patches are applied transactionally to yield a new manifest; full re‑validation is required. The governance system turns patches into journal entries: Proposed → (Shadow) → Approved → Applied.

Authoring ergonomics:
- Zero hashes / missing manifest refs are allowed on input; the compiler fills them once nodes are hashed.
- CLI has `--require-hashes` to enforce explicit hashes for stricter flows.
- PatchDocuments can be submitted over control channel; kernel/daemon compiles them server-side so clients don't need to compute hashes.

## 13) Journal Entries (AIR‑Related)

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

- **DomainEvent** `{ schema, value, key?, now_ns, logical_now_ns, journal_height, entropy, event_hash, manifest_hash }` – emitted by workflow modules/system ingress; replay feeds workflows via routing subscriptions.
- **EffectIntent** `{ intent_hash, kind, params_cbor, idempotency_key, origin }` – queued effects from workflow modules and system/governance flows.
- **EffectReceipt** `{ intent_hash, adapter_id, status, payload_cbor, cost_cents?, signature, now_ns, logical_now_ns, journal_height, entropy, manifest_hash }` – adapters’ signed receipts; replay reproduces workflow receipt progression.
- **Snapshot** `{ snapshot_ref, height, logical_time_ns, receipt_horizon_height?, manifest_hash? }` – baseline snapshot record used as a restore root; replay loads the active baseline and replays tail entries with `height >= baseline.height`.
- **Governance** – proposal/shadow/approve/apply records (design-time control plane).

Ingress-stamped fields (`now_ns`, `logical_now_ns`, `journal_height`, `entropy`, `manifest_hash`, and `event_hash` for DomainEvent) are sampled by the kernel at ingress and replayed verbatim. `event_hash` is the sha256 of the canonical DomainEvent envelope (`schema`, `value`, `key`).

### Budget and Authority (Deferred)

Budgets and hosted authority profiles are deferred; no budget events are emitted in v1.

## 14) Determinism and Replay

Deterministic workflow module execution and canonical expression/value evaluation guarantee that same manifest + journal + receipts ⇒ identical state.

Effects occur only at the boundary; receipts bind non‑determinism. Replay reuses recorded receipts; shadow‑runs stub effects and report predicted intents and receipt progression.

## 15) Error Handling (v1)

**Validation**: Reject patch; journal Proposed → Rejected with reasons.

**Runtime**: Invalid module IO → workflow instance error; undeclared/disallowed `emit_effect` intents fail deterministically at enqueue; adapter timeouts are represented as timeout receipts; cancellation is a governance action.

**Budgets**: Deferred to a future milestone; see `roadmap/vX-future/p4-budgets.md`.

## 16) On‑Disk Expectations

- **Authored world root**: `manifest.air.cbor` (binary), `manifest.air.json` (text), and `aos.sync.json`.
- **Local authoring/cache state**: `.aos/`
- **Node-managed state root**: `.aos-node/` by default for `aos node up`.
- **SQLite journal state**: `.aos-node/journal.sqlite3` when using the SQLite journal backend.
- **CAS bytes**: `.aos/cas/<shard>/<digest>` or `.aos-node/cas/<shard>/<digest>` depending on the active authoring/runtime path.
- **Module/engine caches**: `.aos/cache/{modules,wasmtime}/`

## 17) Security Model

**No ambient authority**: Modules cannot perform I/O directly.

**Effect allowlists**: Workflow modules can only emit effect kinds listed in `abi.workflow.effects_emitted`; `origin_scope` additionally gates workflow/system/governance origins.

**Receipts**: Effects execute at the edge and return signed receipts; replay uses recorded receipts rather than re-running external work.

## 18) Examples (Abridged)

18.1 defschema (FeedItem)

```json
{ "$kind":"defschema", "name":"com.acme/FeedItem@1", "type": { "record": { "title": {"text":{}}, "url": {"text":{}} } } }
```

18.2 defeffect (http.request@1)

```json
{ "$kind":"defeffect", "name":"sys/http.request@1", "kind":"http.request", "params_schema":"sys/HttpRequestParams@1", "receipt_schema":"sys/HttpRequestReceipt@1", "origin_scope":"both" }
```

18.3 defmodule (workflow) excerpt

```json
{
  "$kind":"defmodule",
  "name":"com.acme/daily_digest@1",
  "module_kind":"workflow",
  "wasm_hash":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "abi":{
    "workflow":{
      "state":"com.acme/DigestState@1",
      "event":"com.acme/DigestEvent@1",
      "effects_emitted":["http.request","llm.generate"]
    }
  }
}
```

## 19) Implementation Guidance (Engineering Notes)

- Build order: canonical CBOR + hashing → store/loader/validator → Wasmtime workflow/pure ABIs + schema checks → effect runtime + adapters (http/blob/timer/llm/vault) + receipts → patcher + governance loop → shadow‑run.
- Determinism tests: golden “replay or die” snapshots; fuzz Expr evaluator and CBOR canonicalizer.
- Errors: precise validator diagnostics (name, step id, path). Journal validation failures with structured details for explainers.

## Non‑Goals (v1)

- No user‑defined functions/macros in AIR.
- No user-defined orchestration DSL in AIR; orchestration is implemented in workflow modules + routing.
- No migrations/marks; defmigration reserved.
- No external policy engines (OPA/CEL); authority profiles are deferred until there are concrete hosted needs.
- No WASM Component Model/WIT in v1 (define forward‑compatible WIT, adopt later).

## Conclusions

AIR v1 is a small, typed, canonical IR for the control plane: schemas, modules, effects, routing, secrets, receipts, and the manifest. Workflow orchestration is event-driven via module routing and receipt delivery, enabling deterministic validation, simulation, governance, and replay. Heavy compute lives in deterministic WASM modules; effects are explicit intents reconciled by signed receipts.

Everything is canonical CBOR and content‑addressed, yielding auditability, portability, and safety without requiring a complex DSL. AIR's homoiconic nature—representing the control plane as data—is what enables AgentOS's design-time mode: the system can safely inspect, simulate, and modify its own definition using the same deterministic substrate it uses for runtime execution.
