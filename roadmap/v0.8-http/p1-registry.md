# P1: HTTP Publish Registry (Workspace Mapping)

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks deterministic HTTP publishing)  
**Status**: Draft

## Goal

Provide a first-class, auditable registry that maps HTTP routes to workspace
references, so host serving is a thin, deterministic layer over
`workspace.resolve` and tree effects.

## Motivation

Publishing should not be a bespoke host config. It should be:
- **Deterministic** (route -> workspace ref -> root hash),
- **Auditable** (stored in reducer state), and
- **Policy-friendly** (cap gated and scoped).

## Decision Summary

1) Add a registry reducer `sys/HttpPublish@1` that stores publish rules.
2) Each rule maps a host/path prefix to a `sys/WorkspaceRef@1`.
3) The host server reads the registry and serves via `workspace.resolve` and
   tree effects only (no compute or reducer logic).
4) HTTP headers (content-type, cache-control, etc.) are sourced from workspace
   annotations on the resolved entry (no blob metadata).

## Concepts

- **Route**: `(host?, path_prefix)` with longest-prefix match.
- **Pinned vs HEAD**: a pinned version can be cached as immutable.
- **Path normalization**: request paths are normalized before matching.

## Data Model (Schemas)

### 1) Publish Rule
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishRule@1",
  "type": {
    "record": {
      "host": { "option": { "text": {} } },
      "route_prefix": { "text": {} },
      "workspace": { "ref": "sys/WorkspaceRef@1" },
      "default_doc": { "option": { "text": {} } },
      "allow_dir_listing": { "bool": {} },
      "cache": { "text": {} }
    }
  }
}
```
Notes:
- `cache` is one of: `"immutable" | "etag"`.
- `workspace.path` is joined with the request suffix path if set.
- `workspace.path` follows workspace path rules (no leading `/`).
- `workspace.version = none` resolves to HEAD.
- `route_prefix` should be URL-safe and begin with `/`.

## Path Normalization and Matching

1) Strip query and fragment from the request URI.
2) Percent-decode the path.
3) Normalize slashes: collapse multiple `/` into one, and remove a trailing `/`
   unless the path is exactly `/`.
4) Validate segments against workspace path rules (`[A-Za-z0-9._~-]`).
   If any segment is invalid, return 404.
5) Match rules by host (exact match if present) and path prefix *by segment*:
   `/app` matches `/app` and `/app/...` but not `/apple`.
6) Choose the longest-prefix match (most segments).

## File vs Directory Semantics

- If the resolved path is a file: the request suffix must be empty; any extra
  path segments -> 404.
- If the resolved path is a directory: the request suffix may include any
  number of segments, which are appended to the directory path. This is the
  default static asset behavior.

### 2) Registry State
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishRegistry@1",
  "type": {
    "record": {
      "rules": {
        "map": {
          "key": { "text": {} },
          "value": { "ref": "sys/HttpPublishRule@1" }
        }
      }
    }
  }
}
```

### 3) Registry Event
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishSet@1",
  "type": {
    "record": {
      "id": { "text": {} },
      "rule": { "option": { "ref": "sys/HttpPublishRule@1" } }
    }
  }
}
```
Notes:
- `rule = none` removes a publish rule by id.

### 4) Registry Reducer
```jsonc
{
  "$kind": "defmodule",
  "name": "sys/HttpPublish@1",
  "module_kind": "reducer",
  "abi": {
    "reducer": {
      "state": "sys/HttpPublishRegistry@1",
      "event": "sys/HttpPublishSet@1",
      "context": "sys/ReducerContext@1",
      "effects_emitted": [],
      "cap_slots": {}
    }
  }
}
```
Notes:
- This reducer is intended as a single-instance registry; `key_schema` is
  optional and omitted here.

## Host Behavior (Serving)

On request:
1) Select the best rule by `(host?, path_prefix)` using longest-prefix match.
2) Compute `WorkspaceRef`:
   - `workspace = rule.workspace.workspace`
   - `version = rule.workspace.version`
   - `path = join(rule.workspace.path, request_suffix)`
3) Call `workspace.resolve`.
4) Serve via tree effects:
   - file: `workspace.read_bytes`
   - directory: `workspace.list` (`scope = subtree`) or export archive
5) If a requested file is missing under a directory path and `default_doc` is
   set, serve `default_doc` instead. Otherwise return 404.
6) If the request targets a directory path (empty suffix) and
   `allow_dir_listing = true`, the host may return a directory listing instead
   of a file response.
7) If serving bytes, read workspace annotations at the resolved path and map
   known keys to HTTP headers (e.g., `http.content-type`,
   `http.content-encoding`, `http.cache-control`).

## Caching Semantics

- **Pinned version**: `Cache-Control: immutable`, `ETag = root_hash`.
- **HEAD**: `ETag = root_hash`, revalidate on change.

## Security and Policy

- The host process must hold `sys/workspace@1` caps with `read/list` scoped to
  the published workspaces and prefixes.
- Registry updates should be gated by a plan/cap (e.g., `publish` op) and not
  performed by arbitrary reducers.

## Tests

- Registry apply/remove: rules present/removed deterministically.
- Longest-prefix route match.
- Path normalization and segment-boundary matching.
- Serving: workspace.resolve + file vs directory behavior.
- Cache headers for pinned vs HEAD.

## Open Questions

- Should publish rules support host wildcards or only exact match?
- Do we want a dedicated `sys/http@1` cap type for publish operations?
