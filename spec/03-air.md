# AIR v2 Specification

AIR (Agent Intermediate Representation) is the typed, canonical control-plane IR that AgentOS
loads, validates, diffs, patches, shadow-simulates, and executes deterministically. AIR is not a
general-purpose programming language. Application logic runs in module runtimes; AIR describes the
schemas, modules, workflows, effects, secrets, manifests, and routing that define a world.

## References

JSON Schemas:

- `spec/schemas/common.schema.json`
- `spec/schemas/defschema.schema.json`
- `spec/schemas/defmodule.schema.json`
- `spec/schemas/defworkflow.schema.json`
- `spec/schemas/defeffect.schema.json`
- `spec/schemas/defsecret.schema.json`
- `spec/schemas/manifest.schema.json`
- `spec/schemas/patch.schema.json`

Built-in catalogs:

- `spec/defs/builtin-schemas.air.json`
- `spec/defs/builtin-schemas-sdk.air.json`
- `spec/defs/builtin-schemas-host.air.json`
- `spec/defs/builtin-modules.air.json`
- `spec/defs/builtin-workflows.air.json`
- `spec/defs/builtin-effects.air.json`

The JSON Schemas validate structure. Semantic validation checks name/hash resolution,
workflow/effect implementation compatibility, routing compatibility, workflow effect allowlists,
effect payload schemas, and
system-name restrictions.

## Goals And Scope

AIR v2 provides one canonical, typed control plane that the kernel can load, validate, patch,
shadow-run, and execute deterministically.

The public v0.22 surface has no caps, policy language, public authority profile, or AIR v1
compatibility mode. AIR v2 accepts only `air_version = "2"` manifests. AIR v1 manifests are rejected
rather than translated.

## 1) Vocabulary And Identity

**Root kind**: one of `defschema`, `defmodule`, `defworkflow`, `defeffect`, `defsecret`, or
`manifest`.

**Definition kind**: one of `defschema`, `defmodule`, `defworkflow`, `defeffect`, or `defsecret`.

**Name**: a versioned identifier with format `namespace/name@version`, where version is a positive
integer. Example: `com.acme/order.step@1`.

**Hashing**: node identity is SHA-256 over canonical CBOR encoding of the full node, including its
`$kind`.

**References**: manifests map Names to content hashes. Within one manifest, names are unique per
definition kind and the referenced hashes must exist in the store.

**Workflow**: a `defworkflow` deterministic state machine that consumes events, owns state, and may
emit declared effects.

**Effect**: a `defeffect` callable effect contract with parameter and receipt schemas plus an
implementation entrypoint.

## 2) Types

AIR defines a small data type language for schema definitions and typed values.

Primitive types:

- `bool`, `int` (i64), `nat` (u64), `dec128`
- `bytes`, `text`
- `time`, `duration`
- `hash`, `uuid`
- `unit`

Composite types:

- `record` `{ field: Type, ... }`
- `variant` `{ Alt: Type, ... }`
- `list`, `set`, `map`
- `option`
- `ref` to another `defschema`

Schema identity is the hash of the fully expanded type AST. Typed value hashing binds both schema
identity and canonical value bytes, so two different schemas with the same JSON shape do not
collide.

## 3) Encoding And Canonicalization

AIR nodes exist in authoring JSON, tagged canonical JSON, and canonical CBOR. All persisted identity
uses canonical CBOR.

The loader accepts authoring sugar and canonical tagged JSON at typed value positions, resolves the
surrounding schema, validates the value, and emits canonical CBOR.

Canonicalization rules:

- CBOR uses deterministic map key ordering, shortest integer encodings, and definite lengths.
- Sets deduplicate by typed equality and sort by canonical element bytes.
- Maps sort by canonical key bytes. JSON objects are accepted only for `map<text, V>`.
- Numeric values are range checked and normalized.
- `dec128` authors as decimal text and encodes as the dedicated decimal payload.
- `time` and `duration` author as integer nanoseconds; tooling may accept richer lenses before
  canonicalization.
- `bytes` author as base64; `hash` authors as `sha256:<64hex>`; `uuid` authors as RFC 4122 text.
- Variant sugar expands to the tagged canonical form before CBOR.
- `option<T>` uses `null` for none in authoring JSON.

