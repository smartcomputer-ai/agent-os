# P2: World IO (import/export foundation)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (filesystem paths will drift and become incompatible)

## Status snapshot (current codebase)
- `aos init` writes a minimal `air/manifest.air.json` and directories, but does not seed the store.
- `aos gov propose --patch-dir` builds a PatchDocument in CLI code (custom logic in `crates/aos-cli/src/commands/gov.rs`).
- `manifest_loader::load_from_assets` is the canonical AIR reader and already rejects `sys/*` defs.
- Kernel persists manifest + defs into the store on world load; control `manifest-get` returns canonical CBOR bytes.
- The base manifest hash for patching is currently derived from `.aos/manifest.air.cbor`, which is not guaranteed to exist.
- No shared import/export library; each path manipulates files differently.

## Goal
Create a single World IO path that canonicalizes AIR bundles, derives patch docs, and can export a deterministic filesystem view. Use it for both `init` (genesis) and `gov propose` (patch) so the rules are shared.

## Principles
1. **One import path**: all filesystem submissions go through the same canonicalization rules.
2. **World as source of truth**: import produces canonical CBOR + hashes that match what the kernel runs.
3. **AIR-only is a first-class filter**: patch submissions should be able to ignore modules/sources.
4. **Deterministic outputs**: export must be stable across machines for the same manifest hash.

## Authoring surfaces vs canonical artifacts
- **AIR**: authored as JSON in `air/`, canonicalized to CBOR nodes in CAS.
- **WASM**: compiled externally; stored as blobs and referenced by `defmodule.wasm_hash`.
- **Source code**: stored as a single deterministic tarball blob (the "source bundle"),
  registered in ObjectCatalog with metadata; exported as a directory for local dev.
- **Sys defs**: never authored; optionally exported as a read-only reference file
  (e.g., `air/sys.air.json`) when requested.

## Proposed World IO layer
Add a shared module (e.g., `crates/aos-host/src/world_io.rs`) and re-export from `aos-host`.

### Core types
- `WorldBundle`:
  - `manifest` + `defs` (schemas/modules/plans/caps/policies/effects/secrets)
  - `wasm_blobs` (optional, hash -> bytes)
  - `source_bundle` (optional, hash -> bytes + metadata)
- `ImportMode`:
  - `Genesis`: no base manifest; initialize a new world.
  - `Patch`: requires base manifest hash; emits PatchDocument.
- `BundleFilter`:
  - `AirOnly`: ignore modules/sources; use only AIR JSON.
  - `Full`: include modules and sources.

### Core functions (sketch)
- `load_air_bundle(dir, filter) -> WorldBundle`
  - Uses `manifest_loader::load_from_assets`.
  - Enforces `sys/*` rejection and placeholder normalization.
- `export_bundle(store, manifest_hash, options) -> WorldBundle`
  - Reads manifest + referenced defs from CAS.
  - Pulls wasm blobs for non-sys modules when requested.
  - Optionally attaches the latest source bundle from ObjectCatalog.
  - Optionally includes built-in `sys/*` defs for reference-only export.
- `import_bundle(store, bundle, mode) -> ImportPlan`
  - `Genesis`: store nodes + manifest, emit canonical manifest hash.
  - `Patch`: build PatchDocument with pre-hashes and manifest ref updates.
- `write_air_layout(bundle, out_dir)`
  - Materialize stable `air/*.air.json` files.
  - Write `.aos/manifest.air.cbor` from canonical bytes.
  - If a source bundle is present, unpack into `sources/` for local dev.
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
   - Implement `WorldBundle`, `ImportMode`, `BundleFilter`.
   - Move patch doc construction out of CLI into World IO.

2) **Base manifest resolver**  
   - Prefer control `manifest-get`; fallback to store read.
   - Remove hard dependency on `.aos/manifest.air.cbor`.

3) **Refactor `aos init`**  
   - Generate template bundle and import via `Genesis`.
   - Write canonical `.aos/manifest.air.cbor`.

4) **Refactor `aos gov propose`**  
   - Replace `--patch-dir` logic with World IO.
   - Add `aos import` (or equivalent) as the unified entry point.

5) **Tests**  
   - Import/export round-trip yields identical manifest hash.
   - Patch doc generated from `air/` matches existing semantics.
   - Genesis import seeds store with manifest node and defs.

## Open questions
- Where should World IO live (aos-host vs aos-store)?
- Should `aos init` always seed the store, or keep a file-only mode?
- What is the exact on-disk AIR layout for export (multi-file JSON vs single bundle)?
