# P3: Defs Sync via `aos.sync.json` Imports (As Implemented)

**Status**: Implemented  
**Date**: 2026-02-17  
**Scope**: Authoring/build-time composition of AIR defs across crates/worlds, without kernel/AIR runtime changes.

## Problem

Reducer Rust code composes via Cargo dependencies, but AIR defs (schemas/modules/plans/caps/policies/secrets) were duplicated per app world.
This caused direct duplication of SDK schemas (for example `aos.agent/*`) in consuming worlds.

## Implemented Approach

We implemented defs import at tooling level:

1. `aos.sync.json` now supports `air.imports`.
2. CLI resolves import roots (path or cargo package).
3. `manifest_loader` loads multiple AIR sources:
   - one primary world AIR source (contains manifest),
   - zero or more import sources (defs-only; no manifest allowed).
4. Duplicate defs are merged by name only when content hash is identical.
5. Conflicting duplicate defs (same name, different content hash) are hard errors.

No AIR schema/kernel protocol changes were made.

## Config Surface (`aos.sync.json`)

`air.imports` entries support exactly one of:

1. `path`
2. `cargo`

### Path import

```json
{
  "air": {
    "dir": "air",
    "imports": [
      { "path": "../sdk/air/exports/session-contracts" }
    ]
  }
}
```

`path` is resolved relative to the sync map root.

### Cargo import

```json
{
  "air": {
    "dir": "air",
    "imports": [
      {
        "cargo": {
          "package": "aos-agent-sdk",
          "air_dir": "air/exports/session-contracts",
          "manifest_path": "../../Cargo.toml"
        }
      }
    ]
  }
}
```

Fields:

1. `package` (required)
2. `version` (optional; normalized by stripping leading `=`)
3. `source` (optional; full cargo source string filter)
4. `air_dir` (optional; defaults to `air`)
5. `manifest_path` (optional; see resolution rules)

`lock` is parsed but not enforced yet.

## Cargo Import Resolution Rules

Cargo imports are resolved with `cargo metadata --format-version 1 --manifest-path <...>`.

Manifest path selection:

1. `cargo.manifest_path` if set (resolved relative to sync map root),
2. else `<reducer_dir>/Cargo.toml` if present,
3. else `<world_root>/Cargo.toml` if present,
4. else error.

Package selection:

1. Match by `package` name,
2. filter by `version` if provided,
3. filter by `source` if provided,
4. error on zero matches,
5. error on multiple matches (ambiguous).

Resolved import directory = selected package root + `air_dir`.

## Loader Merge Semantics

Primary AIR root:

1. Loaded using existing world layout search (`air`, `air.*`, `defs`, `plans`).
2. May contain a manifest (exactly one required overall).

Import AIR roots:

1. Loaded directly from the provided root (recursive JSON walk).
2. Must not contain any `manifest` node.

Def merge:

1. Nodes are stored canonically in CAS.
2. Per kind+name:
   - same hash: accepted (deduped),
   - different hash: rejected as conflict.

Other existing invariants remain unchanged:

1. `sys/*` def ownership rules still apply.
2. Manifest refs are patched to actual hashes.
3. Manifest validation continues after merge.

## SDK Export Layout

Added defs-only SDK export:

1. `crates/aos-agent-sdk/air/exports/session-contracts/defs.air.json`
2. `crates/aos-agent-sdk/air/exports/session-contracts/README.md`

This export currently contains `aos.agent/*` `defschema` nodes (no manifest).

## World Migrations Done

### `crates/aos-smoke/fixtures/22-agent-live`

1. Added `aos.sync.json` with cargo import of `aos-agent-sdk` export.
2. Deleted local duplicate `air/schemas.air.json`.
3. Updated smoke harness to pass import roots when loading fixture AIR.

### `apps/demiurge`

1. Added `air.imports` cargo entry in `apps/demiurge/aos.sync.json`.
2. Added `cargo.manifest_path` to point at workspace root (`../../Cargo.toml`) so package discovery includes `aos-agent-sdk`.
3. Added imported schema ref (`aos.agent/SessionId@1`) in `apps/demiurge/air/manifest.air.json` to exercise import path.

## Validation Performed

1. `cargo fmt`
2. `cargo test -p aos-host manifest_loader::tests -- --nocapture`
3. `cargo test -p aos-cli -- --nocapture`
4. `cargo check -p aos-smoke`
5. `cargo run -p aos-cli -- --quiet -w apps/demiurge push --dry-run`
6. `cargo run -p aos-cli -- --quiet -w crates/aos-smoke/fixtures/22-agent-live push --dry-run`
7. `cargo run -p aos-cli -- --quiet -w apps/demiurge status`
8. `cargo run -p aos-cli -- --quiet -w crates/aos-smoke/fixtures/22-agent-live status`
9. `cargo run -p aos-smoke -- agent-live` (live run succeeded)

## Current Limitations / Follow-ups

1. `air.imports[].lock` is not validated yet.
2. Cargo import only resolves packages visible in selected `cargo metadata` graph; if package is not discoverable there, import fails.
3. SDK export currently copies defs as static JSON; no automated export generation pipeline yet.
