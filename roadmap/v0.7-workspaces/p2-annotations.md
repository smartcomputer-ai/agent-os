# P2: Workspace Annotations (Path Metadata)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (limits metadata for tooling and publishing)  
**Status**: Draft

## Goal

Add optional, descriptive metadata to workspace paths without introducing global
semantics or changing determinism guarantees.

## Motivation

Workspace trees are content-addressed and deterministic, but many workflows
need metadata (content type, build hints, doc tags) that should remain
*descriptive* and local to the caller. Annotations provide this without imposing
a global ontology on the kernel.

## Decision Summary

1) Introduce opaque annotations as **hash -> hash** mappings stored in CAS.
2) Add `workspace.annotations_get` and `workspace.annotations_set` effects.
3) The kernel **does not interpret** annotation keys or values.

## Data Model (Schemas)

### 1) Workspace Annotations
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceAnnotations@1",
  "type": {
    "map": {
      "key": { "hash": {} },
      "value": { "hash": {} }
    }
  }
}
```
Notes:
- Keys are opaque annotation identifiers (hashes).
- Values are hashes of blobs that carry the payload.

### 2) Annotations Patch (for updates)
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceAnnotationsPatch@1",
  "type": {
    "map": {
      "key": { "hash": {} },
      "value": { "option": { "hash": {} } }
    }
  }
}
```
Notes:
- `value = none` deletes the annotation key.

## Effect Surface

#### `workspace.annotations_get`
Params:
- `root_hash: hash`
- `path: option<text>` (none = root)
Receipt:
- `annotations: option<sys/WorkspaceAnnotations@1>`

#### `workspace.annotations_set`
Params:
- `root_hash: hash`
- `path: option<text>` (none = root)
- `annotations_patch: sys/WorkspaceAnnotationsPatch@1`
Receipt:
- `{ new_root_hash, annotations_hash }`

## Representation Notes

To make annotations part of the content-addressed tree, a tree schema bump is
likely needed. Add:
- `sys/WorkspaceTree@2.annotations_hash?: hash` for the directory itself
  (including the root).
- `sys/WorkspaceEntry@2.annotations_hash?: hash` for child objects.

New writes would emit v2 nodes; readers should accept v1 and v2 during a
transition.

For updates:
- Directory annotations are stored on the directory node itself.
- File annotations are stored on the parent entry for that file.

## Conventions

- Commit-level annotations live on the root path (`path = none`) and version with
  the tree.
- Suggested keys (hashes of namespaced identifiers):
  - `sys/commit.message`
  - `sys/commit.title`
  - `sys/commit.notes`
  - `sys/tags`
  - `sys/commit.owner` (optional mirror of commit meta)
Notes:
- Key hash = `sha256(utf8(key_name))`, stored as a normal `hash` value.

## Tests

- Annotations round-trip: set/get and delete.
- Annotations hashing: stable across re-encoding.
- Mixed tree versions: read v1 and v2 nodes in the same tree.

## Open Questions

- Do we want annotation keys to optionally be `text` namespaced (in addition to hash)?
- Should annotation values support small inline CBOR for convenience?