Event payloads, effect params, receipt payloads, workflow state, and context envelopes all follow
the same schema-directed normalization path before journal append or hashing.

## 4) Manifest

The manifest is the root catalog for a world.

```json
{
  "$kind": "manifest",
  "air_version": "2",
  "schemas": [{ "name": "com.acme/Event@1", "hash": "sha256:..." }],
  "modules": [{ "name": "com.acme/order_wasm@1", "hash": "sha256:..." }],
  "workflows": [{ "name": "com.acme/order.step@1", "hash": "sha256:..." }],
  "effects": [],
  "secrets": [{ "name": "llm/openai_api@1", "hash": "sha256:..." }],
  "routing": {
    "subscriptions": [
      {
        "event": "com.acme/OrderEvent@1",
        "workflow": "com.acme/order.step@1",
        "key_field": "order_id"
      }
    ]
  }
}
```

Rules:

- `air_version` is required and must be `"2"`.
- `schemas`, `modules`, `workflows`, and `effects` are required arrays. `secrets` defaults to empty.
- `workflows` is the workflow catalog; `effects` is the effect catalog.
- `routing.subscriptions[].workflow` must name a workflow.
- `key_field` is required when the target workflow declares `workflow.key_schema` and rejected
  when the target workflow is unkeyed.
- The routed event schema must either equal the workflow's `workflow.event`, or be a named arm of
  that workflow event variant. Variant-arm delivery wraps the event before workflow invocation.
- `sys/*` definitions are ambient: listing them in a manifest is optional, and external AIR may not
  define or patch them.
- There is no `manifest.ops`, `effect_bindings`, `routing.inboxes`, caps, policies, defaults,
  module bindings, or workflow/effect bindings in AIR v2.

## 5) defschema

```json
{
  "$kind": "defschema",
  "name": "com.acme/OrderEvent@1",
  "type": {
    "record": {
      "order_id": { "text": {} },
      "amount_cents": { "nat": {} }
    }
  }
}
```

Field and variant names must be unique within a schema. Recursive types are not part of AIR v2.

## 6) defmodule

`defmodule` declares runtime and artifact identity. It does not declare workflow ABI, key schema, or
effect contracts.

```json
{
  "$kind": "defmodule",
  "name": "com.acme/order_wasm@1",
  "runtime": {
    "kind": "wasm",
    "artifact": {
      "kind": "wasm_module"
    }
  }
}
```

Supported runtime kinds:

- `wasm`: deterministic workflow modules compiled to a `wasm_module` artifact.
- `python`: future Python runtime support, using `python_bundle` or `workspace_root` artifacts.
- `builtin`: kernel/node supplied built-ins without CAS bytes.

For authored `wasm_module` artifacts, `hash` may be omitted. Loaders treat the missing hash as a
compile-time placeholder and replace it with the compiled WASM blob hash before runtime boot or
manifest persistence.

`sys/*` module names are reserved. External manifests may reference built-in `sys/*` modules but
may not define or patch them.

Removed v1 fields include `module_kind`, `wasm_hash`, `key_schema`, `abi`, `engine`, and
runtime/target metadata outside the v2 schema.

## 7) Workflows And Effects

AIR v2 splits deterministic workflow definitions from effect definitions. Both point at an
implementation entrypoint through `impl`, but they have different runtime contracts.

### Workflow Definition

```json
{
  "$kind": "defworkflow",
  "name": "com.acme/order.step@1",
  "state": "com.acme/OrderState@1",
  "event": "com.acme/OrderEvent@1",
  "context": "sys/WorkflowContext@1",
  "key_schema": "com.acme/OrderId@1",
  "effects_emitted": ["sys/http.request@1"],
  "determinism": "strict",
  "impl": {
    "module": "com.acme/order_wasm@1",
    "entrypoint": "order_step"
  }
}
```

Workflow rules:

