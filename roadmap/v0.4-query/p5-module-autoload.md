# Task: Module autoload without hardcoded wasm_hashes

**Goal**: Allow worlds (and CLI flows like `aos world fs`) to load required modules referenced in `air/*.json` without embedding concrete `wasm_hash` values in the manifest/defs. Improve DX so placeholder hashes are resolved automatically while avoiding accidental cross-patching.

## Problem
- Today manifests often include placeholder hashes for modules.
- `aos-cli` currently patches *all* placeholder modules with the single reducer it just compiled from `reducer/`, which breaks worlds that include multiple modules (e.g., system reducers like `sys/ObjectCatalog@1`).
- Authors have to pin `wasm_hash` values (and populate blobs) in example worlds to prevent mispatching, which hurts authoring ergonomics and upgrade flow.

## Desired Behavior
- CLI should patch only explicitly requested modules (e.g., `--module notes/NotebookSM@1`) and leave others untouched.
- Known system modules (ObjectCatalog, etc.) should be auto-resolved from built artifacts (e.g., `aos-sys` build outputs or workspace cache) when their hash is missing, without requiring authors to copy hashes into `air/`.
- Worlds can ship `air/` files with placeholder hashes and have a deterministic resolution path that avoids “wrong module patched” traps.

## Acceptance Criteria
- Running `aos world fs ...` (and other world commands) against a world with multiple placeholder modules does not mispatch modules.
- System modules referenced in `air/` load automatically if built locally, or produce a clear error instructing how to build/fetch them; no need to hardcode hashes in example manifests.
- Regression tests cover multi-module worlds with placeholders, ensuring correct module selection and replay.

## Proposed Steps
- Add a CLI option `--module <Name>` to target patching to specific modules; default behavior should **not** patch all placeholders indiscriminately.
- Module resolution order:
  1) If `wasm_hash` present, use it.
  2) If placeholder and `--module` matches, patch with compiled hash.
  3) If placeholder and module name matches a known system module, attempt to load hash from `aos-sys` build cache/target (debug by default) or from store if already present; otherwise emit a guided error.
  4) If placeholder and unresolved, fail with a clear message listing missing modules.
- Add helper to locate system module artifacts (e.g., `target/wasm32-unknown-unknown/debug/object_catalog.wasm`) and compute hash on the fly; optionally populate blob store.
- Update `aos-cli` manifest loader tests to cover multi-module placeholder scenarios.
- Update docs (examples/README, roadmap v0.4-query) explaining the new resolution flow and how to override with `--module`.

## Notes / Dependencies
- Leverages existing `aos-sys` binaries for system reducers; consider adding a small registry mapping module name → path to wasm artifact.
- Keep deterministic behavior: resolution must be reproducible; fail loudly if artifacts are missing instead of silently patching wrong modules.
- Align with WorldFS/introspection work so DX is consistent across CLI surfaces.
