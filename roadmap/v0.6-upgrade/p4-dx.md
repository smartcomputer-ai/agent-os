# P4: DX (in-place world workflow)

**Priority**: P4  
**Effort**: Medium  
**Risk if deferred**: Medium (slower local iteration)  
**Status**: Planned

## Goal
Make the "single world directory" workflow ergonomic by defaulting import/export
to the active world dir (AOS_WORLD / --world), so the filesystem is the working tree.

## Proposed work
1) [ ] Add `aos run --watch` to restart the daemon when `air/**` or `reducer/**` changes.
2) [ ] Add `aos run --auto-reset` to prompt/auto-reset the journal when replay fails
   due to manifest/schema mismatches (opt-in, default safe).
3) [ ] Keep `aos run` run-only (no import flag); sync-in happens via `aos import`.
4) [ ] Add `aos build` to compile reducers + patch placeholder `wasm_hash` and report
   resolved hashes without running the world.
5) [ ] Add `aos dev` alias for `run --watch --auto-reset` (or equivalent defaults).
6) [ ] Change `aos import` defaults to read from `<world>/air` and `<world>/reducer`
   (unless explicit `--air/--source` overrides are provided).
7) [ ] Change `aos export` defaults to write to `<world>` (overwriting `air/`,
   `modules/`, and `sources/` with `--force`).
8) [ ] Add `AOS_DEV`/`--dev` mode to `aos import` to auto-apply patches (no shadow by
   default) and skip governance unless explicitly requested.
9) [ ] Load `<world>/.env` for all CLI commands (not just run/import paths).
10) [ ] Document the in-place workflow in the upgrade tutorial and CLI help.
11) [ ] Add CLI tests for `--watch` restart + `--auto-reset` prompt behavior.
