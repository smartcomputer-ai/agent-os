# P3: Workspace Overlays (Derived Trees)

**Priority**: P3  
**Effort**: Medium/High  
**Risk if deferred**: Low/Medium (limits derived views)  
**Status**: Draft

## Goal

Standardize a deterministic pattern for overlays (derived trees) without adding
read-time compute to the kernel or the HTTP server.

## Motivation

Overlays are useful for builds, docs, and previews, but dynamic overlays at
read-time would turn the host into a privileged compute engine. A plan-built
overlay keeps provenance auditable and outputs publishable.

## Decision Summary

1) Define an overlay manifest schema stored in the workspace.
2) Overlays are executed by plans and **commit derived trees**.
3) Overlay outputs are cached by `(base_root_hash, overlay_id)`.

## Data Model (Schemas)

### 1) Overlay Manifest
```jsonc
{
  "$kind": "defschema",
  "name": "sys/WorkspaceOverlay@1",
  "type": {
    "record": {
      "name": { "text": {} },
      "plan": { "text": {} },
      "base": { "ref": "sys/WorkspaceRef@1" },
      "output_workspace": { "text": {} },
      "output_path": { "text": {} },
      "params_hash": { "option": { "hash": {} } }
    }
  }
}
```
Notes:
- `params_hash` references a blob containing plan parameters.

## Convention

Store overlay manifests under:
- `/overlays/<name>.air.json` (or canonical CBOR equivalent)

A build plan:
1) Resolves `base` via `workspace.resolve`.
2) Produces a derived tree.
3) Commits output under `output_workspace` and `output_path`.

## Tests

- Overlay plan determinism: same base hash yields same output root.
- Overlay cache: reuse output when `base_root_hash` and `params_hash` match.

## Open Questions

- Do we want a registry reducer for overlay state/history?
- Should overlays be allowed to write into the same workspace as the base?
