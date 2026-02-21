# Plan Templates

Reusable `aos.agent/*` plan templates live here.

Current templates:
- `workspace_sync_plan.air.json`: resolves a workspace snapshot and raises
  `aos.agent/WorkspaceSyncUnchanged@1` or `aos.agent/WorkspaceSnapshotReady@1`.
