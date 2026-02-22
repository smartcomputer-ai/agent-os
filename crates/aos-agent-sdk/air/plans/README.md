# Plan Templates

Reusable `aos.agent/*` plan templates live here.

Current templates:
- `core_workspace_sync.air.json`: resolves a workspace snapshot and raises
  `aos.agent/WorkspaceSyncUnchanged@1` or `aos.agent/WorkspaceSnapshotReady@1`.
  It resolves prompt-pack/tool-catalog refs and reads their bytes for reducer-side
  JSON validation before apply.
  Exports `aos.agent/core_workspace_sync@1` for composable-core reuse.
