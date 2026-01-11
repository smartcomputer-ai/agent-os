# P4: Perspective Handlers (Userland Extensions)

**Priority**: P4  
**Effort**: Medium  
**Risk if deferred**: Low/Medium (limits non-structural views)  
**Status**: Draft

## Goal

Enable expandable, semantic "perspectives" (e.g., spa, markdown, tar) without
kernel changes, by dispatching to userland handlers built on workspace
primitives.

## Motivation

If the kernel encodes perspectives as an enum, every new view becomes a kernel
change. The kernel should only expose structural, semantics-free views (file,
dir, subtree). Everything else is userland and composable.

## Decision Summary

1) Keep kernel perspectives minimal and structural only.
2) Add a userland registry of perspective handlers.
3) Host servers dispatch to **pure handlers** for non-structural perspectives.

## Concepts

### Structural Perspectives (Kernel)
These map directly to the Merkle tree model and are stable:
- **file**: read blob bytes
- **dir**: list directory entries
- **subtree**: traverse/export a tree

These are covered by `workspace.read_bytes`, `workspace.list`, and `workspace.diff`
(or a future archive export).
Notes:
- `file` means the path resolves to a file entry in the tree.

### Semantic Perspectives (Userland)
Everything else is semantic and should be a handler:
- `spa` (index fallback)
- `markdown` (render)
- `tar` or `zip` (archive)
- `image.resize` (transform)

## Data Model (Schemas)

### 1) Perspective Handler
```jsonc
{
  "$kind": "defschema",
  "name": "sys/PerspectiveHandler@1",
  "type": {
    "record": {
      "exec": { "text": {} },
      "module": { "text": {} },
      "entrypoint": { "text": {} },
      "default_mime": { "option": { "text": {} } }
    }
  }
}
```
Notes:
- `exec` is `"host_pure"` for v0.

### 2) Perspective Registry State
```jsonc
{
  "$kind": "defschema",
  "name": "sys/PerspectiveRegistryState@1",
  "type": {
    "record": {
      "handlers": {
        "map": {
          "key": { "text": {} },
          "value": { "ref": "sys/PerspectiveHandler@1" }
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
  "name": "sys/PerspectiveRegistrySet@1",
  "type": {
    "record": {
      "name": { "text": {} },
      "handler": { "option": { "ref": "sys/PerspectiveHandler@1" } }
    }
  }
}
```
Notes:
- `handler = none` removes a handler by name.

### 4) Registry Reducer (optional)
```jsonc
{
  "$kind": "defmodule",
  "name": "sys/PerspectiveRegistry@1",
  "module_kind": "reducer",
  "key_schema": "sys/WorkspaceName@1",
  "abi": {
    "reducer": {
      "state": "sys/PerspectiveRegistryState@1",
      "event": "sys/PerspectiveRegistrySet@1",
      "context": "sys/ReducerContext@1",
      "effects_emitted": [],
      "cap_slots": {}
    }
  }
}
```
Notes:
- The reducer can be single-instance (fixed key) if preferred.

## Host Dispatch

1) Parse request -> `(WorkspaceRef, perspective)`.
2) If perspective is structural, call workspace effects directly.
3) Else look up handler and run it as a **host-pure module**.

Handlers are invoked per request as pure transforms. The host performs
`workspace.resolve`, `workspace.read_bytes`, and `workspace.list`, then passes
bytes/metadata into the handler. No kernel changes are required to add new
perspectives.

## Capabilities

- The host process must hold `sys/workspace@1` caps for read/list.
- Registry updates should be gated by policy or a plan.

## Examples

### `spa`
- If path exists as file, serve it.
- Else if path is dir, try `index.html`.
- Else fallback to `/index.html`.

### `tar`
- Traverse subtree using `workspace.list` and stream an archive.

## Open Questions

- Should handler inputs/outputs be standardized as schemas for host adapters?
- Do we want a default registry location (fixed reducer key)?
