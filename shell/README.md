# AgentOS Shell

> Status: outdated. This web shell does not currently work with the active AgentOS runtime. It was built against an older control API shape and should be treated as a dormant UI prototype until it is updated.

`shell/` is a React/Vite web UI intended to browse and operate AgentOS worlds from a browser. Its original purpose was to provide a graphical shell for inspecting manifests, definitions, workspace trees, governance drafts, and Demiurge chat/session state.

The current runtime has moved to the unified `aos node` control surface, hot in-process reads, updated workspace/backend semantics, and newer API routes. This app has not been brought forward to that model, so routes, generated API types, assumptions about backend ports, and some feature pages are expected to be stale.

## What Is Here

- React 19 + React Router app under `src/`.
- TanStack Query based SDK helpers under `src/sdk/`.
- Manifest, workspace, governance, and Demiurge feature areas under `src/features/`.
- shadcn/Radix-style UI components under `src/components/ui/`.
- Vite dev/build configuration.

## Historical Development Commands

These commands describe the project shape, but they are not a guarantee that the app works against the current runtime:

```bash
npm install
npm run dev
npm run build
npm run lint
npm run openapi:types
```

The Vite dev server is configured for port `7778` and proxies `/api` to `http://localhost:7777`. That backend assumption is part of what needs revisiting before the shell can be used with the current node.

## Before Reviving It

To make this shell current again, first align it with the active `aos node up` control API and regenerate the OpenAPI types from the current node. Then audit the feature routes against the current manifest, workspace, governance, and Demiurge flows before treating the UI as supported.
