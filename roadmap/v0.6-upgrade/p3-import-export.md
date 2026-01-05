# P3: Import/Export CLI (checkout/commit on World IO)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (no unified workflow for editing + upgrades)
**Status**: Superseded (replaced by `aos push`/`aos pull`)

## Status snapshot (current codebase)
- `aos import`/`aos export` are removed.
- Sync is handled by `aos push`/`aos pull` (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).
- Governance-only flows remain available via `aos gov` commands when needed.

## Dependency
Requires P2 World IO (`roadmap/v0.6-upgrade/p2-world-io.md`) to provide canonical import/export logic.

## Goal
Expose a single push/pull CLI surface for checkout/commit workflows and remove ad-hoc filesystem submission paths.

## CLI surface (proposal)

### Push (filesystem view -> world update)
```
aos push [--map <path>]
```
Behavior:
- Reads AIR JSON + reducer source, builds modules as needed.
- Applies manifest changes directly (governance optional).
- Syncs workspace trees declared in `aos.sync.json`.

### Pull (world -> filesystem view)
```
aos pull [--map <path>]
```
Behavior:
- Writes AIR JSON (omitting wasm hashes by default).
- Optionally materializes `modules/`.
- Syncs workspace trees declared in `aos.sync.json`.

## Migration notes
- `aos import`/`aos export` removed; use `aos push`/`aos pull`.
- `aos init` writes `aos.sync.json` and a minimal AIR layout.

## Proposed work
1) [x] Implement `aos export` backed by World IO.
2) [x] Implement `aos import` with `--air` inputs.
3) [x] Wire `aos init` to the genesis import path.
4) [x] Deprecate `gov propose --patch-dir` and add a compatibility shim.
5) [x] Add examples documenting the new import/export workflow.

## Open questions
- None (commit wrapper deferred for now).
