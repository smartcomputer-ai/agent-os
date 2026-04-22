# P0: Target Schema Shape

Status: planned.

## Goal

Define the concrete AIR v2 JSON Schema target before implementation. This document is not example AIR. It is the intended shape for `spec/schemas/`.

This phase should settle schema names, required fields, enums, and removals. Implementation happens in later phases.

## Compatibility Decision

AIR v2 replaces AIR v1. v0.22 does not carry an AIR v1 compatibility mode, schema set, loader path,
or manifest migration layer. Once this lands, manifests and nodes that declare `air_version = "1"`
should be rejected rather than translated.

AIR v2 v0.22 has no public capability/policy authority model.
workflow.effects_emitted is only a structural effect allowlist.
Worlds running untrusted workflow code or sensitive effects must rely on node-local runtime policy until authority returns.

## Public Surface

AIR v2 root forms:

```text
defschema
defmodule
defop
defsecret
manifest
```

Removed root forms:

```text
defeffect
defcap
defpolicy
```

Removed manifest fields:

```text
effects
effect_bindings
caps
policies
defaults
module_bindings
op_bindings
```

`op_bindings` stays out of v0.22 unless a later phase identifies a concrete non-authority runtime configuration need.

## `common.schema.json`

Most common type/schema definitions can remain structurally unchanged. The important AIR v2 change is `DefKind`.

Target replacement:

```json
{
  "$defs": {
    "RootKind": {
      "title": "AIR root document kind",
      "type": "string",
      "enum": [
        "defschema",
        "defmodule",
        "defop",
        "defsecret",
        "manifest"
      ]
    },
    "DefKind": {
      "title": "AIR definition kind",
      "type": "string",
      "enum": [
        "defschema",
        "defmodule",
        "defop",
        "defsecret"
      ]
    }
  }
}
```

`RootKind` is for top-level AIR document dispatch. `DefKind` is only for catalog definitions and
must be used by patch operations that add, replace, remove, or reference manifest definition lists.

`EffectKind` remains an open semantic string:

```json
{
  "$defs": {
    "EffectKind": {
      "title": "Effect kind (namespaced string)",
      "description": "Open-ended semantic effect kind identifier. Canonical effect identity is the effect op, not this string.",
      "type": "string",
      "pattern": "^[a-z][a-z0-9_.-]*(\\.[a-z0-9_.-]+)*$"
    }
  }
}
```

## `defmodule.schema.json`

