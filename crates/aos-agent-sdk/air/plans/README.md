# Plan Templates

Reusable `aos.agent/*` plan templates live here.

Current templates:
- `core_workspace_sync.air.json`: resolves a workspace snapshot and returns
  a `aos.agent/SessionEventKind@1` result with either
  `WorkspaceSyncUnchanged` or `WorkspaceSnapshotReady`.
  It resolves prompt-pack/tool-catalog refs and reads their bytes for
  reducer-side JSON validation before apply.
  Exports `aos.agent/core_workspace_sync@1` for composable-core reuse.
