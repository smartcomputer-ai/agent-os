# P3: Import/Export CLI (checkout/commit on World IO)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (no unified workflow for editing + upgrades)
**Status**: Complete

## Status snapshot (current codebase)
- `aos export` uses World IO to materialize `air/` plus optional `modules/` and `sources/`.
- `aos export --defs-bundle` writes a single `air/defs.air.json` bundle.
- `aos import --air` supports genesis/patch modes and can drive governance steps.
- Source import/export is deferred to `roadmap/v0.7-workspaces/p7-fs-sync.md` (workspace-backed).
- `aos init` goes through the World IO genesis import path.
- `aos gov propose --patch-dir` is hidden and prints a deprecation notice.

## Dependency
Requires P2 World IO (`roadmap/v0.6-upgrade/p2-world-io.md`) to provide canonical import/export logic.

## Goal
Expose a single import/export CLI surface for checkout/commit workflows and remove ad-hoc filesystem submission paths.

## CLI surface (proposal)

### Export (world -> filesystem view)
```
aos export [--out <dir>] [--with-modules] [--with-sources] [--with-sys] [--defs-bundle] [--manifest <hash>] [--air-only]
```
Behavior:
- Uses control `manifest-get` when daemon is running; falls back to store.
- Materializes a stable `air/` layout plus optional `modules/`.
- If `--with-sources` is set and a source bundle exists, unpack it to `sources/`.
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
- `--air-only`: ignores modules/sources (replacement for `gov propose --patch-dir`).
- Source bundle import/export is deferred to `roadmap/v0.7-workspaces/p7-fs-sync.md`.

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
2) [x] Implement `aos import` with `--air` and `--source` inputs.
3) [x] Wire `aos init` to the genesis import path.
4) [x] Deprecate `gov propose --patch-dir` and add a compatibility shim.
5) [x] Add examples documenting the new import/export workflow.

## Open questions
- None (commit wrapper deferred for now).

## Source bundle format (deferred)
We only need a portable format for **source code**, not for AIR or WASM. AIR is JSON in
`air/`, and WASM is stored as blobs referenced by `defmodule.wasm_hash`. The source bundle
will be a single deterministic tarball stored as a blob and registered in workspace history.

**Deterministic tar rules**:
- Sort paths lexicographically.
- Normalize metadata: uid/gid=0, uname/gname empty, mode fixed, mtime=0.
- Preserve executable bits when present.
- Exclude build outputs (`target/`, `node_modules/`) via `.aosignore` and `.gitignore`.
  - Apply `.gitignore` first, then `.aosignore` with last-match-wins so `.aosignore`
    can tighten or override ignores.
  - Rationale: `.aosignore` is the AgentOS-specific publishing contract; it should be
    able to exclude files that are otherwise kept for developer workflows.

**Why tar**:
- Single artifact, stable hash, easy to blob-put and register.
- Maps cleanly to a local directory on export.

**Sys defs**:
- Not part of the source bundle or AIR assets; they remain built-ins.
