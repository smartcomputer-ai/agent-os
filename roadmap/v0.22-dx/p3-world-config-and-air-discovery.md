# P3: Optional World Config And AIR Discovery Cleanup

Status: mostly implemented.

Completed so far:

1. `aos.world.json` is the optional local world config file.
2. The loader falls back to inferred defaults when `aos.world.json` is absent.
3. `aos.sync.json` is not kept as a compatibility alias in the normal path.
4. AIR imports no longer live in world config; direct Cargo dependencies with
   `[package.metadata.aos]` are discovered automatically.
5. Discovered packages carry package/source identity and defs hashes in authoring resolution.
6. Existing `aos.sync.json` files in the current tree have been removed or replaced with
   `aos.world.json` where operational config is still needed.
7. `build.workflow_dir` has been renamed to `build.module_dir`.
8. `modules.pull` has been removed from world config.

Still open:

1. surface discovered AIR package identities more directly from user-facing build/check commands,
2. keep explicit CLI/test-harness override behavior covered for non-Cargo fixtures,
3. decide later whether `aos.air.lock.json` is needed after the discovery model settles.

## Goal

Make local world configuration optional and operational, while moving AIR discovery to Rust/Cargo
tooling by default.

Primary outcome:

1. normal Rust-authored worlds do not use `aos.sync.json` for AIR imports,
2. `aos.world.json` becomes the optional local config file for build, workspace sync, and secret
   source hints,
3. reusable AIR packages are discovered from Cargo metadata,
4. discovered AIR packages and defs hashes are visible in diagnostics/check output,
5. hand-authored AIR remains supported without sync-file import wiring,
6. a dedicated AIR lock file is deferred until the discovery shape settles.

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

### 3) AIR dependency identity is visible first, locked later

Current `air.imports[*].lock` payloads couple import identity to sync config. That shape should be
removed in the cutover.

The first version should not introduce a lock file. Instead, authoring commands should print or
return the discovered AIR dependency identities and defs hashes so changes are reviewable:

1. package name,
2. package version and source,
3. package manifest path,
4. AIR directory or generated export binary,
5. defs hash.

A future hardening phase may add `aos.air.lock.json` once the discovery model is stable. That lock
should use the same defs hash semantics, but it is explicitly not part of the first implementation.

### 4) Workspace sync remains local operational config

Workspace sync is still checkout-local intent, not AIR. It belongs in optional world config for now.

The same is true for secret source bindings and temporary module-pull behavior. The manifest
declares secret identities; local config says how this checkout supplies values.

## Proposed `aos.world.json`

Illustrative shape:

```json
{
  "version": 1,
  "air": { "mode": "auto" },
  "build": {
    "module_dir": "workflow",
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
}
```

This file should be sparse in practice. Most worlds should not need to set `air` at all.

## Migration Plan

### Phase 3A: Read Optional `aos.world.json`

Status: implemented.

Add a new loader that checks, in order:

1. explicit CLI config path,
2. `aos.world.json`,
3. inferred defaults.

Do not keep `aos.sync.json` as a deprecated alias. Existing sync files should be migrated to
`aos.world.json` or removed.

### Phase 3B: Move AIR Source Resolution To Auto Discovery

Status: implemented for the normal authoring path.

Build `AirSource` resolution from:

1. local world/workflow generated AIR,
2. direct Cargo dependencies with `[package.metadata.aos]`,
3. checked-in local `air/` if present,
4. explicit CLI/test-harness override imports only when requested.

This removes `air.imports` from local world config.

### Phase 3C: Expose Discovered AIR Identity

Status: partially implemented.

Expose discovered AIR packages and defs hashes from build/check commands.

The first version should:

1. compute the same deterministic defs hash used by existing import locks,
2. include discovered AIR dependencies in command output and diagnostics,
3. fail only on real load/validation conflicts, not on missing lock files.

`aos.air.lock.json` remains a later phase.

### Phase 3D: Migrate Existing Sync Files

Status: implemented for existing in-tree sync files.

Remove existing `aos.sync.json` files and replace only the still-needed operational parts:

1. `worlds/demiurge/aos.sync.json` becomes optional `aos.world.json` or inferred defaults,
2. `crates/aos-smoke/fixtures/09-workspaces/aos.sync.json` becomes `aos.world.json` if workspace
   sync config is still needed,
3. `crates/aos-smoke/fixtures/22-agent-live/aos.sync.json` should drop AIR imports and use Cargo
   discovery or a test harness override,
4. all `aos-agent` AIR import wiring is removed from config files.

### Phase 3E: Remove Sync-File AIR Imports

Status: implemented for the normal authoring path.

Remove sync-file AIR import support from the normal authoring path. Docs and examples should use:

1. Cargo dependency discovery for reusable Rust packages,
2. visible discovered package/defs hash output for review,
3. CLI/test-harness overrides for non-Cargo or fixture-specific imports.

## Non-Goals

- Do not make `aos.world.json` mandatory.
- Do not make local config part of canonical AIR or replay.
- Do not remove `air/` directory loading.
- Do not require every world to use Rust-authored AIR.

## Exit Criteria

Status: mostly met, except for more direct user-facing discovered package output.

P3 is complete when:

1. a Rust-authored world with conventional layout builds without any config file,
2. Demiurge no longer uses sync config for `aos-agent` AIR import wiring,
3. build/check output reports discovered AIR dependencies and defs hashes,
4. `aos.world.json` can carry workspace sync and secret source config when needed,
5. existing `aos.sync.json` files are removed or replaced with `aos.world.json`,
6. docs clearly distinguish canonical AIR from local world config.