Target complete schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v2/defmodule.schema.json",
  "title": "AIR v2 defmodule",
  "type": "object",
  "properties": {
    "$kind": { "const": "defmodule" },
    "name": { "$ref": "common.schema.json#/$defs/Name" },
    "runtime": { "$ref": "#/$defs/Runtime" }
  },
  "required": ["$kind", "name", "runtime"],
  "additionalProperties": false,
  "$defs": {
    "Runtime": {
      "type": "object",
      "oneOf": [
        { "$ref": "#/$defs/WasmRuntime" },
        { "$ref": "#/$defs/PythonRuntime" },
        { "$ref": "#/$defs/BuiltinRuntime" }
      ]
    },
    "WasmRuntime": {
      "type": "object",
      "properties": {
        "kind": { "const": "wasm" },
        "artifact": { "$ref": "#/$defs/WasmModuleArtifact" }
      },
      "required": ["kind", "artifact"],
      "additionalProperties": false
    },
    "PythonRuntime": {
      "type": "object",
      "properties": {
        "kind": { "const": "python" },
        "python": {
          "type": "string",
          "pattern": "^[0-9]+\\.[0-9]+(\\.[0-9]+)?$"
        },
        "artifact": { "$ref": "#/$defs/PythonArtifact" }
      },
      "required": ["kind", "python", "artifact"],
      "additionalProperties": false
    },
    "BuiltinRuntime": {
      "type": "object",
      "properties": {
        "kind": { "const": "builtin" }
      },
      "required": ["kind"],
      "additionalProperties": false
    },
    "PythonArtifact": {
      "type": "object",
      "oneOf": [
        { "$ref": "#/$defs/PythonBundleArtifact" },
        { "$ref": "#/$defs/WorkspaceRootArtifact" }
      ]
    },
    "WasmModuleArtifact": {
      "type": "object",
      "properties": {
        "kind": { "const": "wasm_module" },
        "hash": { "$ref": "common.schema.json#/$defs/Hash" }
      },
      "required": ["kind", "hash"],
      "additionalProperties": false
    },
    "PythonBundleArtifact": {
      "type": "object",
      "properties": {
        "kind": { "const": "python_bundle" },
        "root_hash": { "$ref": "common.schema.json#/$defs/Hash" }
      },
      "required": ["kind", "root_hash"],
      "additionalProperties": false
    },
    "WorkspaceRootArtifact": {
      "type": "object",
      "properties": {
        "kind": { "const": "workspace_root" },
        "root_hash": { "$ref": "common.schema.json#/$defs/Hash" },
        "path": {
          "type": "string",
          "minLength": 1
        }
      },
      "required": ["kind", "root_hash"],
      "additionalProperties": false
    }
  }
}
```

Runtime field decisions:

```text
runtime.kind is the public runtime discriminator.
engine is not part of AIR v2; Wasmtime, CPython, and node/kernel placement are implementation choices.
artifact.kind is the public artifact discriminator.
runtime.kind restricts which artifact kinds are valid.
wasm supports only compiled wasm_module artifacts in v0.22.
python supports python_bundle and workspace_root artifacts in v0.22.
workspace_root artifacts must be pinned by root_hash; updating code means writing a new workspace root and replacing the defmodule.
workspace_root.path is an optional subpath within the pinned tree; omitted means the tree root.
Builtins use the versioned module name as identity because there are no CAS bytes.
Python target/platform is bundle metadata, not AIR metadata.
```

Removed from `DefModule`:

```text
module_kind
wasm_hash
key_schema
abi
engine
artifact.format
runtime.target
```

## `defop.schema.json`

Target complete schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v2/defop.schema.json",
  "title": "AIR v2 defop",
  "type": "object",
  "properties": {
    "$kind": { "const": "defop" },
    "name": { "$ref": "common.schema.json#/$defs/Name" },
    "op_kind": {
      "type": "string",
      "enum": ["workflow", "effect"]
    },
    "workflow": { "$ref": "#/$defs/WorkflowOp" },
    "effect": { "$ref": "#/$defs/EffectOp" },
    "impl": { "$ref": "#/$defs/OpImpl" }
  },
  "required": ["$kind", "name", "op_kind", "impl"],
  "allOf": [
    {
      "if": { "properties": { "op_kind": { "const": "workflow" } }, "required": ["op_kind"] },
      "then": { "required": ["workflow"], "not": { "required": ["effect"] } }
    },
    {
      "if": { "properties": { "op_kind": { "const": "effect" } }, "required": ["op_kind"] },
      "then": { "required": ["effect"], "not": { "required": ["workflow"] } }
    }
  ],
  "additionalProperties": false,
  "$defs": {
    "WorkflowOp": {
      "type": "object",
      "properties": {
        "state": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "event": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "context": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "annotations": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "key_schema": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "effects_emitted": {
          "type": "array",
          "items": { "$ref": "common.schema.json#/$defs/Name" },
          "uniqueItems": true
        },
        "determinism": {
          "type": "string",
          "enum": ["strict", "checked", "decision_log"],
          "default": "strict"
        }
      },
      "required": ["state", "event", "effects_emitted"],
      "additionalProperties": false
    },
    "EffectOp": {
      "type": "object",
      "properties": {
        "kind": { "$ref": "common.schema.json#/$defs/EffectKind" },
        "params": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "receipt": { "$ref": "common.schema.json#/$defs/SchemaRef" }
      },
      "required": ["kind", "params", "receipt"],
      "additionalProperties": false
    },
    "OpImpl": {
      "type": "object",
      "properties": {
        "module": { "$ref": "common.schema.json#/$defs/Name" },
        "entrypoint": {
          "type": "string",
          "minLength": 1
        }
      },
      "required": ["module", "entrypoint"],
      "additionalProperties": false
    }
  }
}
```

Schema-level removals:

```text
workflow.cap_slots
effect.cap_type
effect.origin_scope
effect.execution_class
pure op kind
cap_enforcer op kind
```

`cap_enforcer` can be added later if authority returns. It should not be in v0.22.
`pure` is also out of the v0.22 target because current uses are tests or cap/policy residue. Module
authors can still use private helper functions inside their bundles; AIR does not expose them as
independently callable world ops in this phase.

`effect.origin_scope` is removed because workflows are the only public AIR origin that can request
effects. A workflow must list the effect op in `workflow.effects_emitted[]`; that is the public
structural authority check. System, governance, and owner-internal operations remain implementation
paths, not public effect emitters in AIR v2.

