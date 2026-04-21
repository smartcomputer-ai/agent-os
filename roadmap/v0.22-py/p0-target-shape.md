# P0: Target Schema Shape

Status: planned.

## Goal

Define the concrete AIR v2 JSON Schema target before implementation. This document is not example AIR. It is the intended shape for `spec/schemas/`.

This phase should settle schema names, required fields, enums, and removals. Implementation happens in later phases.

## Compatibility Decision

AIR v2 replaces AIR v1. v0.22 does not carry an AIR v1 compatibility mode, schema set, loader path,
or manifest migration layer. Once this lands, manifests and nodes that declare `air_version = "1"`
should be rejected rather than translated.

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
    "DefKind": {
      "title": "AIR definition kind",
      "type": "string",
      "enum": [
        "defschema",
        "defmodule",
        "defop",
        "defsecret",
        "manifest"
      ]
    }
  }
}
```

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
        "artifact": { "$ref": "#/$defs/HashedArtifact" }
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
        "artifact": { "$ref": "#/$defs/HashedArtifact" }
      },
      "required": ["kind", "python", "artifact"],
      "additionalProperties": false
    },
    "BuiltinRuntime": {
      "type": "object",
      "properties": {
        "kind": { "const": "builtin" },
        "builtin_id": {
          "type": "string",
          "pattern": "^[a-z][a-z0-9_.-]*(\\.[a-z0-9_.-]+)*$"
        }
      },
      "required": ["kind", "builtin_id"],
      "additionalProperties": false
    },
    "HashedArtifact": {
      "type": "object",
      "properties": {
        "hash": { "$ref": "common.schema.json#/$defs/Hash" }
      },
      "required": ["hash"],
      "additionalProperties": false
    }
  }
}
```

Runtime field decisions:

```text
runtime.kind is the public runtime discriminator.
engine is not part of AIR v2; Wasmtime, CPython, and node/kernel placement are implementation choices.
artifact exists only for content-addressed runtime bundles.
artifact format is implied by runtime.kind in v0.22.
Builtins use builtin_id directly because there are no CAS bytes.
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
      "required": ["state", "event"],
      "additionalProperties": false
    },
    "EffectOp": {
      "type": "object",
      "properties": {
        "kind": { "$ref": "common.schema.json#/$defs/EffectKind" },
        "params": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "receipt": { "$ref": "common.schema.json#/$defs/SchemaRef" },
        "origin_scope": {
          "type": "array",
          "items": {
            "type": "string",
            "enum": ["workflow", "system", "governance"]
          },
          "minItems": 1,
          "uniqueItems": true
        },
        "execution_class": {
          "type": "string",
          "enum": ["internal_deterministic", "owner_local_async", "external_async"]
        }
      },
      "required": ["kind", "params", "receipt", "origin_scope", "execution_class"],
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
pure op kind
cap_enforcer op kind
```

`cap_enforcer` can be added later if authority returns. It should not be in v0.22.
`pure` is also out of the v0.22 target because current uses are tests or cap/policy residue. Module
authors can still use private helper functions inside their bundles; AIR does not expose them as
independently callable world ops in this phase.

Invocation convention is inferred from the referenced module's `runtime.kind` and the op's `op_kind`.
The schema does not expose a separate ABI selector until there are multiple supported conventions for
the same runtime/op-kind pair.

