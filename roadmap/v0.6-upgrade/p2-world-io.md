# P2: World IO (import/export foundation)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (filesystem paths will drift and become incompatible)  
**Status**: Complete

## Status snapshot (current codebase)
- World IO module exists in `crates/aos-host/src/world_io.rs` and is re-exported by `aos-host`.
- `aos init` seeds the store via World IO and writes canonical `.aos/manifest.air.cbor`.
- `aos gov propose --patch-dir` uses World IO to build PatchDocuments and resolve base manifest hashes.
- `manifest_loader::load_from_assets_with_defs` preserves `defsecret` nodes for World IO.
- `export_bundle` loads manifest + defs from CAS and can include built-in `sys/*` defs.
- Base manifest resolution prefers control `manifest-get`, then CAS, then `.aos/manifest.air.cbor`.

## Goal
Create a single World IO path that canonicalizes AIR bundles, derives patch docs, and can export a deterministic filesystem view. Use it for both `init` (genesis) and `gov propose` (patch) so the rules are shared.

## Principles
1. **One import path**: all filesystem submissions go through the same canonicalization rules.
2. **World as source of truth**: import produces canonical CBOR + hashes that match what the kernel runs.
3. **AIR-only is a first-class filter**: patch submissions should be able to ignore modules.
4. **Deterministic outputs**: export must be stable across machines for the same manifest hash.

## Authoring surfaces vs canonical artifacts
- **AIR**: authored as JSON in `air/`, canonicalized to CBOR nodes in CAS.
- **WASM**: compiled externally; stored as blobs and referenced by `defmodule.wasm_hash`.
- **Source code**: synced via workspaces (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).
- **Sys defs**: never authored; optionally exported as a read-only reference file
  (e.g., `air/sys.air.json`) when requested.

## Proposed World IO layer
Add a shared module (e.g., `crates/aos-host/src/world_io.rs`) and re-export from `aos-host`.

### Core types
- `WorldBundle`:
  - `manifest` + `defs` (schemas/modules/plans/caps/policies/effects/secrets)
  - `wasm_blobs` (optional, hash -> bytes)
- `ImportMode`:
  - `Genesis`: no base manifest; initialize a new world.
  - `Patch`: requires base manifest hash; emits PatchDocument.
- `BundleFilter`:
  - `AirOnly`: ignore modules; use only AIR JSON.
  - `Full`: include modules.

### Core functions (sketch)
- `load_air_bundle(dir, filter) -> WorldBundle`
  - Uses `manifest_loader::load_from_assets`.
  - Enforces `sys/*` rejection and placeholder normalization.
- `export_bundle(store, manifest_hash, options) -> WorldBundle`
  - Reads manifest + referenced defs from CAS.
  - Pulls wasm blobs for non-sys modules when requested.
  - Optionally includes built-in `sys/*` defs for reference-only export.
- `import_bundle(store, bundle, mode) -> ImportPlan`
  - `Genesis`: store nodes + manifest, emit canonical manifest hash.
  - `Patch`: build PatchDocument with pre-hashes and manifest ref updates.
- `write_air_layout(bundle, out_dir)`
  - Materialize stable `air/*.air.json` files.
  - Write `.aos/manifest.air.cbor` from canonical bytes.
- `resolve_base_manifest_hash(dirs, control) -> hash`
  - Prefer control `manifest-get`, fall back to store; `.aos/manifest.air.cbor` only as last resort.

## Unifying init + governance patching

### `aos init`
Replace ad-hoc file writing with World IO:
1. Generate a minimal template bundle (or load a named template).
2. `import_bundle(..., Genesis)` to seed the store and write canonical manifest bytes.
3. `write_air_layout(...)` to populate `air/` for editing.

### `aos gov propose`
Route through World IO:
1. `load_air_bundle(air_dir, AirOnly)`.
2. `resolve_base_manifest_hash(...)`.
3. `import_bundle(..., Patch)` to build a PatchDocument.
4. Submit via governance effects.

### CLI shape change
Deprecate `gov propose --patch-dir` in favor of a single `aos import` path:
- `aos import --air <dir> --mode patch --air-only [--propose]`
- Keep `--patch-dir` as a thin wrapper for one release, then remove.

## Work plan
1) **World IO module**  
   - [x] Implement `WorldBundle`, `ImportMode`, `BundleFilter`.
   - [x] Move patch doc construction out of CLI into World IO.

2) **Base manifest resolver**  
   - [x] Prefer control `manifest-get`; fallback to store read.
   - [x] Remove hard dependency on `.aos/manifest.air.cbor`.

3) **Refactor `aos init`**  
   - [x] Generate template bundle and import via `Genesis`.
   - [x] Write canonical `.aos/manifest.air.cbor`.

4) **Refactor `aos gov propose`**  
   - [x] Replace `--patch-dir` logic with World IO.
   - [ ] Add `aos import` (or equivalent) as the unified entry point. (Moved to P3)

5) **Tests**  
   - [x] Import/export round-trip yields identical manifest hash.
   - [x] Patch doc generated from `air/` matches existing semantics.
   - [x] Genesis import seeds store with manifest node and defs.

## Open questions
- Where should World IO live (aos-host vs aos-store)?
- Should `aos init` always seed the store, or keep a file-only mode?
- What is the exact on-disk AIR layout for export (multi-file JSON vs single bundle)?