`workflow.effects_emitted` is required in canonical AIR. Workflows that emit no effects must set
`"effects_emitted": []`. Authoring sugar may allow omission only if the loader materializes the
empty array before canonical hashing/CBOR.

`effect.execution_class` is removed because dispatch class is runtime implementation metadata. The
node resolves the execution path from the effect op implementation and the active runtime registry,
not from a public schema field and not from semantic kind prefixes.

`workflow.key_schema` replaces v1 `defmodule.key_schema`. In v1, `defmodule` carried both the
artifact and the workflow ABI. In AIR v2, `defmodule` is only the runtime/artifact declaration, so
keyedness belongs to the workflow op whose state cells are being addressed.

`workflow.event` is the single input schema for a workflow op. AIR v2 does not add a parallel
`workflow.events[]` list. A routing subscription event is deliverable when it exactly equals the
target workflow op's `workflow.event`, or when `workflow.event` is a variant whose arm is a ref to
the subscription event schema. In the variant-arm case, runtime delivery wraps the incoming event as
that variant arm before invoking the workflow. Routable workflow event variants must use named
schema refs for arms so subscription matching is unambiguous.

`routing.subscriptions[].key_field` remains route-local event extraction. Semantic validation must
require `key_field` when the target workflow op has `workflow.key_schema`, reject it when the target
workflow op has no key schema, and verify that the extracted field type matches the workflow key
schema.

Invocation convention is inferred from the referenced module's `runtime.kind` and the op's `op_kind`.
The schema does not expose a separate ABI selector until there are multiple supported conventions for
the same runtime/op-kind pair.

`impl.entrypoint` is an op-local entrypoint selector, not a module-kind marker. For WASM modules it is
the exported function name to invoke; for Python modules it is the import path plus callable name; for
builtins it is the built-in dispatcher key. The value `"workflow"` is not special. A single
content-addressed WASM module can export many workflow ops, each with a different `defop.impl.entrypoint`.

## `defsecret.schema.json`