`impl.entrypoint` is an op-local entrypoint selector, not a module-kind marker. For WASM modules it is
the exported function name to invoke; for Python modules it is the import path plus callable name; for
builtins it is the built-in dispatcher key. The value `"workflow"` is not special. A single
content-addressed WASM module can export many workflow ops, each with a different `defop.impl.entrypoint`.

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
      "hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
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
    "receipt": "acme/SlackPostReceipt@1",
    "origin_scope": ["workflow", "system"],
    "execution_class": "external_async"
  },
  "impl": {
    "module": "acme/order_bundle@1",
    "entrypoint": "orders.effects:post_to_slack"
  }
}
```

### Builtin Timer Effect

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_effects@1",
  "runtime": {
    "kind": "builtin",
    "builtin_id": "sys.effects.v1"
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
    "receipt": "sys/TimerSetReceipt@1",
    "origin_scope": ["workflow", "system", "governance"],
    "execution_class": "owner_local_async"
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
      "items": { "$ref": "#/$defs/SecretEntry" }
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
    "SecretEntry": {
      "oneOf": [
        { "$ref": "#/$defs/NamedRef" },
        { "$ref": "#/$defs/SecretDecl" }
      ]
    },
    "SecretDecl": {
      "type": "object",
      "properties": {
        "alias": {
          "type": "string",
          "pattern": "^[a-z][a-z0-9_.-]*(/[A-Za-z][A-Za-z0-9_.-]*)*$"
        },
        "version": {
          "type": "integer",
          "minimum": 1
        },
        "binding_id": {
          "type": "string",
          "minLength": 1
        },
        "expected_digest": { "$ref": "common.schema.json#/$defs/Hash" }
      },
      "required": ["alias", "version", "binding_id"],
      "additionalProperties": false
    },
    "Routing": {
      "type": "object",
      "properties": {
        "subscriptions": {
          "type": "array",
          "items": { "$ref": "#/$defs/RoutingSubscription" }
        },
        "inboxes": {
          "type": "array",
          "items": { "$ref": "#/$defs/InboxRoute" }
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
    },
    "InboxRoute": {
      "type": "object",
      "properties": {
        "source": {
          "type": "string",
          "minLength": 1
        },
        "op": {
          "$ref": "common.schema.json#/$defs/Name",
          "description": "Workflow op that will receive messages from this inbox."
        }
      },
      "required": ["source", "op"],
      "additionalProperties": false
    }
  }
}
```

Removed from `Manifest`:

```text
effects
effect_bindings
```

Removed from routing:

```text
RoutingSubscription.module
InboxRoute.workflow
```

## `defeffect.schema.json`

Target action:

```text
delete spec/schemas/defeffect.schema.json
```

There is no AIR v2 compatibility schema for `defeffect`.

## `patch.schema.json`

Patch documents should accept `defop` through `common.schema.json#/$defs/DefKind`. The schema does not need op-specific patch operations.

Target field-level changes:

```json
{
  "$id": "https://aos.dev/air/v2/patch.schema.json",
  "title": "AIR v2 Manifest Patch",
  "$defs": {
    "node_json": {
      "description": "Authoring form of any AIR node: defschema, defmodule, defop, defsecret.",
      "type": "object",
      "minProperties": 1
    }
  }
}
```

Patch operation semantics:

```text
add_def / replace_def / remove_def accept kind = defop.
set_manifest_refs accepts kind = defop and updates manifest.ops.
set_routing_events uses RoutingSubscription.op.
set_routing_inboxes uses InboxRoute.op.
```

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
4. Every op implementation references an active module.
5. Every workflow op schema ref exists.
6. Every effect op params and receipt schema ref exists.
7. Every routing subscription references an active workflow op.
8. Every inbox route references an active workflow op.
9. Every workflow `effects_emitted[]` entry references an active effect op.
10. Workflow key-field validation uses `workflow.key_schema`.
11. Effect semantic kind duplicates are allowed only if the runtime has a deterministic dispatch rule by op identity. Recommendation: allow duplicates because op identity is canonical.
12. The referenced module runtime kind must support the op kind.
13. Effect execution class must be compatible with module runtime kind and op kind.
14. Hashed artifacts must decode as the artifact shape implied by `runtime.kind`.
15. Python bundle metadata must satisfy the declared `runtime.python` version and provide a compatible target for the runner host.

## Open Schema Decisions

- Whether `manifest.routing` should be required. Current target keeps it optional.
- Whether inline `SecretDecl` remains in manifest v2. Current target keeps the current ability.

## Done When

- This document is accepted as the target for `spec/schemas/`.
- P1 can implement these schemas without inventing additional public fields.
