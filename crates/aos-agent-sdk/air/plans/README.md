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
- `core_workspace_sync.air.json`: composed workspace sync parent that
  spawns prompt/tool subplans and merges results into one
  `aos.agent/SessionEventKind@1` output.
  Exports `aos.agent/core_workspace_sync@1`.