Target complete schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v2/defsecret.schema.json",
  "title": "AIR v2 defsecret",
  "type": "object",
  "properties": {
    "$kind": { "const": "defsecret" },
    "name": { "$ref": "common.schema.json#/$defs/Name" },
    "binding_id": {
      "type": "string",
      "minLength": 1,
      "description": "Opaque node-local resolver binding ID, such as env:SLACK_BOT_TOKEN."
    },
    "expected_digest": {
      "$ref": "common.schema.json#/$defs/Hash",
      "description": "Optional hash of resolved secret plaintext for drift detection."
    }
  },
  "required": ["$kind", "name", "binding_id"],
  "additionalProperties": false
}
```

Secret values remain outside AIR. `defsecret.binding_id` names a node-local resolver binding, and
`expected_digest` remains optional.

AIR v2 v0.22 does not add `allowed_ops` or another per-secret op ACL. Secret access follows the
same public structural path as effect access: a workflow must be allowed to emit an effect op through
`workflow.effects_emitted[]`, that effect op's params schema must admit a `SecretRef`, and the
referenced secret must be present in `manifest.secrets`. The resolver then injects the secret for
execution. Stronger per-secret policy remains node-local/runtime policy for v0.22.

## Examples

These examples are illustrative only. The sections above define the actual schema target.

### WASM Workflow

```json
{
  "$kind": "defmodule",
  "name": "acme/order_wasm@1",
  "runtime": {
    "kind": "wasm",
    "artifact": {
      "kind": "wasm_module",
      "hash": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}
```

```json
{
  "$kind": "defop",
  "name": "acme/order.step@1",
  "op_kind": "workflow",
  "workflow": {
    "state": "acme/OrderState@1",
    "event": "acme/OrderEvent@1",
    "context": "sys/WorkflowContext@1",
    "key_schema": "acme/OrderId@1",
    "effects_emitted": [
      "acme/slack.post@1",
      "sys/timer.set@1"
    ],
    "determinism": "strict"
  },
  "impl": {
    "module": "acme/order_wasm@1",
    "entrypoint": "order_step"
  }
}
```

### Python Effect

```json
{
  "$kind": "defmodule",
  "name": "acme/order_bundle@1",
  "runtime": {
    "kind": "python",
    "python": "3.12",
    "artifact": {
      "kind": "python_bundle",
      "root_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
}
```

```json
{
  "$kind": "defop",
  "name": "acme/slack.post@1",
  "op_kind": "effect",
  "effect": {
    "kind": "acme.slack.post",
    "params": "acme/SlackPostParams@1",
    "receipt": "acme/SlackPostReceipt@1"
  },
  "impl": {
    "module": "acme/order_bundle@1",
    "entrypoint": "orders.effects:post_to_slack"
  }
}
```

### Python Workflow

```json
{
  "$kind": "defmodule",
  "name": "acme/order_py@1",
  "runtime": {
    "kind": "python",
    "python": "3.12",
    "artifact": {
      "kind": "workspace_root",
      "root_hash": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
      "path": "python/order"
    }
  }
}
```

```json
{
  "$kind": "defop",
  "name": "acme/order.py_step@1",
  "op_kind": "workflow",
  "workflow": {
    "state": "acme/OrderState@1",
    "event": "acme/OrderEvent@1",
    "context": "sys/WorkflowContext@1",
    "key_schema": "acme/OrderId@1",
    "effects_emitted": [
      "acme/slack.post@1"
    ],
    "determinism": "decision_log"
  },
  "impl": {
    "module": "acme/order_py@1",
    "entrypoint": "orders.workflow:step"
  }
}
```

### Python Workspace Artifact

```json
{
  "$kind": "defmodule",
  "name": "acme/order_workspace@1",
  "runtime": {
    "kind": "python",
    "python": "3.12",
    "artifact": {
      "kind": "workspace_root",
      "root_hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "path": "python/order"
    }
  }
}
```

### Builtin Timer Effect

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_effects@1",
  "runtime": {
    "kind": "builtin"
  }
}
```

```json
{
  "$kind": "defop",
  "name": "sys/timer.set@1",
  "op_kind": "effect",
  "effect": {
    "kind": "timer.set",
    "params": "sys/TimerSetParams@1",
    "receipt": "sys/TimerSetReceipt@1"
  },
  "impl": {
    "module": "sys/builtin_effects@1",
    "entrypoint": "timer.set"
  }
}
```

## `manifest.schema.json`

Target complete schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://aos.dev/air/v2/manifest.schema.json",
  "title": "AIR v2 manifest",
  "type": "object",
  "properties": {
    "$kind": { "const": "manifest" },
    "air_version": {
      "type": "string",
      "enum": ["2"],
      "description": "AIR format major version. v2 manifests must set this to \"2\"."
    },
    "schemas": {
      "type": "array",
      "items": { "$ref": "#/$defs/NamedRef" }
    },
    "modules": {
      "type": "array",
      "items": { "$ref": "#/$defs/NamedRef" }
    },
    "ops": {
      "type": "array",
      "items": { "$ref": "#/$defs/NamedRef" }
    },
    "secrets": {
      "type": "array",
      "items": { "$ref": "#/$defs/NamedRef" }
    },
    "routing": { "$ref": "#/$defs/Routing" }
  },
  "required": ["$kind", "air_version", "schemas", "modules", "ops"],
  "additionalProperties": false,
  "$defs": {
    "NamedRef": {
      "type": "object",
      "properties": {
        "name": { "$ref": "common.schema.json#/$defs/Name" },
        "hash": { "$ref": "common.schema.json#/$defs/Hash" }
      },
      "required": ["name", "hash"],
      "additionalProperties": false
    },
    "Routing": {
      "type": "object",
      "properties": {
        "subscriptions": {
          "type": "array",
          "items": { "$ref": "#/$defs/RoutingSubscription" }
        }
      },
      "additionalProperties": false
    },
    "RoutingSubscription": {
      "type": "object",
      "properties": {
        "event": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "op": { "$ref": "common.schema.json#/$defs/Name" },
        "key_field": {
          "type": "string",
          "description": "Field path in event value that carries the cell key for keyed workflow ops."
        }
      },
      "required": ["event", "op"],
      "additionalProperties": false
    }
  }
}
```

Manifest secrets are refs to `defsecret` nodes only. AIR v2 does not allow inline `SecretDecl`
entries in the manifest; adding or changing a secret declaration uses normal definition replacement
plus a manifest ref update.

`routing.subscriptions` is the only public domain ingress table. It maps typed domain events to
workflow ops and may fan out one event to multiple workflows. The old `routing.inboxes` lane was a
direct source-to-workflow route for messages that skipped the domain-event bus. AIR v2 removes it;
external ingress should first become a typed domain event and then route through subscriptions.
Routing subscriptions do not participate in receipt continuation delivery; continuations route by
recorded origin identity.

Removed from `Manifest`:

```text
effects
effect_bindings
routing.inboxes
```

Removed from routing:

```text
RoutingSubscription.module
```

## `defeffect.schema.json`

Target action:

```text
delete spec/schemas/defeffect.schema.json
```

There is no AIR v2 compatibility schema for `defeffect`.

## `patch.schema.json`

Patch documents should accept `defop` through the narrower
`common.schema.json#/$defs/DefKind`. The schema does not need op-specific patch operations.

Patch document format should move to version `"2"` because the operation surface changes with AIR
v2. AIR v2 does not keep patch compatibility with v1 patch documents.

Target field-level changes:

```json
{
  "$id": "https://aos.dev/air/v2/patch.schema.json",
  "title": "AIR v2 Manifest Patch",
  "properties": {
    "version": {
      "type": "string",
      "enum": ["2"],
      "default": "2"
    }
  },
  "$defs": {
    "node_json": {
      "description": "Authoring form of any AIR node: defschema, defmodule, defop, defsecret.",
      "type": "object",
      "minProperties": 1
    },
    "set_routing_subscriptions": {
      "type": "object",
      "properties": {
        "set_routing_subscriptions": {
          "type": "object",
          "properties": {
            "pre_hash": { "$ref": "common.schema.json#/$defs/Hash" },
            "subscriptions": {
              "type": "array",
              "items": { "$ref": "manifest.schema.json#/$defs/RoutingSubscription" }
            }
          },
          "required": ["pre_hash", "subscriptions"],
          "additionalProperties": false
        }
      },
      "required": ["set_routing_subscriptions"],
      "additionalProperties": false
    }
  }
}
```

Patch operation semantics:

```text
add_def / replace_def / remove_def accept only DefKind: defschema, defmodule, defop, and defsecret.
set_manifest_refs accepts only DefKind and updates manifest.schemas, manifest.modules,
manifest.ops, or manifest.secrets.
set_routing_subscriptions replaces routing.subscriptions and uses RoutingSubscription.op.
```

Removed patch operations:

```text
set_routing_inboxes
set_secrets
```

`set_routing_inboxes` is removed with `routing.inboxes`. `set_secrets` is removed because secrets
are ordinary `defsecret` refs in `manifest.secrets`; use `add_def`/`replace_def` plus
`set_manifest_refs` instead.

## Schema Set

Target schema files:

```text
COMMON
DEFSCHEMA
DEFMODULE
DEFOP
DEFSECRET
MANIFEST
PATCH
```

Remove:

```text
DEFEFFECT
```

## Validation Required Beyond JSON Schema

JSON Schema covers structure only. Semantic validation still needs:

1. Every manifest schema ref resolves to a `defschema` or built-in schema.
2. Every manifest module ref resolves to a `defmodule`.
3. Every manifest op ref resolves to a `defop`.
4. Every manifest secret ref resolves to a `defsecret`.
5. Every op implementation references an active module.
6. Every workflow op schema ref exists.
7. Every effect op params and receipt schema ref exists.
8. Every routing subscription references an active workflow op.
9. Every routing subscription event schema must be deliverable to the target workflow op's `workflow.event`: exact match, or a ref arm of a variant workflow event schema.
10. Routable workflow event variant arms must be named schema refs and must not contain duplicate refs.
11. Every workflow `effects_emitted[]` entry references an active effect op.
12. Workflow key-field validation uses the target op's `workflow.key_schema`, not the referenced module.
13. Effect semantic kind duplicates are allowed only if the runtime has a deterministic dispatch rule by op identity. Recommendation: allow duplicates because op identity is canonical.
14. The referenced module runtime kind must support the op kind.
15. Effect execution path must resolve from the referenced module runtime and op implementation.
16. Artifact kind compatibility is enforced by the `defmodule` schema: `wasm` accepts `wasm_module`; `python` accepts `python_bundle` or `workspace_root`; `builtin` has no artifact.
17. `wasm_module.hash` must identify compiled WASM bytes.
18. `python_bundle.root_hash` and `workspace_root.root_hash` must identify a workspace/tree root that the Python runner can hydrate.
19. Python artifact metadata must satisfy the declared `runtime.python` version and provide a compatible target for the runner host.
20. Secret refs in effect params are admitted by the effect op params schema and resolved through manifest `defsecret` declarations; v0.22 has no public per-secret op ACL.

## Open Schema Decisions

- Whether `manifest.routing` should be required. Current target keeps it optional.
- Whether a later authority phase should add `defsecret.allowed_ops` or another per-secret
  resolver ACL. v0.22 intentionally defers this and relies on effect-op admission plus params
  schema shape.

## Done When

- This document is accepted as the target for `spec/schemas/`.
- P1 can implement these schemas without inventing additional public fields.
