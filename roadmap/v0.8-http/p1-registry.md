# P1: HTTP Publish Registry (Workspace Mapping)

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks deterministic HTTP publishing)  
**Status**: Draft

## Goal

Provide a first-class, auditable registry that maps HTTP routes to workspace
references plus an explicit perspective (file/dir/subtree), so host serving is a
thin, deterministic layer over `workspace.resolve` and tree effects.

## Motivation

Publishing should not be a bespoke host config. It should be:
- **Deterministic** (route -> workspace ref -> root hash),
- **Auditable** (stored in reducer state), and
- **Policy-friendly** (cap gated and scoped).

## Decision Summary

1) Add a registry reducer `sys/HttpPublish@1` that stores publish rules.
2) Each rule maps a host/path prefix to a WorkspaceRef + perspective.
3) The host server reads the registry and serves via `workspace.resolve` and
   tree effects only (no compute or reducer logic).

## Concepts

- **Route**: `(host?, path_prefix)` with longest-prefix match.
- **Perspective**: `file | dir | subtree` to avoid ambiguous semantics.
- **Pinned vs HEAD**: a pinned version can be cached as immutable.

## Data Model (Schemas)

### 1) Publish Key
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishKey@1",
  "type": { "text": {} }
}
```

### 2) Publish Rule
```jsonc
{
  "$kind": "defschema",
  "name": "sys/HttpPublishRule@1",
  "type": {
    "record": {
      "host": { "option": { "text": {} } },
      "route_prefix": { "text": {} },
      "workspace": { "text": {} },
      "version": { "option": { "nat": {} } },
      "workspace_path_prefix": { "option": { "text": {} } },
      "perspective": { "text": {} },
      "default_doc": { "option": { "text": {} } },
      "mode": { "text": {} },
      "allow_dir_listing": { "bool": {} },
      "cache": { "text": {} }
    }
  }
}
```
Notes:
- `perspective` is one of: `"file" | "dir" | "subtree"`.
- `mode` is one of: `"static" | "spa"`.
- `cache` is one of: `"immutable" | "etag"`.
- `workspace_path_prefix` is joined with the request suffix path.
- `workspace_path_prefix` follows workspace path rules (no leading `/`).
- `version = none` resolves to HEAD.
- `route_prefix` should be URL-safe and begin with `/`.

### 3) Registry State
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

### 4) Registry Event
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

### 5) Registry Reducer
```jsonc
{
  "$kind": "defmodule",
  "name": "sys/HttpPublish@1",
  "module_kind": "reducer",
  "key_schema": "sys/HttpPublishKey@1",
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
- This reducer can be single-instance (fixed key) if preferred.

## Host Behavior (Serving)

On request:
1) Select the best rule by `(host?, path_prefix)` using longest-prefix match.
2) Compute `WorkspaceRef`:
   - `workspace = rule.workspace`
   - `version = rule.version`
   - `path = join(rule.workspace_path_prefix, request_suffix)`
3) Call `workspace.resolve`.
4) Serve via tree effects:
   - `file`: `workspace.read_bytes`
   - `dir`: `workspace.list` (`scope = dir`)
   - `subtree`: `workspace.list` (`scope = subtree`) or export archive
5) Apply `mode`:
   - `static`: missing file -> 404
   - `spa`: missing file -> `default_doc` (if set)

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
- Serving: workspace.resolve + file/dir/subtree paths.
- Cache headers for pinned vs HEAD.

## Open Questions

- Should publish rules support host wildcards or only exact match?
- Do we want a dedicated `sys/http@1` cap type for publish operations?
