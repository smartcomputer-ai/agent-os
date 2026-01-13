# P1: HTTP Publish Registry (Workspace Mapping)

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks deterministic HTTP publishing)  
**Status**: Complete

## Goal

Provide a first-class, auditable registry that maps HTTP routes to workspace
references, so host serving is a thin, deterministic layer over
`workspace.resolve` and tree effects.

## Motivation

Publishing should not be a bespoke host config. It should be:
- **Deterministic** (route -> workspace ref -> root hash),
- **Auditable** (stored in reducer state), and
- **Policy-friendly** (governance/routing gated and scoped).

## Decision Summary

1) Add a registry reducer `sys/HttpPublish@1` that stores publish rules.
2) Each rule maps a host/path prefix to a `sys/WorkspaceRef@1`.
3) The host server reads the registry and serves via `workspace.resolve` and
   tree effects only (no compute or reducer logic).
4) HTTP headers (content-type, cache-control, etc.) are sourced from workspace
   annotations on the resolved entry (no blob metadata).

## Concepts

- **Route**: `path_prefix` with longest-prefix match.
- **Pinned vs HEAD**: immutable caching is only allowed for pinned versions.
- **Path normalization**: request paths are normalized before matching.

## Data Model (Schemas)

### 1) Publish Rule
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishRule@1",
  "type": {
      "record": {
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
- `cache = "immutable"` is only valid when `workspace.version` is pinned.
- `workspace.path` is joined with the request suffix path if set.
- `workspace.path` follows workspace path rules (no leading `/`).
- `workspace.version = none` resolves to HEAD.
- `route_prefix` should be URL-safe and begin with `/`.

## Path Normalization and Matching

1) Strip query and fragment from the request URI.
2) Percent-decode the path.
3) Normalize slashes: collapse multiple `/` into one. Record whether the path
   ends with `/` (after normalization).
4) For matching only, remove a trailing `/` unless the path is exactly `/`.
5) Validate segments against workspace path rules (`[A-Za-z0-9._~-]`).
   If any segment is invalid, return 404.
6) Match rules by path prefix *by segment*: `/app` matches `/app` and
   `/app/...` but not `/apple`.
7) Choose the longest-prefix match (most segments).

## Trailing Slash Canonicalization

- Path normalization is for matching only; the original trailing slash is
  preserved for response decisions.
- If the resolved target is a directory and the request URL did not end in `/`,
  return a 308 redirect to the slash version (preserve the query string).
- If the resolved target is a file and the request URL ends in `/`, return 404.

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
1) Select the best rule by `path_prefix` using longest-prefix match.
2) Compute `WorkspaceRef`:
   - `workspace = rule.workspace.workspace`
   - `version = rule.workspace.version`
   - `path = join(rule.workspace.path, request_suffix)`
3) Call `workspace.resolve`.
4) If the resolved target is a directory and the request URL did not end in
   `/`, return a 308 redirect to the slash version (preserve the query string).
5) Serve via tree effects:
   - file: `workspace.read_bytes`
   - directory: `workspace.list` (`scope = subtree`) or export archive
   - if `Range: bytes=start-end` is present for files, serve a single range via
     `workspace.read_bytes.range` and return `206` with `Content-Range`.
6) If a requested file is missing under a directory path and `default_doc` is
   set, serve `default_doc` instead. Otherwise return 404.
7) If the request targets a directory path (empty suffix) and
   `allow_dir_listing = true`, the host may return a directory listing instead
   of a file response.
8) If serving bytes, read workspace annotations at the resolved path and map
   known keys to HTTP headers (see HTTP Annotation Semantics). Rule-derived
   headers (Cache-Control, ETag) override annotation values.

## HTTP Annotation Semantics

- Workspace annotations are `map<text, hash>`; for `http.*` keys the referenced
  blob must be UTF-8 bytes representing the header value exactly.
- If the blob is not valid UTF-8, ignore the header.
- Header precedence is by path proximity: file annotation overrides the nearest
  directory annotation, which overrides none.
- The host should honor a safe allowlist of headers:
  `http.content-type`, `http.content-encoding`, `http.content-language`,
  `http.content-disposition`, `http.cache-control`.

## Caching Semantics

- `cache = "etag"`: set `ETag = entry_hash` and `Cache-Control: no-cache` (or
  `max-age=0`) to force revalidation. `http.cache-control` may be honored when
  `cache = "etag"` to override the default.
- `cache = "immutable"`: only allowed when `workspace.version` is pinned.
  Set `Cache-Control: public, max-age=31536000, immutable` and
  `ETag = entry_hash`. Ignore `http.cache-control` in this mode.
- The host may include `X-AOS-Root-Hash = root_hash` for debugging.

## Security and Policy

- The host process must hold `sys/workspace@1` caps with `read/list` scoped to
  the published workspaces and prefixes.
- Registry updates should be gated by governance/manifest routing and not
  performed by arbitrary reducers or unauthenticated event injection.

## Tests

- Registry apply/remove: rules present/removed deterministically.
- Longest-prefix route match.
- Path normalization and segment-boundary matching.
- Trailing-slash redirect for directory targets.
- Serving: workspace.resolve + file vs directory behavior.
- Annotation header mapping (UTF-8 decoding and precedence).
- Cache headers for `etag` vs `immutable`.

## Done

- Built-in schemas/types for publish rules and registry.
- `sys/HttpPublish@1` reducer and built-in module wiring.
- Host matcher helpers: normalization, longest-prefix selection, suffix handling.
- Publish serving logic: workspace resolve, default-doc fallback, dir listing, headers, caching.
