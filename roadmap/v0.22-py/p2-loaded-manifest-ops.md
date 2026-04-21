# P2: Loaded Manifest Uses Ops

Status: planned.

## Goal

Make loaded runtime state op-centered while keeping execution behavior as close as possible to current behavior.

After this phase, the kernel should no longer ask for workflow/effect metadata from `DefModule` or `DefEffect`. It should ask the active op indexes.

## Work

- Replace `LoadedManifest.effects` with `LoadedManifest.ops`.
- Replace `LoadedManifest.effect_catalog` construction from `DefEffect` with construction from `DefOp` effect entries.
- Add indexes:
  - `workflow_ops`
  - `pure_ops`
  - `effect_ops`
  - semantic effect kind to op refs, if needed for diagnostics only
- Update manifest catalog loading to load `manifest.ops`.
- Update authoring manifest loading to collect `AirNode::Defop`.
- Update governance patch canonicalization and summaries to include `defop`.
- Update patch document application so `set_manifest_refs` supports `defop`.
- Update query/list-defs surfaces to return `defop`.
- Remove compatibility fallback that auto-includes built-in effects when `manifest.effects` is empty.

## Main Touch Points

- `crates/aos-kernel/src/manifest.rs`
- `crates/aos-kernel/src/manifest_catalog.rs`
- `crates/aos-kernel/src/governance.rs`
- `crates/aos-kernel/src/governance_effects.rs`
- `crates/aos-kernel/src/governance_utils.rs`
- `crates/aos-kernel/src/patch_doc.rs`
- `crates/aos-authoring/src/manifest_loader.rs`
- `crates/aos-authoring/src/build.rs`
- `crates/aos-kernel/src/world/mod.rs`
- `crates/aos-kernel/src/world/query_api.rs`

## Validation Rules

- Every `manifest.ops[]` ref must resolve to a `defop`.
- Every op implementation module must resolve to an active `defmodule`.
- Workflow ops must have `workflow` and must not have `pure` or `effect`.
- Pure ops must have `pure` and must not have `workflow` or `effect`.
- Effect ops must have `effect` and must not have `workflow` or `pure`.
- Schema refs inside ops must exist in active or built-in schemas.
- `routing.subscriptions[].op` must reference an active workflow op.
- Workflow `effects_emitted[]` must reference active effect ops.

## Done When

- `LoadedManifest` carries ops as the authoritative operation catalog.
- Manifest validation errors mention ops, not modules/effects, for callable interface failures.
- Governance patch summaries include `defop` adds/replaces/removes.
- `cargo test -p aos-kernel manifest` and `cargo test -p aos-authoring` pass or have only expected follow-on failures from runtime migration.

