# P4: DX (in-place world workflow)

**Priority**: P4  
**Effort**: Medium  
**Risk if deferred**: Medium (slower local iteration)  
**Status**: Planned

## Goal
Make the "single world directory" workflow ergonomic by defaulting push/pull
to the active world dir (AOS_WORLD / --world), so the filesystem is the working tree.

## Proposed work
1) [ ] Add `aos run --watch` to restart the daemon when `air/**` or `reducer/**` changes.
2) [ ] Add `aos run --auto-reset` to prompt/auto-reset the journal when replay fails
   due to manifest/schema mismatches (opt-in, default safe).
3) [ ] Keep `aos run` run-only (no build/import); sync happens via `aos push`.
4) [ ] Add `aos build` to compile reducers + patch placeholder `wasm_hash` and report
   resolved hashes without running the world.
5) [ ] Add `aos dev` alias for `run --watch --auto-reset` (or equivalent defaults).
6) [ ] Add `aos push`/`aos pull` backed by `aos.sync.json` (AIR, build, workspaces).
7) [ ] Remove `aos import`/`aos export` and migrate docs to push/pull.
8) [ ] Add optional governance flags to `aos push` (propose/shadow/approve/apply).
9) [ ] Load `<world>/.env` for all CLI commands (not just run/push paths).
10) [ ] Document the in-place workflow in the upgrade tutorial and CLI help.
11) [ ] Add CLI tests for `--watch` restart + `--auto-reset` prompt behavior.
