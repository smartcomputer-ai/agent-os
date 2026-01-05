# P3: Import/Export CLI (checkout/commit on World IO)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (no unified workflow for editing + upgrades)
**Status**: Complete

## Status snapshot (current codebase)
- `aos export` uses World IO to materialize `air/` plus optional `modules/`.
- `aos export --defs-bundle` writes a single `air/defs.air.json` bundle.
- `aos import --air` supports genesis/patch modes and can drive governance steps.
- Source sync is handled by workspaces (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).
- `aos init` goes through the World IO genesis import path.
- `aos gov propose --patch-dir` is hidden and prints a deprecation notice.

## Dependency
Requires P2 World IO (`roadmap/v0.6-upgrade/p2-world-io.md`) to provide canonical import/export logic.

## Goal
Expose a single import/export CLI surface for checkout/commit workflows and remove ad-hoc filesystem submission paths.

## CLI surface (proposal)

### Export (world -> filesystem view)
```
aos export [--out <dir>] [--with-modules] [--with-sys] [--defs-bundle] [--manifest <hash>] [--air-only]
```
Behavior:
- Uses control `manifest-get` when daemon is running; falls back to store.
- Materializes a stable `air/` layout plus optional `modules/`.
- If `--with-sys` is set, export built-in `sys/*` defs into `air/sys.air.json` for reference.
- If `--defs-bundle` is set, write a single `air/defs.air.json` instead of per-kind files.
- Writes `.aos/manifest.air.cbor` with canonical bytes.

### Import (filesystem view -> world update)
```
aos import --air <dir> [--import-mode genesis|patch] [--air-only] [--dry-run]
```
Behavior:
- `--import-mode genesis`: initializes a world (used by `aos init`).
- `--import-mode patch`: emits a PatchDocument (used by governance/commit flows).
- `--air-only`: ignores modules (replacement for `gov propose --patch-dir`).
- Source sync is handled by workspaces (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).

### Governance integration
```
aos import --air <dir> --import-mode patch --air-only --propose [--shadow] [--approve] [--apply]
```
Behavior:
- Builds PatchDocument via World IO, then runs governance steps.
- Replaces `aos gov propose --patch-dir`.

### Build integration (optional)
```
aos import --air <dir> --import-mode patch --build
```
Behavior:
- Builds wasm externally, stores blobs, and updates `defmodule.wasm_hash` before patch generation.

## Migration notes
- `gov propose --patch-dir` is deprecated (hidden in help) and redirects users to `aos import --air --air-only`.
- Keep `gov propose --patch` for raw PatchDocument / ManifestPatch inputs.
- `aos init` becomes a thin wrapper: template -> `aos import --import-mode genesis`.

## Proposed work
1) [x] Implement `aos export` backed by World IO.
2) [x] Implement `aos import` with `--air` inputs.
3) [x] Wire `aos init` to the genesis import path.
4) [x] Deprecate `gov propose --patch-dir` and add a compatibility shim.
5) [x] Add examples documenting the new import/export workflow.

## Open questions
- None (commit wrapper deferred for now).
