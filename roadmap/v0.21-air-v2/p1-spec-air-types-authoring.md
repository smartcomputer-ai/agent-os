# P1: Forked AIR Spec And Core Model Cut

Status: implemented for the narrow P1 targets.

## Goal

Implement the forked AIR v2 public surface in the spec files, built-in definition shelf, AIR model
crate, authoring loader, kernel control-plane loaders, and small support crates.

This phase intentionally focuses on the definition/catalog layer first:

```text
spec/schemas/
spec/defs/
crates/aos-air-types
crates/aos-authoring
crates/aos-kernel manifest/governance loader layer
small schema/catalog support crates
```

The kernel runtime cut is a second step, but the kernel manifest/governance layer must move in P1
because `aos-authoring` depends on `LoadedManifest`, `ManifestLoader`, `MemStore`, and patch helper
types from `aos-kernel`. Other crates may be temporarily broken while the core AIR surface is being
replaced.

## Direction

Replace the temporary canonical `defop` model with explicit public definitions:

```text
defworkflow
defeffect
```

The runtime identity lesson from `defop` remains valid: workflows and effects are named executable
definitions with canonical hashes and implementation entrypoints. The public model should no longer
encode that as one root form plus `op_kind`.

## Non-Goals

- Do not update kernel execution, world stepping, domain routing execution, open work, receipts,
  snapshots, or replay in this phase except for narrow compile shims if they are needed to keep the
  P1 targets testable.
- Do not migrate every fixture, smoke test, CLI surface, node endpoint, or agent path.
- Do not implement Python workflow or Python effect execution.
- Do not add public capability/policy authority.
- Do not keep an AIR v1 loader, migration path, or compatibility schema set.
- Do not preserve `defop` as a canonical public form.

## Compatibility And Breakage

This is an aggressive replacement phase. It is acceptable for downstream crates to fail to compile
while P1 lands, as long as the breakage is from known references to the old op-centered public
surface.

The first green targets are narrow:

```text
cargo test -p aos-air-types
cargo test -p aos-kernel --lib manifest
cargo test -p aos-authoring
```

If a support crate blocks these targets, update only the minimum necessary surface there. Kernel
world runtime, node, CLI, smoke, and agent convergence belongs in later phases.

## Ambient `sys/*` Definitions

All built-in `sys/*` definitions are ambiently available in every AIR v2 manifest.

That means a user manifest does not need to list built-in system definitions under:

```text
schemas[]
modules[]
workflows[]
effects[]
secrets[]
```

Examples:

- A workflow may list `sys/timer.set@1` in `effects_emitted[]` without also listing
  `sys/timer.set@1` in `manifest.effects[]`.
- A workflow may use `sys/WorkflowContext@1` as `context` without listing that schema in
  `manifest.schemas[]`.
- Built-in modules such as `sys/builtin_effects@1` do not need manifest module refs.

Validation should treat the active catalog as:

```text
ambient built-in sys definitions + explicit manifest refs
```

External manifests must not define or override `sys/*` definitions. If a manifest lists a `sys/*`
ref explicitly, it is redundant but allowed only when it matches the built-in definition identity
and hash rules. Tooling should prefer omitting redundant `sys/*` refs from authored manifests.

This rule is important for the fork because `manifest.workflows[]` and `manifest.effects[]` should
describe the world's application surface, not repeat the whole system catalog.

## Spec Surface Work

- [x] Update `spec/schemas/common.schema.json`:
  - `RootKind`: `defschema`, `defmodule`, `defworkflow`, `defeffect`, `defsecret`, `manifest`
  - `DefKind`: `defschema`, `defmodule`, `defworkflow`, `defeffect`, `defsecret`
  - add `$defs.Impl` with `module` and `entrypoint`
  - keep `EffectKind` absent
- [x] Replace `spec/schemas/defop.schema.json` with:
  - `spec/schemas/defworkflow.schema.json`
  - `spec/schemas/defeffect.schema.json`
- [x] Update `spec/schemas/manifest.schema.json`:
  - `ops[]` -> `workflows[]` and `effects[]`
  - `routing.subscriptions[].op` -> `routing.subscriptions[].workflow`
  - keep `secrets[]` as refs to `defsecret`
  - do not require manifests to list ambient `sys/*` definitions
- [x] Update `spec/schemas/patch.schema.json`:
  - patch version remains `"2"`
  - `DefKind` accepts workflow/effect definitions
  - `set_manifest_refs` updates `schemas`, `modules`, `workflows`, `effects`, or `secrets`
  - `set_routing_subscriptions` uses the new workflow-targeted routing subscription shape
- [x] Delete or retire public `defop` schema references from the active schema set.
- [x] Validate all active schema JSON with `jq empty`.

## Built-In Definition Shelf

- [x] Convert `spec/defs/` from built-in ops to built-in workflows/effects:
  - built-in workflows use `$kind = "defworkflow"`
  - built-in effects use `$kind = "defeffect"`
  - implementation refs use `impl.module` and `impl.entrypoint`
- [x] Split `builtin-ops.air.json` into clearer files:
  - `builtin-workflows.air.json`
  - `builtin-effects.air.json`
  - P1 kept the existing filename temporarily to reduce churn; the P2 cleanup split it.
- [x] Ensure no active built-in definition uses `defop` or `op_kind`.
- [x] Ensure built-in manifests or examples omit redundant `sys/*` refs where practical.
- [x] Keep external `sys/*` definitions rejected.
- [x] Add or update built-in catalog tests for ambient `sys/*` resolution.

## `crates/aos-air-types`

