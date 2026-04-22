# P1: AIR v2 Schema And Catalog Cut

Status: in progress. Core AIR v2 model/schema/catalog, kernel loader/governance plumbing, and
authoring library plumbing are complete. Remaining work is mostly v1 fixture/import migration.
Secret-reference admission through effect-op params schemas is deferred to P4 cleanup.

## Goal

Implement the canonical AIR v2 public model from `p0-target-shape.md`.

This phase should make the loader, schema set, Rust AIR model, built-in catalog, and patch document
surface speak AIR v2 directly. It is an aggressive replacement phase, not a compatibility shim over
AIR v1.

Note: since this is a bigger refactor, it is acceptable to have modules breaking while we do p1-p3. It is fine to just refacor a few crates like crates/aos-air-types, etc and only then move to next.

## Non-Goals

- Do not implement Python workflow or Python effect execution.
- Do not keep an AIR v1 loader, migration path, or compatibility schema set.
- Do not add public capability/policy authority.
- Do not add `defsecret.allowed_ops`; v0.22 relies on effect-op admission, params schemas, and
  node-local/runtime policy.

## Work

- [x] Treat `roadmap/v0.22-py/p0-target-shape.md` as the source of truth for JSON Schema shape.
- [x] Replace the public schema set with:
  - `common.schema.json`
  - `defschema.schema.json`
  - `defmodule.schema.json`
  - `defop.schema.json`
  - `defsecret.schema.json`
  - `manifest.schema.json`
  - `patch.schema.json`
- [x] Remove the public schema entry for `defeffect`.
- [x] Split common kind enums:
  - `RootKind`: `defschema`, `defmodule`, `defop`, `defsecret`, `manifest`
  - `DefKind`: `defschema`, `defmodule`, `defop`, `defsecret`
- [x] Remove public `EffectKind`. Effect identity is the versioned `defop.name` plus the canonical
  definition hash.
- [x] Add AIR model types for:
  - `DefModule` as runtime/artifact metadata only
  - `DefOp` with `op_kind = workflow | effect`
  - `WorkflowOp`
  - `EffectOp`
  - `OpImpl`
  - `DefSecret`
  - AIR v2 `Manifest`
  - AIR v2 patch documents
- [x] Remove public model fields and forms eliminated by P0:
  - `DefEffect`
  - `EffectBinding`
  - `module_kind`
  - module-level workflow ABI fields
  - `manifest.effects`
  - `manifest.effect_bindings`
  - `routing.inboxes`
  - `routing.subscriptions[].module`
  - public cap/policy fields
- [x] Redefine `DefModule.runtime` and artifact validation:
  - `wasm` accepts only `wasm_module`
  - `python` accepts `python_bundle` or `workspace_root`
  - `builtin` has no artifact
  - `workspace_root.path` is optional and `workspace` is not part of the artifact identity
- [x] Make canonical workflow ops require `workflow.effects_emitted`, including empty arrays.
- [x] Keep `workflow.event` as a single schema ref. Do not add `workflow.events[]`.
- [x] Convert built-in definitions to module-plus-op definitions:
  - built-in effects become `defop` effect entries
  - built-in workflows become workflow `defop` entries backed by built-in or WASM modules as
    appropriate
  - secrets remain `defsecret` refs through `manifest.secrets`
- [x] Update schema embedding and catalog registration.
- [x] Update authoring manifest loading to collect `defop` and `defsecret` nodes.
- [x] Update governance patch schemas and canonicalization:
  - patch `version = "2"`
  - `add_def`, `replace_def`, and `remove_def` use `DefKind`
  - `set_manifest_refs` updates `schemas`, `modules`, `ops`, and `secrets`
  - `set_routing_subscriptions` replaces `set_routing_events`
  - `set_routing_inboxes` and `set_secrets` are removed

## Semantic Validation

Implement the static validation rules that can run at load time without executing workflows:

- [x] Every manifest schema ref resolves to a `defschema` or built-in schema.
- [x] Every manifest module ref resolves to a `defmodule`.
- [x] Every manifest op ref resolves to a `defop`.
- [x] Every manifest secret ref resolves to a `defsecret`.
- [x] Every op implementation references an active module.
- [x] Workflow ops have `workflow` and no `effect`.
- [x] Effect ops have `effect` and no `workflow`.
- [x] Workflow schema refs exist.
- [x] Effect `params` and `receipt` schema refs exist.
- [x] Workflow `effects_emitted[]` entries reference active effect ops.
- [x] Routing subscriptions reference active workflow ops.
- [x] Routing subscription event schemas are deliverable to the target workflow event schema:
  exact match, or a named ref arm of a variant workflow event schema.
- [x] Routable workflow event variants use named schema refs and do not contain duplicate refs.
- [x] Key-field validation uses the target workflow op's `workflow.key_schema`.
- [x] The referenced module runtime kind supports the op kind.
- [ ] Secret refs in effect params are admitted by the effect op params schema and resolve through
  active `defsecret` declarations. Deferred to `p4-cleanup.md`.

## Remaining

- [ ] Migrate v1 AIR fixtures/import sources used by `aos-authoring` tests, including
  `crates/aos-smoke/fixtures/01-hello-timer`, `crates/aos-agent/air`, and sync-test fixture
  manifests. Current failures are parse errors from old `manifest.effects`/`routing.module` shape.
- [ ] Decide whether `aos-authoring` tests should keep fixture compatibility helpers or require
  all fixtures to be canonical AIR v2. P1 intent is canonical AIR v2 only, so fixture migration is
  preferred.
- [ ] Run the post-fixture-migration authoring verification target: `cargo test -p aos-authoring`.

## Main Touch Points

- [x] `spec/schemas/*.schema.json`
- [x] `spec/defs/*.air.json`
- [x] `crates/aos-air-types/src/model.rs`
- [x] `crates/aos-air-types/src/validate.rs`
- [x] `crates/aos-air-types/src/catalog.rs`
- [x] `crates/aos-air-types/src/builtins.rs`
- [x] `crates/aos-air-types/src/schemas.rs`
- [x] `crates/aos-authoring/src/manifest_loader.rs`
- [x] `crates/aos-authoring/src/build.rs`
- [x] `crates/aos-kernel/src/patch_doc.rs`
- [x] `crates/aos-kernel/src/governance*.rs`
- [x] `crates/aos-kernel/src/manifest*.rs`, `world/manifest_runtime.rs`, and related loader surfaces
- [x] Minimal `crates/aos-node` compile blockers needed by `aos-authoring`

## Done When

- [x] AIR v2 nodes parse, validate, canonicalize, hash, and serialize without inventing fields outside P0.
- [x] Built-in AIR loads as `defschema` + `defmodule` + `defop` + `defsecret` + `manifest`.
- [x] Manifest validation no longer relies on `DefEffect` or `EffectBinding`.
- [x] Patch documents can add/replace/remove `defop` and update `manifest.ops`.
- [x] AIR v1 manifests and patch documents are rejected instead of translated.
- [ ] `cargo test -p aos-air-types` and authoring loader tests pass, or failures are limited to runtime
  code that P2 intentionally has not migrated yet.
