# P1: Workspaces (Name Resolution + Tree Registry)

**Priority**: P1  
**Effort**: Medium/High  
**Risk if deferred**: High (blocks in-world development UX)  
**Status**: Draft

## Goal

Replace the ObjectCatalog with a single, unified Workspace system that:
- serves as the **versioned registry** for world artifacts,
- provides a **world-native tree API** for in-world agents (browse/edit/diff), and
- makes **name → referent resolution** explicit and ergonomic for HTTP and plans.

The Workspace system is the new source of truth for code, artifacts, and
in-world editing. Tar bundles remain **interop only**.

## Motivation (What was wrong)

The current source import path stores a deterministic tarball in the CAS and
registers it in ObjectCatalog. That makes source **opaque** to in-world agents:
- cannot list files without unpacking,
- cannot edit a single file without re-tarring,
- cannot diff versions cheaply,
- does not feel world-native.

ObjectCatalog was meant as a general artifact registry, but we are not using it
for real workflows. Rather than building another system beside it, we will
**replace it** with a unified Workspace primitive.

## Decision Summary

1) **Deprecate ObjectCatalog** and replace it with a Workspace reducer.
2) **Make Workspace a superset registry**: artifacts live in trees via
   conventional paths.
3) **Introduce WorkspaceRef + workspace.resolve** for explicit name resolution.
4) **Tree operations are kernel-internal effects** (deterministic, cap-gated).
5) **Commit history is reducer state** (auditable, replayable).
6) **Commit metadata stays minimal**; descriptive metadata lives in annotations (P2).
7) **Caps scope authority** by workspace, path prefixes, and ops.
8) **Tar is only for import/export** (CLI/World IO), not a canonical format.

Breaking changes are acceptable for this milestone; no migration plan is required.

This keeps the model minimal: one registry reducer + one tree effect surface.

## Concepts

### Workspace
A workspace is a **named versioned root** stored in reducer state:
- The root is always a **tree** (source code, artifacts, metadata).
- Every commit is append-only and auditable.

### WorkspaceRef (Name vs Referent)
Plans, HTTP routes, and adapters should carry a **reference** that makes the
name → referent step explicit:
- **Stable name**: `workspace` (evolving pointer)
- **Immutable referent**: resolved `{ root_hash, version }`

### Tree
A workspace tree is a DAG of **directory nodes** stored in CAS. Each directory
node contains sorted entries; file entries point to blobs.

### Workspace as Registry
With this design, "artifact registry" is not a separate system. It is simply a
workspace whose tree uses conventional paths for artifacts.

### Commit Metadata vs Annotations
Commit metadata is reserved for correctness and history navigation. Descriptive
metadata (messages, tags, publish hints, tool metadata) belongs in annotations
attached to the tree root (see P2).

## Naming and Path Rules (URL-safe)

All names and path segments are URL-safe and deterministic.

- Allowed characters: `[A-Za-z0-9._~-]` only.
- **Workspace name**: a single segment, no `/`.
  - Regex: `^[A-Za-z0-9._~-]+$`
- **Path**: `/` is the separator; each segment must match the same regex.
  - No empty segments (`//`), no `.` or `..`, no trailing `/`.
  - Segments starting with `~` are reserved for encoded forms.

### Import/Export Encoding
Unix filenames may contain arbitrary bytes. For lossless interop:
- Encode each path segment using a `~`-hex scheme on **UTF-8 bytes**.
- If a segment is already `[A-Za-z0-9._~-]` and does **not** start with `~`,
  keep it as-is.
- Otherwise encode as `~` + uppercase hex of the UTF-8 bytes.
- Literal `~` is always encoded (so `~` never appears unescaped at segment start).
- On export, decode segments that start with `~`.
- On import, reject `~` segments with odd-length or non-hex payloads.
- On import, **reject non-UTF-8 filenames** (fail fast) to keep determinism.

This preserves 1:1 round-tripping while keeping internal paths URL-safe.

## Data Model (Schemas)

### 1) Workspace Name
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceName@1",
  "type": { "text": {} }
}
```
Notes:
- Runtime validation enforces URL-safe rules (see above).

### 2) Workspace Reference
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceRef@1",
  "type": {
    "record": {
      "workspace": { "text": {} },
      "version": { "option": { "nat": {} } },
      "path": { "option": { "text": {} } }
    }
  }
}
```
Notes:
- `version = none` resolves to HEAD.
- `path = none` resolves to the workspace root.

### 3) Workspace Commit Metadata
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceCommitMeta@1",
  "type": {
    "record": {
      "root_hash": { "hash": {} },
      "owner": { "text": {} },
      "created_at": { "time": {} }
    }
  }
}
```
Notes:
- `root_hash` is the tree root hash.
- Descriptive metadata (messages, tags) lives in annotations (P2).

### 4) Workspace History (Reducer State)
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceHistory@1",
  "type": {
    "record": {
      "latest": { "nat": {} },
      "versions": {
        "map": {
          "key": { "nat": {} },
          "value": { "ref": "sys/WorkspaceCommitMeta@1" }
        }
      }
    }
  }
}
```

