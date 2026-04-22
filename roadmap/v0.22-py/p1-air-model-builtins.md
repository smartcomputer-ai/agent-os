# P1: AIR v2 Schema And Catalog Cut

Status: planned.

## Goal

Implement the canonical AIR v2 public model from `p0-target-shape.md`.

This phase should make the loader, schema set, Rust AIR model, built-in catalog, and patch document
surface speak AIR v2 directly. It is an aggressive replacement phase, not a compatibility shim over
AIR v1.

## Non-Goals

- Do not implement Python workflow or Python effect execution.
- Do not keep an AIR v1 loader, migration path, or compatibility schema set.
- Do not add public capability/policy authority.
- Do not add `defsecret.allowed_ops`; v0.22 relies on effect-op admission, params schemas, and
  node-local/runtime policy.

## Work

- Treat `roadmap/v0.22-py/p0-target-shape.md` as the source of truth for JSON Schema shape.
- Replace the public schema set with:
  - `common.schema.json`
  - `defschema.schema.json`
  - `defmodule.schema.json`
  - `defop.schema.json`
  - `defsecret.schema.json`
  - `manifest.schema.json`
  - `patch.schema.json`
- Remove the public schema entry for `defeffect`.
- Split common kind enums:
  - `RootKind`: `defschema`, `defmodule`, `defop`, `defsecret`, `manifest`
  - `DefKind`: `defschema`, `defmodule`, `defop`, `defsecret`
- Remove public `EffectKind`. Effect identity is the versioned `defop.name` plus the canonical
  definition hash.
- Add AIR model types for:
  - `DefModule` as runtime/artifact metadata only
  - `DefOp` with `op_kind = workflow | effect`
  - `WorkflowOp`
  - `EffectOp`
  - `OpImpl`
  - `DefSecret`
  - AIR v2 `Manifest`
  - AIR v2 patch documents
- Remove public model fields and forms eliminated by P0:
  - `DefEffect`
  - `EffectBinding`
  - `module_kind`
  - module-level workflow ABI fields
  - `manifest.effects`
  - `manifest.effect_bindings`
  - `routing.inboxes`
  - `routing.subscriptions[].module`
  - public cap/policy fields
- Redefine `DefModule.runtime` and artifact validation:
  - `wasm` accepts only `wasm_module`
  - `python` accepts `python_bundle` or `workspace_root`
  - `builtin` has no artifact
  - `workspace_root.path` is optional and `workspace` is not part of the artifact identity
- Make canonical workflow ops require `workflow.effects_emitted`, including empty arrays.
- Keep `workflow.event` as a single schema ref. Do not add `workflow.events[]`.
- Convert built-in definitions to module-plus-op definitions:
  - built-in effects become `defop` effect entries
  - built-in workflows become workflow `defop` entries backed by built-in or WASM modules as
    appropriate
  - secrets remain `defsecret` refs through `manifest.secrets`
- Update schema embedding and catalog registration.
- Update authoring manifest loading to collect `defop` and `defsecret` nodes.
- Update governance patch schemas and canonicalization:
  - patch `version = "2"`
  - `add_def`, `replace_def`, and `remove_def` use `DefKind`
  - `set_manifest_refs` updates `schemas`, `modules`, `ops`, and `secrets`
  - `set_routing_subscriptions` replaces `set_routing_events`
  - `set_routing_inboxes` and `set_secrets` are removed

## Semantic Validation

Implement the static validation rules that can run at load time without executing workflows:

- Every manifest schema ref resolves to a `defschema` or built-in schema.
- Every manifest module ref resolves to a `defmodule`.
- Every manifest op ref resolves to a `defop`.
- Every manifest secret ref resolves to a `defsecret`.
- Every op implementation references an active module.
- Workflow ops have `workflow` and no `effect`.
- Effect ops have `effect` and no `workflow`.
- Workflow schema refs exist.
- Effect `params` and `receipt` schema refs exist.
- Workflow `effects_emitted[]` entries reference active effect ops.
- Routing subscriptions reference active workflow ops.
- Routing subscription event schemas are deliverable to the target workflow event schema:
  exact match, or a named ref arm of a variant workflow event schema.
- Routable workflow event variants use named schema refs and do not contain duplicate refs.
- Key-field validation uses the target workflow op's `workflow.key_schema`.
- The referenced module runtime kind supports the op kind.
- Secret refs in effect params are admitted by the effect op params schema and resolve through
  active `defsecret` declarations.

## Main Touch Points

- `spec/schemas/*.schema.json`
- `spec/defs/*.air.json`
- `crates/aos-air-types/src/model.rs`
- `crates/aos-air-types/src/validate.rs`
- `crates/aos-air-types/src/catalog.rs`
- `crates/aos-air-types/src/builtins.rs`
- `crates/aos-air-types/src/schemas.rs`
- `crates/aos-authoring/src/manifest_loader.rs`
- `crates/aos-authoring/src/build.rs`
- `crates/aos-kernel/src/patch_doc.rs`
- `crates/aos-kernel/src/governance*.rs`

## Done When

- AIR v2 nodes parse, validate, canonicalize, hash, and serialize without inventing fields outside P0.
- Built-in AIR loads as `defschema` + `defmodule` + `defop` + `defsecret` + `manifest`.
- Manifest validation no longer relies on `DefEffect` or `EffectBinding`.
- Patch documents can add/replace/remove `defop` and update `manifest.ops`.
- AIR v1 manifests and patch documents are rejected instead of translated.
- `cargo test -p aos-air-types` and authoring loader tests pass, or failures are limited to runtime
  code that P2 intentionally has not migrated yet.
