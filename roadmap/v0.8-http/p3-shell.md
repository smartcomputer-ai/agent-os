# P3: Shell UI (Workspace-Hosted SPA)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (slows first-agent UX)  
**Status**: Completed

## Goal

Provide a local, interactive shell UI for AgentOS without bundling UI assets in
 the kernel. The shell is a static SPA built externally and installed into a
 workspace, then published at the root route via the HTTP publish registry.

## Decision Summary

1) The shell is a React SPA built outside of AgentOS using a normal toolchain.
2) The shell lives under `shell/` in the repo; build output goes to
   `shell/dist/`.
3) The shell is installed into a workspace (e.g., `shell`) and published at
   `/` using `sys/HttpPublish@1`.
4) The kernel is not bundled with UI assets; the CLI handles build + install.

## Structure

- Source: `shell/`
- Build output: `shell/dist/` (gitignored)
- Published workspace: `shell` (default; configurable)

## Install Flow (CLI)

Add a CLI command that builds and installs the shell into the current world:

```
aos ui install \
  --world <AOS_DIR> \
  --workspace shell \
  --route /
```

Steps performed by the CLI:
1) Build the shell (e.g., `shell` -> `shell/dist`).
2) Sync `dist/` into the target workspace using `workspace.write_*`.
3) Apply workspace annotations for HTTP headers (content-type, cache-control).
4) Create or update the publish rule in `sys/HttpPublish@1`:
   - `route_prefix = /`
   - `workspace = { workspace: "shell", version: none, path: none }`
   - `default_doc = "index.html"`

## Runtime Behavior

- HTTP server serves `/api/*` API routes directly (P2).
- All other routes are matched against publish rules (P1).
- Shell assets are served from the published workspace at `/`.

## Future Extensions (Not in v0.8)

- Extension manifests so other bundles can add routes/panels to the shell.
- Multiple shell versions per world or per workspace.
- In-browser dev mode or live-reload support.

## Open Questions

Resolved:
- `aos ui install` should auto-create the publish registry if missing (same as workspaces).
- Add `--force` to overwrite existing `/` publish rules.
- Default workspace name remains `shell`, but allow override via CLI option.