- [x] Replace public model types:
  - remove canonical public `DefOp`
  - add `DefWorkflow`
  - add `DefEffect`
  - add shared `Impl`
  - keep workflow determinism on `DefWorkflow`
- [x] Removed the temporary internal `DefOp` compatibility shim during the P2 runtime cut.
- [x] Replace `RootKind` and `DefKind` enums.
- [x] Replace manifest model:
  - `ops` -> `workflows` and `effects`
  - routing target `op` -> `workflow`
- [x] Replace catalog indexes:
  - workflow definitions by name/hash
  - effect definitions by name/hash
  - modules by name/hash
  - schemas by name/hash
  - secrets by name/hash
- [x] Make built-in `sys/*` definitions ambient in validation and loaded catalog construction.
- [x] Preserve semantic validation:
  - workflow schema refs resolve
  - effect params and receipt schema refs resolve
  - `effects_emitted[]` references active or ambient built-in effects
  - routing subscriptions reference active or ambient built-in workflows
  - keyed routing checks `DefWorkflow.key_schema`
  - `impl.module` resolves to active or ambient built-in modules
  - module runtime supports the definition kind
- [x] Update canonicalization/hash tests to cover `defworkflow`, `defeffect`, and ambient `sys/*`
  references.
- [x] Remove active `defop` round-trip tests or rewrite them as workflow/effect tests.

## `crates/aos-authoring`

- [x] Update manifest loading to collect `defworkflow` and `defeffect` nodes.
- [x] Update authored bundle/build outputs:
  - workflow exports produce `defworkflow`
  - effect exports produce `defeffect`
  - module artifacts still produce `defmodule`
- [x] Update manifest synthesis:
  - application definitions go into `manifest.workflows[]` and `manifest.effects[]`
  - redundant `sys/*` refs are omitted by default
  - `routing.subscriptions[].workflow` is emitted
- [x] Update patch/build helpers to use workflow/effect `DefKind` values.
- [x] Keep import rejection for AIR v1; old op-centered public forms are no longer in active schemas.
- [x] Reject old `defop` nodes and manifest `ops[]` during AIR v2 model/authoring deserialization.
- [x] Add focused authoring tests for:
  - workflow/effect fixture loading
  - generated manifest refs
  - ambient built-in effect references from `effects_emitted[]`
  - routing subscriptions targeting workflows

## `crates/aos-kernel` Control Plane

Update the kernel pieces that own manifest loading, patching, governance definition storage, and the
`LoadedManifest` type consumed by authoring. Do not chase world execution yet.

- [x] Update `LoadedManifest`:
  - `ops` -> `workflows` and `effects`
  - keep `schemas`, `modules`, and `secrets`
  - keep enough compatibility shims only if required to make P1 compile, and mark them temporary
- [x] Update manifest loading and storage:
  - collect `DefWorkflow` and `DefEffect`
  - reject external `sys/*` definitions
  - include ambient built-in `sys/*` definitions in loaded catalogs
  - do not require redundant `sys/*` manifest refs
- [x] Update manifest patch helpers:
  - `manifest_patch_from_loaded`
  - `store_loaded_manifest`
  - definition add/replace/remove dispatch
  - `set_manifest_refs` for workflows/effects
  - routing subscription patching with `workflow`
- [x] Update governance utility surfaces that summarize or normalize manifest patches:
  - definition kind strings
  - workflow/effect manifest ref sections
  - routing target labels
- [x] Update `manifest_catalog.rs` or equivalent catalog loaders:
  - load workflow refs from `manifest.workflows[]`
  - load effect refs from `manifest.effects[]`
  - resolve built-in ambient definitions without explicit refs
- [x] Keep world/runtime modules compiling only where that is cheap and local. Larger runtime
  semantic updates belong in P2.
- [x] Add focused kernel control-plane tests for:
  - loading forked manifests
  - patching workflow/effect refs
  - ambient `sys/*` resolution
  - explicit matching `sys/*` refs if allowed by the chosen hash rules
  - rejecting external `sys/*` definitions

## Support Crates

Update only support crates that directly block the P1 targets.

Likely touch points:

- `crates/aos-cbor` only if schema embedding or canonicalization wrappers require changes.
- `crates/aos-sys` only if generated support schemas or fixture helpers still emit `op` fields.
- `crates/aos-wasm-sdk` only if compile-time helper types are needed by authoring tests.

Do not chase every downstream compile error in this phase.

## Explicitly Deferred To P2

- Domain routing execution.
- Workflow instance state and keyed cell storage names.
- Effect intent/open-work/receipt/stream durable records.
- Snapshot and replay field names.
- Kernel world/runtime indexes and bootstrapping.
- Node worker/control summaries.
- CLI and query rendering.
- Smoke, agent, and full fixture convergence.

P2 should make the runtime internally distinguish workflows and effects wherever semantics differ,
while keeping small shared helpers for `Impl` resolution and module runtime compatibility.

## Done When

- [x] Active JSON schemas define `defworkflow` and `defeffect` and no active `defop` schema remains.
- [x] Active built-in defs use `defworkflow` and `defeffect`.
- [x] `aos-air-types` parses, validates, canonicalizes, hashes, and serializes the forked AIR surface.
- [x] `aos-authoring` can load and synthesize forked AIR manifests.
- [x] Kernel manifest/governance loaders can store, load, and patch forked manifests.
- [x] Ambient `sys/*` definitions work without redundant manifest refs.
- [x] Focused tests for `aos-air-types`, kernel control-plane loading, and `aos-authoring` pass, or
  remaining failures are documented as P2 runtime fallout.
