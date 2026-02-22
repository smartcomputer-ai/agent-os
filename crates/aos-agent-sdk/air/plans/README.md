# Plan Templates

Reusable `aos.agent/*` plan templates live here.

Current templates:
- `core_prompt_sync_from_workspace.air.json`: resolves workspace prompt-pack
  content for a selected `prompt_pack` and returns
  `WorkspaceSyncUnchanged` or `WorkspaceSnapshotReady` with prompt refs/bytes.
  Exports `aos.agent/core_prompt_sync_from_workspace@1` for composable-core
  prompt sync reuse.
- `core_tool_catalog_sync_from_workspace.air.json`: resolves workspace
  tool-catalog content for a selected `tool_catalog` and returns
  `WorkspaceSyncUnchanged` or `WorkspaceSnapshotReady` with tool refs/bytes.
  Exports `aos.agent/core_tool_catalog_sync_from_workspace@1` for
  composable-core tool sync reuse.
- `core_workspace_sync.air.json`: resolves a workspace snapshot and returns
  a `aos.agent/SessionEventKind@1` result with either
  `WorkspaceSyncUnchanged` or `WorkspaceSnapshotReady`.
  It resolves prompt-pack/tool-catalog refs and reads their bytes for
  reducer-side JSON validation before apply.
  Exports `aos.agent/core_workspace_sync@1` for composable-core reuse.