### 5) Workspace Commit Event
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceCommit@1",
  "type": {
    "record": {
      "workspace": { "text": {} },
      "expected_head": { "option": { "nat": {} } },
      "meta": { "ref": "sys/WorkspaceCommitMeta@1" }
    }
  }
}
```
Notes:
- `expected_head` provides optimistic concurrency. If set and not equal to
  `state.latest`, the reducer rejects the event.
- The reducer also validates workspace naming rules.

### 6) Workspace Reducer (sys/Workspace@1)
```jsonc
{
  "$kind": "defmodule",
  "name": "sys/Workspace@1",
  "module_kind": "reducer",
  "key_schema": "sys/WorkspaceName@1",
  "abi": {
    "reducer": {
      "state": "sys/WorkspaceHistory@1",
      "event": "sys/WorkspaceCommit@1",
      "context": "sys/ReducerContext@1",
      "effects_emitted": [],
      "cap_slots": {}
    }
  }
}
```
`sys/WorkspaceName@1` is `text` with runtime validation per the rules above.

### 7) Tree Node Schemas
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceEntry@1",
  "type": {
    "record": {
      "name": { "text": {} },   // single segment, URL-safe
      "kind": { "text": {} },   // "file" | "dir"
      "hash": { "hash": {} },   // blob hash if file, tree hash if dir
      "size": { "nat": {} },    // bytes for file, 0 for dir
      "mode": { "nat": {} }     // 0o644 or 0o755
    }
  }
}

{
  "$kind": "defschema",
  "name": "sys/WorkspaceTree@1",
  "type": {
    "record": {
      "entries": { "list": { "ref": "sys/WorkspaceEntry@1" } }
    }
  }
}
```
Constraints (runtime validation):
- `entries` sorted lexicographically by `name`.
- Unique `name` within a directory.
- `name` matches URL-safe segment rules.
- `kind` is `"file"` or `"dir"`.
- `mode` is normalized: file `0644` or `0755`, dir `0755`.

## Tree Storage in CAS

Tree nodes are stored as **canonical CBOR** of `sys/WorkspaceTree@1` and hashed
with SHA-256. The resulting hash is a normal `hash` value in the store.

- For file entries, `hash` points to a blob in CAS.
- For dir entries, `hash` points to another `WorkspaceTree@1` node.

This keeps the tree content-addressed and deduplicated.

## Effect Surface (Resolution + Tree Ops)

These are **kernel-internal, plan-scope effects** (like `introspect.*`).
They are deterministic, replayable, and cap-gated.

### Cap Type
- New cap type: `workspace`
- Built-in defcap: `sys/workspace@1` with scoped authority

Suggested shape:
```jsonc
{
  "$kind": "defcap",
  "name": "sys/workspace@1",
  "type": {
    "record": {
      "workspaces": { "option": { "list": { "text": {} } } },
      "path_prefixes": { "option": { "list": { "text": {} } } },
      "ops": { "option": { "set": { "text": {} } } }
    }
  }
}
```
Notes:
- `ops` values: `"resolve" | "read" | "list" | "diff" | "write" | "commit" | "publish"`.
- Omitted fields mean "no additional restriction".
- `path_prefixes` use normalized relative paths (no leading `/`).

### Effects (names and shapes)
The exact schema names below should be added to `spec/defs/builtin-schemas.air.json`
and `spec/defs/builtin-effects.air.json`.

#### `workspace.resolve`
Params:
- `workspace: text`
- `version: option<nat>` (none = HEAD)
Receipt:
- `{ exists: bool, resolved_version?: nat, head?: nat, root_hash?: hash }`
Notes:
- If `exists = false`, other fields are absent.
- If a specific `version` is missing, return `exists = false`.
- `resolved_version` is the version actually resolved (useful when `version = none`).

#### `workspace.list`
Params:
- `root_hash: hash`
- `path: option<text>` (directory or subtree root, none = root)
- `scope: option<text>` ("dir" | "subtree", default "dir")
- `cursor: option<text>` (opaque)
- `limit: nat`
Receipt:
- `entries: list<{ path, kind, hash?, size?, mode? }>`
- `next_cursor: option<text>`
Notes:
- `entries[].path` is a normalized relative path (no leading `/`).

#### `workspace.read_ref`
Params:
- `root_hash: hash`
- `path: text`
Receipt:
- `{ kind, hash, size, mode }` or `null` when missing

#### `workspace.read_bytes`
Params:
- `root_hash: hash`
- `path: text`
- `range: option<{ start: nat, end: nat }>`
Receipt:
- `bytes`

