# Session Contracts Export

Defs-only AIR export for SDK session contracts.

- Contains `aos.agent/*` `defschema` nodes only.
- Includes workspace-config contracts (`WorkspaceBinding`, `WorkspaceSnapshot`,
  sync/apply event schemas) in addition to core session lifecycle schemas.
- Intended for app/world import via `aos.sync.json` `air.imports`.
- Does not include a manifest node.
