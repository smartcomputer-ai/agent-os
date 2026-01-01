# P2: World Checkout/Commit (short-term)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (world stays a deploy target, not the source of truth)

## Status snapshot (current codebase)
- `aos` CLI already supports `blob put/get`, `defs ls/get`, and `manifest get` (control or local).
- `aos gov propose --patch-dir` builds a PatchDocument from `air/` assets and patches routing/triggers/module_bindings/secrets.
- `manifest_loader::load_from_assets` reads `air/` JSON bundles and rejects `sys/*` defs, so `air/` is already an authoring surface.
- Reducer compilation is wired into `aos run` via `resolve_placeholder_modules`, and expects `modules/<name>-<hash>.wasm`.
- ObjectCatalog (`sys/ObjectCatalog@1`) is present with `aos obj` reads, but no CLI wrapper to register source bundles yet.
- Gap: no checkout/export command, no commit/import command, and `--patch-dir` currently relies on `.aos/manifest.air.cbor` which is not written by the daemon.

## Goal
Ship a short-term checkout/commit loop where the world is the source of truth and the filesystem is a view for editing/building. Keep the door open for later in-world build effects.

## Short-term workflow

### Checkout (export world -> filesystem view)
1. Fetch canonical manifest bytes from the world (control `manifest-get` when available; else store).
2. Fetch defs referenced by the manifest (skip `sys/*` defs).
3. Materialize AIR JSON assets under `air/` in a stable layout:
   - `air/manifest.air.json`
   - `air/schemas.air.json`
   - `air/module.air.json`
   - `air/plans.air.json`
   - `air/capabilities.air.json`
   - `air/policies.air.json`
   - `air/effects.air.json`
   - `air/secrets.air.json`
4. Materialize wasm blobs as `modules/<name>-<hash>.wasm` for each non-sys `defmodule`.
5. Write `.aos/manifest.air.cbor` with canonical manifest bytes to make patching reproducible.
6. Optional: fetch latest source bundles from ObjectCatalog (kind `source.bundle`) into `sources/`.

### Commit (filesystem view -> governed world update)
1. Build wasm externally (reuse `aos-wasm-build` via a new `aos build` or `aos commit --build`).
2. Persist wasm to `modules/<name>-<hash>.wasm` and `aos blob put` it into the store.
3. Update `defmodule.wasm_hash` in `air/` to the new hash (or have commit patch it).
4. Generate PatchDocument from `air/` (`aos gov propose --patch-dir` already does this).
5. Submit through governance (`gov-propose` -> `gov-shadow` -> `gov-approve` -> `gov-apply`).
6. Register the source bundle in ObjectCatalog by emitting `sys/ObjectRegistered@1` with `meta`:
   - `name`: bundle name/path (key must match)
   - `kind`: `source.bundle`
   - `hash`: blob hash from `blob-put`
   - `tags`: include `manifest_hash`, `wasm_hash`, toolchain id, git sha (if available)
   - `created_at`, `owner`

## Proposed work (short-term)
1) **Add checkout/export command**  
   - CLI: `aos checkout [--out <dir>] [--with-sources]`.  
   - Writes `air/`, `modules/`, and `.aos/manifest.air.cbor`.  
   - Uses control when daemon is running; falls back to local store.  

2) **Add commit/import wrapper**  
   - CLI: `aos commit [--build] [--source <tar>] [--dry-run]`.  
   - Wraps build + blob put + patch doc + governance submit.  
   - `--dry-run` prints PatchDocument JSON for review.  

3) **Fix base-manifest hash source**  
   - Teach `--patch-dir` (or commit wrapper) to use `manifest-get` / manifest hash from the store instead of relying on `.aos/manifest.air.cbor`.  

4) **Object catalog registration helper**  
   - Add `aos obj register` (thin wrapper around `aos event send sys/ObjectRegistered@1`).  
   - Ensures key equals `meta.name` for keyed routing.  

## Groundwork for later in-world iteration
- Store source bundles in ObjectCatalog now so worlds remain portable and self-describing.
- Add a plan-only build effect later (e.g., `build.rust_wasm`) that takes `source_ref` and returns `wasm_ref`; the checkout/commit flow can swap to it without changing the object model.

## Open questions
- Should checkout export one `air/defs.air.json` array or use the multi-file layout used in examples?
- Where should source bundles land on disk (`sources/`, `bundle/`, or `source.tgz`)?
- Should commit default to governance only, or allow a "local apply" mode for developer loops?
