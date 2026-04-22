# P4: AIR v2 Workflow/Effect Definition Fork

Status: planned.

## Goal

Fork the AIR v2 public model from the temporary unified `defop` surface to explicit canonical
workflow and effect definitions:

```text
defworkflow
defeffect
```

The P1-P3 `defop` work was useful and intentional. It proved the runtime shape we want: stable
named executable identities, op-local implementation entrypoints, manifest-controlled activation,
workflow-origin effect allowlists, and durable receipt/open-work records bound to workflow/effect
definition hashes.

This phase keeps those lessons, but changes the public AIR language to expose the determinism
boundary directly.

## Direction

AIR should have separate canonical root forms for workflows and effects:

```text
defschema
defmodule
defworkflow
defeffect
defsecret
manifest
```

The manifest should likewise activate workflows and effects through distinct lists:

```text
manifest.workflows[]
manifest.effects[]
```

Routing should target workflow definitions explicitly:

```text
routing.subscriptions[].workflow
```

Effect emission should continue to name effect definitions explicitly:

```text
defworkflow.effects_emitted[] -> defeffect.name
```

This is a public schema and authoring fork, not a return to AIR v1 effect semantics.

## Why

`defop` unified an implementation concern: workflows and effects are both named executable
definitions with an implementation module, entrypoint, manifest ref, and definition hash.

That commonality is real, but it is not the most important semantic boundary in AIR. Workflows and
effects sit on opposite sides of the determinism boundary:

- A workflow is deterministic orchestration. It owns state, event admission, keyed cells, receipt
  continuations, and a declared effect allowlist.
- An effect is authorized nondeterminism. It owns params, executor dispatch, external or internal
  observation, and terminal receipt payload validation.

Making both canonical documents look like a generic `defop` forces users, tools, specs, and audits
to first understand a meta-category and then inspect `op_kind`. Splitting the public forms makes the
AIR language say what the system means.

The unified model was still productive. It flushed out the right durable identities and removed the
old weak identity model based on workflow modules and semantic effect kind strings. This fork should
preserve that runtime correctness while improving the public language.

## Target Shapes

Canonical workflow definition:

```json
{
  "$kind": "defworkflow",
  "name": "acme/order.step@1",
  "state": "acme/OrderState@1",
  "event": "acme/OrderEvent@1",
  "context": "sys/WorkflowContext@1",
  "annotations": "acme/OrderAnnotations@1",
  "key_schema": "acme/OrderId@1",
  "effects_emitted": ["acme/slack.post@1"],
  "determinism": "strict",
  "impl": {
    "module": "acme/orders@1",
    "entrypoint": "orders.workflow:step"
  }
}
```

Canonical effect definition:

```json
{
  "$kind": "defeffect",
  "name": "acme/slack.post@1",
  "params": "acme/SlackPostParams@1",
  "receipt": "acme/SlackPostReceipt@1",
  "impl": {
    "module": "acme/orders@1",
    "entrypoint": "orders.effects:post_to_slack"
  }
}
```

Canonical manifest shape:

```json
{
  "$kind": "manifest",
  "air_version": "2",
  "schemas": [{ "name": "acme/OrderEvent@1" }],
  "modules": [{ "name": "acme/orders@1" }],
  "workflows": [{ "name": "acme/order.step@1" }],
  "effects": [{ "name": "acme/slack.post@1" }],
  "secrets": [],
  "routing": {
    "subscriptions": [
      {
        "event": "acme/OrderSubmitted@1",
        "workflow": "acme/order.step@1",
        "key_field": "order_id"
      }
    ]
  }
}
```

## Preserved Runtime Invariants

- Workflow identity is the workflow definition name and canonical definition hash.
- Effect identity is the effect definition name and canonical definition hash.
- A single module may implement many workflows and many effects.
- Replacing one workflow or effect does not imply replacing every definition in the same module.
- Workflows emit effect names, not semantic effect-kind strings.
- Workflow effect admission checks `effects_emitted[]` against active effect definitions.
- Effect params and receipts are schema-bound through the active effect definition.
- Open work, receipts, streams, snapshots, replay records, and audit traces pin workflow/effect
  definition identity and hashes.
- Receipt continuation routing remains independent of domain routing subscriptions.
- Opened async effects are still published only after the containing journal frame is durably
  flushed.

## Not A Regression To AIR v1

Do not bring back:

```text
effect_kind
manifest.effect_bindings
module_bindings
op_bindings
routing.module
routing.inboxes
defcap
defpolicy
cap_type
origin_scope
```

`defeffect` in this fork is not the old v1 effect catalog. It is the typed executable effect
definition that `defop(op_kind = "effect")` temporarily modeled.

## Implementation Model

The implementation may still keep an internal shared abstraction for common executable-definition
machinery:

```text
ExecutableDef
ImplRef
DefinitionHash
RuntimeSupport
```

That abstraction should stay below the public AIR boundary. Public JSON schemas, Rust AIR model
types, patch operations, manifest summaries, CLI output, and specs should speak in terms of
workflows and effects.

## Work

- [ ] Replace public root-kind enums:
  - `defop` -> `defworkflow`, `defeffect`
  - `DefKind` adds `defworkflow` and `defeffect`
- [ ] Replace `spec/schemas/defop.schema.json` with:
  - `defworkflow.schema.json`
  - `defeffect.schema.json`
- [ ] Update the Rust AIR model:
  - remove canonical `DefOp`
  - add `DefWorkflow`
  - add `DefEffect`
  - keep shared `OpImpl` or rename it to `ImplRef`
- [ ] Update manifests:
  - `manifest.ops[]` -> `manifest.workflows[]` and `manifest.effects[]`
  - routing target `op` -> `workflow`
- [ ] Update semantic validation:
  - active workflow refs resolve to `defworkflow`
  - active effect refs resolve to `defeffect`
  - workflow `effects_emitted[]` refs resolve to active effects
  - implementation modules resolve and support the workflow/effect runtime class
  - keyed routing validates against the target workflow's `key_schema`
- [ ] Update built-in definitions and catalogs:
  - built-in workflows become `defworkflow`
  - built-in effects become `defeffect`
- [ ] Update patch documents and governance summaries:
  - add/replace/remove workflow defs
  - add/replace/remove effect defs
  - set manifest workflow/effect refs separately
  - route summaries name workflow targets
- [ ] Update kernel/node runtime indexes while preserving existing durable identity semantics:
  - workflow name/hash index
  - effect name/hash index
  - shared implementation module/runtime lookup
- [ ] Update SDK envelopes, receipt/open-work structs, snapshots, and traces only where naming
  currently says `op` but the field semantically means workflow or effect.
- [ ] Update CLI/query output:
  - list/show `defworkflow`
  - list/show `defeffect`
  - report workflow/effect counts without generic op counts
- [ ] Update specs:
  - `spec/03-air.md`
  - `spec/04-workflows.md`
  - `spec/05-effects.md`
  - schema and built-in reference shelves
- [ ] Convert fixtures and examples from `defop` to `defworkflow`/`defeffect`.
- [ ] Sweep active implementation and fixture paths for stale public `defop` terminology once the
  fork lands.

## Done When

- AIR v2 public schemas no longer expose `defop` or `op_kind`.
- Active manifests use `workflows[]`, `effects[]`, and `routing.subscriptions[].workflow`.
- Workflows and effects remain definition-hash-bound in durable runtime records.
- No checked-in active fixture relies on `defop`.
- The repository remains clean for old AIR v1 effect-binding and effect-kind concepts.
- Replay from new forked AIR v2 journals remains byte-identical.
