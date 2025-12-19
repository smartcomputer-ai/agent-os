# Task: Module autoload without hardcoded wasm_hashes

**Goal**: Allow worlds (and CLI flows) to load required modules referenced in `air/*.json` without embedding concrete `wasm_hash` values in the manifest/defs. Improve DX so placeholder hashes are resolved automatically without accidental cross-patching.

## Problem
- Today manifests often include placeholder hashes for modules.
- `aos-cli` currently patches *all* placeholder modules with the single reducer it just compiled from `reducer/`, which breaks worlds that include multiple modules (e.g., system reducers like `sys/ObjectCatalog@1`).
- Authors have to pin `wasm_hash` values (and populate blobs) in example worlds to prevent mispatching, which hurts authoring ergonomics and upgrade flow.

## Desired Behavior
- CLI should resolve **all** placeholder module hashes without mispatching.
- World modules should be resolved from `world/modules/<name>@<ver>-<hash>.wasm` when present.
- Known system modules (ObjectCatalog, etc.) should be auto-resolved from built artifacts (e.g., `target/wasm32-unknown-unknown/*/object_catalog.wasm`) when their hash is missing, without requiring authors to copy hashes into `air/`.
- Worlds can ship `air/` files with placeholder hashes and have a deterministic resolution path that avoids “wrong module patched” traps.

## Acceptance Criteria
- Running CLI commands against a world with multiple placeholder modules resolves all modules or fails with a clear list of missing modules.
- System modules referenced in `air/` load automatically if built locally, or produce a clear error instructing how to build/fetch them; no need to hardcode hashes in example manifests.
- Regression tests cover multi-module worlds with placeholders, ensuring correct module selection and replay.

## Proposed Steps
- Add a module resolver that fills in **all** placeholders using deterministic sources:
  1) If `wasm_hash` present, use it.
  2) If placeholder and module file exists in `world/modules/`, patch from filename hash after verifying content.
  3) If placeholder and module name matches a known system module, attempt to load hash from `target/wasm32-unknown-unknown/{debug,release}/` and optionally copy into `world/modules/`.
  4) If placeholder and unresolved, fail with a clear message listing missing modules and how to resolve.
- Keep `--module <Name>` as an explicit override for patching a specific module with the compiled reducer hash.
- Update `aos-cli` tests to cover multi-module placeholder scenarios.
- Update docs (examples/README, roadmap v0.4-query) explaining the new resolution flow.

## Notes / Dependencies
- Leverages existing `aos-sys` binaries for system reducers; consider adding a small registry mapping module name → path to wasm artifact.
- Keep deterministic behavior: resolution must be reproducible; fail loudly if artifacts are missing instead of silently patching wrong modules.
- Align with WorldFS/introspection work so DX is consistent across CLI surfaces.
