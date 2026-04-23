# P3: Optional World Config And AIR Discovery Cleanup

Status: planned.

## Goal

Make local world configuration optional and operational, while moving AIR discovery to Rust/Cargo
tooling by default.

Primary outcome:

1. normal Rust-authored worlds do not use `aos.sync.json` for AIR imports,
2. `aos.world.json` becomes the optional local config file for build, workspace sync, and secret
   source hints,
3. reusable AIR packages are discovered from Cargo metadata,
4. AIR import identity moves to a dedicated lock file instead of sync import entries,
5. hand-authored AIR remains supported without sync-file import wiring.

## Design Stance

### 1) `aos.world.json` is optional and non-canonical

`aos.world.json` describes how this checkout builds, syncs, and supplies local operator inputs for a
world. It is not part of world identity, manifest hashing, replay, or governance.

If the file is absent, authoring commands should infer conventional defaults:

1. local AIR from `air/` when present,
2. local workflow crate from `workflow/Cargo.toml` when present,
3. Rust-authored AIR from the workflow crate export binary when present,
4. reusable AIR from direct Cargo dependencies with `[package.metadata.aos]`,
5. no workspace sync roots,
6. no secret source config beyond explicit CLI inputs and existing env fallbacks.

CLI flags override `aos.world.json`.

### 2) AIR imports should not live in world config by default

The normal Rust-authored path should be:

```text
workflow Cargo.toml dependency
  -> cargo metadata
  -> package.metadata.aos
  -> AirSource::GeneratedRustPackage or AirSource::Directory
  -> merged AIR defs
  -> manifest ref patching and validation
```

Explicit AIR imports are still useful, but they should be treated as command-line or
test-harness-only overrides for:

1. non-Cargo AIR packages,
2. smoke fixtures that intentionally exercise hand-authored import behavior,
3. migration/debugging while package metadata is incomplete.

They should not live in `aos.world.json`, and they should not be the primary developer-facing
package import mechanism.

### 3) AIR lock identity should move out of sync config

Current `air.imports[*].lock` payloads couple import identity to sync config. That shape should be
removed. Import identity should live in a dedicated file, for example:

```text
aos.air.lock.json
```

The lock should record discovered and explicit AIR dependencies after generation/materialization:

```json
{
  "version": 1,
  "packages": [
    {
      "source": "cargo",
      "package": "aos-agent",
      "version": "0.1.0",
      "source_id": null,
      "manifest_path": "workflow/Cargo.toml",
      "air_dir": "air",
      "defs_hash": "sha256:..."
    }
  ]
}
```

Local commands may warn when the lock is missing or stale. CI/strict mode should fail.

### 4) Workspace sync remains local operational config

Workspace sync is still checkout-local intent, not AIR. It belongs in optional world config for now.

The same is true for secret source bindings and temporary module-pull behavior. The manifest
declares secret identities; local config says how this checkout supplies values.

## Proposed `aos.world.json`

Illustrative shape:

```json
{
  "version": 1,
  "air": {
    "mode": "auto",
    "lock": "aos.air.lock.json"
  },
  "build": {
    "workflow_dir": "workflow",
    "profile": "debug",
    "module": "demiurge/Demiurge_wasm@1"
  },
  "workspaces": [
    {
      "ref": "main",
      "dir": "workspace",
      "ignore": [".git", "target"]
    }
  ],
  "secrets": {
    "sources": [
      { "name": "local_env", "kind": "dotenv", "path": ".env" }
    ],
    "bindings": [
      {
        "binding": "llm/openai_api",
        "from": { "source": "local_env", "key": "OPENAI_API_KEY" }
      }
    ]
  },
  "modules": {
    "pull": false
  }
}
```

This file should be sparse in practice. Most worlds should not need to set `air` at all.

## Migration Plan

### Phase 3A: Read Optional `aos.world.json`

Add a new loader that checks, in order:

1. explicit CLI config path,
2. `aos.world.json`,
3. inferred defaults.

Do not keep `aos.sync.json` as a deprecated alias. Existing sync files should be migrated to
`aos.world.json` or removed.

### Phase 3B: Move AIR Source Resolution To Auto Discovery

Build `AirSource` resolution from:

1. local world/workflow generated AIR,
2. direct Cargo dependencies with `[package.metadata.aos]`,
3. checked-in local `air/` if present,
4. explicit CLI/test-harness override imports only when requested.

This removes `air.imports` from local world config.

### Phase 3C: Add `aos.air.lock.json`

Create lock read/write/check helpers for discovered AIR dependencies.

The lock should cover both generated and checked-in package AIR after materialization, using the
same defs hash semantics as existing import locks.

### Phase 3D: Migrate Existing Sync Files

Remove existing `aos.sync.json` files and replace only the still-needed operational parts:

1. `worlds/demiurge/aos.sync.json` becomes optional `aos.world.json` or inferred defaults,
2. `crates/aos-smoke/fixtures/09-workspaces/aos.sync.json` becomes `aos.world.json` if workspace
   sync config is still needed,
3. `crates/aos-smoke/fixtures/22-agent-live/aos.sync.json` should drop AIR imports and use Cargo
   discovery or a test harness override,
4. all `aos-agent` AIR import wiring is removed from config files.

### Phase 3E: Remove Sync-File AIR Imports

Remove sync-file AIR import support from the normal authoring path. Docs and examples should use:

1. Cargo dependency discovery for reusable Rust packages,
2. `aos.air.lock.json` for import identity,
3. CLI/test-harness overrides for non-Cargo or fixture-specific imports.

## Non-Goals

- Do not make `aos.world.json` mandatory.
- Do not make local config part of canonical AIR or replay.
- Do not remove `air/` directory loading.
- Do not require every world to use Rust-authored AIR.

## Exit Criteria

P3 is complete when:

1. a Rust-authored world with conventional layout builds without any config file,
2. Demiurge no longer uses sync config for `aos-agent` AIR import wiring,
3. `aos.air.lock.json` records discovered AIR dependency identity,
4. `aos.world.json` can carry workspace sync and secret source config when needed,
5. existing `aos.sync.json` files are removed or replaced with `aos.world.json`,
6. docs clearly distinguish canonical AIR from local world config.