#### `workspace.write_bytes`
Params:
- `root_hash: hash`
- `path: text`
- `bytes`
- `mode: option<nat>`
Receipt:
- `{ new_root_hash, blob_hash }`

#### `workspace.remove`
Params:
- `root_hash: hash`
- `path: text`
Receipt:
- `{ new_root_hash }`

#### `workspace.diff`
Params:
- `root_a: hash`
- `root_b: hash`
- `prefix: option<text>`
Receipt:
- `changes: list<{ path, kind, old_hash?, new_hash? }>`

### Effect Semantics
- All inputs are validated against URL-safe rules.
- Errors return `ReceiptStatus::Error` with structured error payload.
- No wall-clock access; deterministic only.
- `workspace.resolve` reads reducer state deterministically (no CAS reads).
- `workspace.read_bytes.range` uses `[start, end)` with `end` exclusive.
- `start <= end` is required; `end` past EOF returns an error.
- `workspace.remove` deletes files or empty directories; non-empty dirs error.

## Reducer Semantics

`sys/Workspace@1` reducer:
- Validates workspace name (URL-safe, no `/`).
- If `expected_head` is present and does not equal `state.latest`, reject.
- Increments `latest` and appends `meta` to `versions`.
- Does not validate `root_hash` existence (must be done by effects or caller).

This keeps reducer deterministic and small, and avoids CAS reads in reducers.

## API and UX Rules

### API Separation
All APIs take `{ workspace, version, path }` separately. No combined path format.
`sys/WorkspaceRef@1` is the standard carrier for these fields.
All workspace paths are normalized relative paths; root is represented as `none`
where a path is optional.

### CLI (planned)
- Replace `aos obj` with `aos ws`.
- `aos ws ls` lists workspaces via `introspect.list_cells`.
- `aos ws log <workspace>` reads commit history via `introspect.reducer_state`.
- `aos ws cat <workspace> <path>` uses `workspace.read_bytes`.
- `aos ws edit` uses `workspace.write_bytes` (or patch helpers).

## HTTP Publishing Mapping (Host)

Perspective is explicit at the boundary, which makes HTTP routing obvious.
Routes map cleanly to `sys/WorkspaceRef@1` plus a perspective (`file|dir|subtree`).

Suggested routes:
- `GET /ws/<name>/head/file/<path>` → `workspace.read_bytes`
- `GET /ws/<name>/head/dir/<path>` → `workspace.list` (`scope = dir`)
- `GET /ws/<name>/head/subtree/<path>` → `workspace.list` (`scope = subtree`)
- Same forms for pinned versions:
  - `/ws/<name>/v/<n>/file/<path>`
  - `/ws/<name>/v/<n>/dir/<path>`
  - `/ws/<name>/v/<n>/subtree/<path>`

The host server first calls `workspace.resolve`, then uses tree ops. This keeps
HTTP publishing a thin mapping layer over deterministic primitives.

### Import/Export Changes
- `aos import --source` becomes: build tree -> commit workspace.
- Tar remains only for interop via CLI/World IO (no workspace effects).

## Security and Policy

- All tree effects require `workspace` cap grants.
- `workspace.resolve` requires `resolve` on the named workspace.
- Workspace commit events should be emitted by trusted plans or control paths.
- If policy wants to restrict who can commit, implement a
  `WorkspaceCommitPlan@1` and gate it with caps, then use that plan as the only
  authorized committer (recommended for production).

## Tests

Add tests for:
- URL-safe validation (workspace and path segments).
- Tree canonicalization: entry ordering and hash stability.
- Workspace effects: resolve/list/read_ref/read/write/remove/diff round-trips.
- Reducer concurrency checks (`expected_head`).
- CLI/World IO `~`-hex encoding round-trip.
- Replay determinism (tree effects receipts should be replay-safe).
- Cap scoping: workspace/path/op allowlists.

## Design Rationale

- **Unify registry + workspace**: avoids two overlapping systems and aligns
  with the core mental model (versioned named roots).
- **Name → referent resolution**: makes plan/HTTP integrations explicit and
  reduces dependence on `introspect.reducer_state`.
- **Tree ops as internal effects**: reducers cannot traverse CAS; effects can,
  while still remaining deterministic and cap-gated.
- **URL-safe naming**: prevents ambiguity and enables stable identifiers.
- **Tar as interop**: avoids making the canonical representation opaque.

## Deferred Follow-ups (P2+)

- Workspace annotations/xattrs (path-level descriptive metadata): `roadmap/v0.7-workspaces/p2-annotations.md`
- Overlays/derived trees (plan-built views): `roadmap/v0.7-workspaces/p3-overlays.md`
- Perspective handlers (userland extensions): `roadmap/v0.7-workspaces/p4-perspectives.md`
- Publish registry reducer (HTTP mappings + policy)

## Open Questions

- Do we need a `workspace.move/rename` effect (likely not; use write+remove)?
- Should we add a small index reducer for annotation queries (optional)?
