# P7: FS Sync (Workspace <-> Filesystem)

**Priority**: P2  
**Effort**: Medium/High  
**Risk if deferred**: High (blocks local dev workflows)  
**Status**: Draft

## Goal

Provide a first-class, ergonomic way to sync local folders (reducer code and
other artifacts) with workspace trees. Sync should be orchestrated by top-level
`aos push`/`aos pull`, not by separate AIR import/export commands.

## Current state (review)

- `aos import/export` is removed; sync is handled by `aos push`/`aos pull`.
- `aos ws` supports per-file read/write but no folder sync.
- `~`-hex path encoding is specified but not implemented in CLI or world IO.

## Direction (breaking-change-friendly)

1) Make workspaces the canonical carrier for source trees (reducers, assets).  
2) Replace tar source bundles with workspace checkout/sync.  
3) Introduce a local sync file to declare AIR/build/modules/workspace bindings
   plus optional annotations.

## Sync file

File: `aos.sync.json` (world root; checked into VCS).

```json
{
  "version": 1,
  "air": { "dir": "air" },
  "build": { "reducer_dir": "reducer" },
  "modules": { "pull": false },
  "workspaces": [
    {
      "ref": "reducer",
      "dir": "reducer",
      "annotations": {
        "README.md": { "sys/commit.title": "Notes Reducer" },
        "src/lib.rs": { "sys/lang": "rust" },
        "": { "sys/commit.message": "sync from local" }
      },
      "ignore": ["target/", ".git/", ".aos/"]
    }
  ]
}
```

Notes:
- `air.dir` is the AIR JSON directory (defaults to `air`).
- `build.reducer_dir` is the reducer crate directory (defaults to `reducer`).
- `modules.pull` controls whether `aos pull` materializes `modules/`.
- `ref` is a workspace ref string: `<workspace>[@<version>][/path]`.
- `dir` is resolved relative to the map file.
- `annotations` is optional; keys are workspace paths (`""` means root).
  - Values can be either strings or JSON values.
  - String values are stored as UTF-8 text blobs.
  - Non-string values are encoded as canonical CBOR of the JSON value.
  - `aos ws ann get` should decode CBOR values to JSON for display.
- `ignore` extends `.gitignore` (no `.aosignore` support) and is relative to `dir`.

## Encoding (filesystem <-> workspace)

Workspace paths must be URL-safe; local names may not be. Use the `~`-hex
scheme on UTF-8 bytes for each segment:
- If a segment matches `[A-Za-z0-9._~-]` and does not start with `~`, keep it.
- Otherwise encode as `~` + uppercase hex of the UTF-8 bytes.
- Segments that start with `~` are always encoded.
- On export, decode segments starting with `~`. Invalid hex or odd length is an
  error.
- If decoding produces path collisions, error and require `--raw` export.
- Reject non-UTF-8 filenames on import for determinism.

## Push behavior
Push uses `aos.sync.json` by default; explicit args override it.

- `aos push` (no args) pushes every workspace entry.
- `aos push <dir> <ref>` pushes a single pair.
- Push rejects refs that include a version (`@<version>`).

Push orchestration:
1) Parse AIR JSON, canonicalize defs, and apply patch directly to the world.
2) Build reducers and patch module hashes before applying the manifest.
3) Sync workspace entries (local -> workspace).
4) Create a snapshot after patching so the world can run without AIR files.

1) Resolve workspace head (or create empty root).
2) Walk local tree (respect `.gitignore` + `ignore`), compute per-file hash + mode.
3) List workspace subtree, compute diff by path/hash/mode.
4) Apply writes/removes; update annotations.
5) Commit once with `sys/WorkspaceCommit@1` (`expected_head = resolve`).
6) Optionally set root annotations like `sys/commit.message`.

## Pull behavior
Pull uses `aos.sync.json` by default; explicit args override it.

- `aos pull` (no args) pulls every workspace entry.
- `aos pull <ref> <dir>` pulls a single pair.
- Pull allows versioned refs for reproducible checkout.

Pull orchestration:
1) Export AIR JSON from the world (omit wasm hashes by default).
2) Optionally materialize `modules/`.
3) Sync workspace entries (workspace -> local).

1) Resolve workspace head.
2) List workspace subtree; decode paths.
3) Write files locally; set executable bit for mode 755.
4) Optionally prune local files not in workspace (still respecting `.gitignore`).
5) Optionally write annotations when requested.

Safety:
- Default is no delete; require `--prune` for removes.
- If `expected_head` changes mid-sync, abort unless `--force`.
- Non-UTF-8 names or unsupported file types (symlinks, devices) error out.
- Empty directories are not represented; use a `.keep` file if needed.

## CLI surface

New commands:
- `aos push [--map <path>] [<dir> <ref>] [--dry-run] [--prune] [--message <text>]`
- `aos pull [--map <path>] [<ref> <dir>] [--dry-run] [--prune]`

Notes:
- `aos push`/`pull` uses the map file by default; ad-hoc commands do not.
- `--message` sets `sys/commit.message` on the root path.

## Implementation notes

- Do not resurrect SourceBundle/tar; source bundles are removed from `WorldBundle`.
- Add shared `~`-hex encode/decode helpers to CLI or a small utility crate.
- Batch commits in sync; avoid per-file commits.

## Open questions

- Should annotation values support JSON or CBOR (not just text)?
- Do we want a bulk `workspace.write_tree` internal effect for speed?
- Do we want per-workspace ignore rules stored in annotations?