- `state`, `event`, and `effects_emitted` are required.
- Workflows that emit no effects must set `"effects_emitted": []` in canonical AIR.
- `effects_emitted[]` names effect definitions, not semantic effect strings.
- `key_schema` makes the workflow keyed; route validation then requires `key_field`.
- `context` and `annotations` are optional schema refs.
- `determinism` defaults to `strict`.

### Effect Definition

```json
{
  "$kind": "defeffect",
  "name": "sys/http.request@1",
  "params": "sys/HttpRequestParams@1",
  "receipt": "sys/HttpRequestReceipt@1",
  "impl": {
    "module": "sys/builtin_effects@1",
    "entrypoint": "http.request"
  }
}
```

Effect rules:

- `effect.params` and `effect.receipt` are required schema refs.
- Dispatch class is resolved from `impl.module`, `impl.entrypoint`, and runtime configuration, not
  from a public effect-kind field.
- Workflow emission names the effect. Intent records, stream frames, receipts, audit records, and
  replay metadata carry the effect identity and, where needed, the resolved effect hash.

`impl.entrypoint` is local to the workflow or effect definition. For WASM it is an exported function
name; for Python it is an import path plus callable; for built-ins it is the built-in dispatcher key.

## 8) defsecret

```json
{
  "$kind": "defsecret",
  "name": "llm/openai_api@1",
  "binding_id": "env:OPENAI_API_KEY",
  "expected_digest": "sha256:..."
}
```

Secret values are never stored in AIR. `binding_id` is an opaque node-local resolver binding. The
optional `expected_digest` can be used by operators to detect resolver drift.

AIR v2 has no per-secret public ACL. A workflow can reach a secret only through an admitted effect
whose parameter schema accepts `SecretRef`, and only if the secret is present in
`manifest.secrets`.

## 9) Built-In Catalogs

Built-in schemas live in `spec/defs/builtin-schemas*.air.json`.

Built-in modules live in `spec/defs/builtin-modules.air.json`, including:

- `sys/builtin_effects@1`
- `sys/workspace_wasm@1`
- `sys/http_publish_wasm@1`

Built-in workflows live in `spec/defs/builtin-workflows.air.json`, including:

- `sys/Workspace@1`
- `sys/HttpPublish@1`

Built-in effects live in `spec/defs/builtin-effects.air.json`, including:

- `sys/http.request@1`
- `sys/blob.put@1`
- `sys/blob.get@1`
- `sys/timer.set@1`
- `sys/llm.generate@1`
- `sys/vault.put@1`
- `sys/vault.rotate@1`
- `sys/workspace.*@1`
- `sys/introspect.*@1`
- `sys/host.*@1`

External manifests may reference `sys/*` entries, but may not define or patch them.

## 10) Workflow ABI

Workflow invocation uses canonical CBOR envelopes.

Input:

```text
{
  version: 1,
  state: bytes | null,
  event: { schema: Name, value: bytes, key?: bytes },
  ctx?: bytes
}
```

Output:

```text
{
  state: bytes | null,
  domain_events?: [{ schema: Name, value: bytes, key?: bytes }],
  effects?: [{ effect: Name, params: bytes, idempotency_key?: bytes, issuer_ref?: text }],
  ann?: bytes
}
```

The kernel normalizes output event payloads and effect params before hashing or journaling. Returning
`state = null` from a keyed workflow deletes that cell.

`sys/WorkflowContext@1` includes deterministic time, entropy, journal metadata, manifest hash,
workflow identity, optional workflow hash, optional key, and `cell_mode`.

## 11) Effect Intents And Receipts

An effect intent records a request to execute one effect definition:

```text
{
  intent_hash: hash,
  effect: Name,
  effect_hash?: hash,
  params_cbor: bytes,
  idempotency_key: bytes,
  origin: recorded workflow/system origin,
  executor_module?: Name,
  executor_module_hash?: hash,
  executor_entrypoint?: text
}
```

Before enqueue, params are decoded against the effect's `params` schema, validated,
canonicalized, and re-encoded as canonical CBOR. The intent hash for workflow-origin effects binds
the origin workflow, instance key, emission position, effect identity, canonical params, and
effective idempotency input.

Terminal receipts bind to open work by `intent_hash`. Generic workflow receipt envelopes carry:

