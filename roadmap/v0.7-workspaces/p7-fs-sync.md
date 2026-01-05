# P7: FS Sync (Workspace <-> Filesystem)

**Priority**: P2  
**Effort**: Medium/High  
**Risk if deferred**: High (blocks local dev workflows)  
**Status**: Draft

## Goal

Provide a first-class, ergonomic way to sync local folders (reducer code and
other artifacts) with workspace trees. Keep AIR JSON import/export separate; we
do not need to mirror AIR assets into workspaces.

## Current state (review)

- `aos import/export` handles AIR only; source bundles are removed
  (source bundles are never populated).
- `aos ws` supports per-file read/write but no folder sync.
- `~`-hex path encoding is specified but not implemented in CLI or world IO.

## Direction (breaking-change-friendly)

1) Make workspaces the canonical carrier for source trees (reducers, assets).  
2) Keep AIR JSON import/export as-is.  
3) Replace tar source bundles with workspace checkout/sync.  
4) Introduce a local workspace map to declare folder <-> workspace bindings
   plus optional annotations.

## Workspace map

File: `aos.workspaces.json` (world root; checked into VCS).

```json
{
  "version": 1,
  "workspaces": [
    {
      "ref": "reducer",
      "local_dir": "reducer",
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
- `ref` is a workspace ref string: `<workspace>[@<version>][/path]`.
- `local_dir` is resolved relative to the map file.
- `annotations` is optional; keys are workspace paths (`""` means root).
  - Values can be either strings or JSON values.
  - String values are stored as UTF-8 text blobs.
  - Non-string values are encoded as canonical CBOR of the JSON value.
  - `aos ws ann get` should decode CBOR values to JSON for display.
- `ignore` extends `.gitignore` (no `.aosignore` support).

## Encoding (filesystem <-> workspace)

Workspace paths must be URL-safe; local names may not be. Use the `~`-hex
scheme on UTF-8 bytes for each segment:
- If a segment matches `[A-Za-z0-9._~-]` and does not start with `~`, keep it.
- Otherwise encode as `~` + uppercase hex of the UTF-8 bytes.
- Literal `~` is always encoded.
- On export, decode segments starting with `~`. Invalid hex or odd length is an
  error.
- If decoding produces path collisions, error and require `--raw` export.
- Reject non-UTF-8 filenames on import for determinism.

## Push behavior
Push uses the map file by default; explicit args override it.

- `aos ws push` (no args) pushes every map entry.
- `aos ws push <local_dir> <ref>` pushes a single pair.
- Push rejects refs that include a version (`@<version>`).

1) Resolve workspace head (or create empty root).
2) Walk local tree (respect `.gitignore` + `ignore`), compute per-file hash + mode.
3) List workspace subtree, compute diff by path/hash/mode.
4) Apply writes/removes; update annotations.
5) Commit once with `sys/WorkspaceCommit@1` (`expected_head = resolve`).
6) Optionally set root annotations like `sys/commit.message`.

## Pull behavior
Pull uses the map file by default; explicit args override it.

- `aos ws pull` (no args) pulls every map entry.
- `aos ws pull <ref> <local_dir>` pulls a single pair.
- Pull allows versioned refs for reproducible checkout.

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
- `aos ws push [--map <path>] [<local_dir> <ref>] [--dry-run] [--prune] [--message <text>]`
- `aos ws pull [--map <path>] [<ref> <local_dir>] [--dry-run] [--prune]`

Notes:
- `aos ws push`/`pull` uses the map file by default; ad-hoc commands do not.
- `--message` sets `sys/commit.message` on the root path.

## Implementation notes

- Do not resurrect SourceBundle/tar; remove `source_bundle` from `WorldBundle`
  and `--with-sources` from `aos export`.
- Add shared `~`-hex encode/decode helpers to CLI or a small utility crate.
- Batch commits in sync; avoid per-file commits.

## Open questions

- Should annotation values support JSON or CBOR (not just text)?
- Do we want a bulk `workspace.write_tree` internal effect for speed?
- Do we want per-workspace ignore rules stored in annotations?
