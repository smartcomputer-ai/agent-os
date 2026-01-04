# P5: Workspace CLI (aos ws)

**Goal**: replace the old object registry CLI with a workspace-native CLI.

## Scope (updated)
- Replace `aos obj` with `aos ws`
- Wire CLI reads to `workspace.resolve` + tree ops (`list`, `read_bytes`, `read_ref`)
- Wire CLI writes to `workspace.write_bytes` / `workspace.remove`
- Commit after writes by emitting `sys/WorkspaceCommit@1`
- Lazy initialization: if a workspace does not exist, first write/annotation creates an empty root and commits it (no `ws init` required)
- Keep world_io/import/export changes out of this doc (see P7)

## Proposed CLI Surface

Workspace ref format:
- `<workspace>[@<version>][/path]`
  - `@<version>` optional (default: HEAD)
  - `/path` optional (default: root)

Commands:
- `aos ws ls [ref] [--scope dir|subtree] [--limit N] [--cursor CURSOR]`
  - `aos ws ls` (no ref) lists all workspace names
  - `aos ws ls <ref>` lists paths within the resolved tree
- `aos ws cat <ref> [--range START:END] [--raw|--out PATH]`
- `aos ws stat <ref>` (uses `read_ref`)
- `aos ws write <ref> --in <file|@-> [--mode 644|755]`
- `aos ws rm <ref>`
- `aos ws diff <refA> <refB> [--prefix PATH]`
- `aos ws log <workspace>` (reads `sys/Workspace@1` keyed state)
- `aos ws ann get <ref>`
- `aos ws ann set <ref> <key>=<hash>...`
- `aos ws ann del <ref> <key>...`

## Resolution Flow (all ops)
1) Parse ref → `{workspace, version?, path?}`
   - If `ls` is invoked without a ref, skip resolve and list workspaces instead
2) Call `workspace.resolve`:
   - If missing and op is **write/remove/ann-set/ann-del**, treat as empty tree:
     - Create empty tree node in CAS (see control verb below)
     - Commit as version 1 with `expected_head = none`
     - Use that root for the op
   - If missing and op is **read/ls/cat/stat/diff/log/ann-get**, error
3) Use `root_hash` to run the tree effect

## Commit Semantics
- After any write/remove/annotation-set, emit `sys/WorkspaceCommit@1`:
  - `workspace`: name
  - `expected_head`: resolved version (if exists)
  - `meta`: `{ root_hash, owner, created_at }`
- `created_at` uses wall-clock; deterministic replay is unaffected (events are logged).

## Control Protocol Additions (daemon path)
Add control commands that map 1:1 to internal workspace effects:
- `workspace-resolve`
- `workspace-list`
- `workspace-read-ref`
- `workspace-read-bytes`
- `workspace-write-bytes`
- `workspace-remove`
- `workspace-diff`
- `workspace-annotations-get`
- `workspace-annotations-set`
- `workspace-empty-root` (returns the hash of an empty tree node)

Each handler builds an `EffectIntent`, calls `kernel.handle_internal_intent`, and returns decoded receipt JSON.

Workspace listing (no ref):
- Use existing `state-list` for reducer `sys/Workspace@1` to list keyed cells and decode keys.

## Batch Fallback (non-daemon)
CLI uses `WorldHost` directly:
- Build intent with params
- Call `kernel.handle_internal_intent`
- Decode receipt and render

## Tests
- CLI integration: `ws ls/cat/write/rm/diff/log` (daemon + batch)
- Control protocol: workspace verbs return expected receipts
- Lazy init: first write to missing workspace creates commit and succeeds