- origin workflow identity and optional workflow hash
- origin instance key when keyed
- intent identity
- effect identity and optional effect hash
- executor module, executor module hash, and entrypoint when resolved
- params hash, issuer ref, receipt payload bytes, status, emitted sequence metadata, cost, and
  signature

## 12) Authority And Admission

AIR v2 public admission is structural:

- the emitted effect must exist in active definitions or the ambient built-in catalog
- workflow-origin effects must come from workflows
- the effect must be listed in the origin workflow's `effects_emitted`
- params must validate against the effect params schema
- open work must be recorded before async execution starts

This is not a hosted security boundary. Network, tenant, budget, and secret policy remain node-local
runtime policy until a future public authority model is defined.

## 13) Patch Format

Patch documents are JSON documents with `version = "2"`:

```json
{
  "version": "2",
  "base_manifest_hash": "sha256:...",
  "patches": [
    { "add_def": { "kind": "defworkflow", "node": { "$kind": "defworkflow" } } },
    {
      "set_routing_subscriptions": {
        "pre_hash": "sha256:...",
        "subscriptions": [{ "event": "com.acme/Event@1", "workflow": "com.acme/workflow@1" }]
      }
    }
  ]
}
```

Operations:

- `add_def`: `{ kind, node }`
- `replace_def`: `{ kind, name, new_node, pre_hash }`
- `remove_def`: `{ kind, name, pre_hash }`
- `set_manifest_refs`: `{ add:[{kind,name,hash}], remove:[{kind,name}] }`
- `set_routing_subscriptions`: `{ pre_hash, subscriptions:[{event,workflow,key_field?}] }`

Patch compilation resolves `base_manifest_hash`, applies operations, canonicalizes new defs,
computes hashes, rewrites manifest refs, stores nodes in CAS, and produces a compiled manifest
patch. `sys/*` definitions and manifest refs are immutable through public patches.

## 14) Journal Records

Important AIR-related journal records include:

- `Manifest { manifest_hash }`
- `Proposed { proposal_id, patch_hash, manifest_base, description? }`
- `ShadowReport { proposal_id, patch_hash, manifest_hash, effects_predicted, pending_workflow_receipts, workflow_instances, module_effect_allowlists }`
- `Approved { proposal_id, patch_hash, approver, decision }`
- `Applied { proposal_id, patch_hash, manifest_hash_new }`
- `DomainEvent { schema, value, key?, stamps..., manifest_hash }`
- `EffectIntent { intent_hash, effect, effect_hash?, params_cbor, idempotency_key, origin, executor_module?, executor_module_hash?, executor_entrypoint? }`
- `EffectReceipt { intent_hash, effect, effect_hash?, payload_cbor, status, cost_cents?, signature, stamps..., manifest_hash }`
- `EffectStreamFrame { intent_hash, effect, effect_hash?, seq, payload, payload_ref?, stamps... }`
- `Snapshot { snapshot_ref, height, logical_time_ns, receipt_horizon_height?, manifest_hash? }`

Ingress-stamped fields are sampled once and replayed verbatim. Replay applies `Manifest` records in
order to swap active manifests without emitting new records.

## 15) Determinism And Replay

Same manifest, same snapshot/checkpoint baseline, same journal frames, same receipts, and same CAS
content must produce byte-identical state. Effects execute only at the edge; replay consumes recorded
receipts and stream frames rather than re-running external work.

## 16) On-Disk Expectations

- authored world root: `air/`, optional `aos.world.json`, `.aos/`
- canonical manifest export: `manifest.air.json` and optional `.aos/manifest.air.cbor`
- local caches: `.aos/cache/{modules,wasmtime}`
- node state root: `.aos-node/`
- SQLite journal: `.aos-node/journal.sqlite3` by default
- CAS bytes: `.aos/cas/...` or `.aos-node/cas/...`

## 17) Non-Goals

- No AIR v1 compatibility or migration layer.
- No public caps/policies/authority profile in v0.22.
- No public `defeffect`, `defcap`, or `defpolicy`.
- No public pure op kind.
- No user-defined orchestration DSL in AIR; orchestration lives in workflow code plus routing.
