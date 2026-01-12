# P2: HTTP Host Surface (Local UI + API)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (blocks local UI + browser tooling)  
**Status**: Draft

## Goal

Provide a local HTTP server for interactive UI and tooling that:
- Serves published workspace assets using the publish registry (P1).
- Exposes a stable, deterministic API for reads and controlled writes.
- Streams ephemeral progress for long-running effects.

## Non-Goals (v0.8)

- Remote, multi-tenant hosting or public auth.
- Arbitrary compute or reducer logic inside the host.
- Rich websocket protocols beyond basic streaming.

## Decision Summary

1) The host runs an optional HTTP server bound to `127.0.0.1` by default.
2) `/api/*` is reserved for control/introspection APIs and is never routed
   through the publish registry.
3) All other routes are resolved via `sys/HttpPublish@1` rules.
4) HTTP handlers call the same in-process control handlers used by the daemon
   (no control socket hop).
5) JSON is the default wire format; CBOR is supported via content negotiation.
6) Streaming uses SSE for progress and journal tailing (ephemeral only).

## Transport and Content Types

- Default bind: `http://127.0.0.1:<port>` (configurable).
- Request bodies:
  - `application/json`: values may be provided as JSON and are schema-validated
    and canonicalized to CBOR.
  - `application/cbor`: values are canonical CBOR.
- Responses:
  - `application/json` by default.
  - `application/cbor` when `Accept: application/cbor` is provided.
- Errors are JSON with `{ code, message }` and HTTP 4xx/5xx.

## Publish Serving (P1)

- Routes are matched by `sys/HttpPublish@1` rules.
- Paths are normalized and matched by segment prefix (see P1).
- Workspace annotations provide HTTP headers (e.g., `http.content-type`).
- `/api/*` is reserved and never served via publish rules.

## API Routes (v0.8)

### General
- `GET /api/health` -> `{ ok: true, manifest_hash, journal_height }`
- `GET /api/info` -> `{ version, world_id?, manifest_hash, snapshot_hash? }`

### Manifest and Definitions
- `GET /api/manifest?consistency=<head|exact:hash|at_least:hash>`
- `GET /api/defs?kinds=...&prefix=...`
- `GET /api/defs/<kind>/<name>`

### State
- `GET /api/state/<reducer>?key_b64=...&consistency=...`
- `GET /api/state/<reducer>/cells`

### Events and Receipts
- `POST /api/events`
  - body: `{ schema, value?, value_b64? }`
- `POST /api/receipts`
  - body: `{ intent_hash, adapter_id, payload?, payload_b64? }`

### Journal
- `GET /api/journal/head`
- `GET /api/journal?from=<cursor>&limit=<n>`

### Workspace (read)
- `GET /api/workspace/resolve?workspace=...&version=...`
- `GET /api/workspace/list?root_hash=...&path=...&scope=dir|subtree&cursor=...&limit=...`
- `GET /api/workspace/read-ref?root_hash=...&path=...`
- `GET /api/workspace/read-bytes?root_hash=...&path=...&range=start-end`
- `GET /api/workspace/annotations?root_hash=...&path=...`

### Workspace (write, local/admin only)
- `POST /api/workspace/write-bytes`
  - body: `{ root_hash, path, bytes_b64, mode? }`
- `POST /api/workspace/remove`
  - body: `{ root_hash, path }`
- `POST /api/workspace/annotations`
  - body: `{ root_hash, path?, annotations_patch }`

### Blobs
- `POST /api/blob` -> `{ hash }`
- `GET /api/blob/<hash>` -> bytes

### Governance (local/admin only)
- `POST /api/gov/propose`
- `POST /api/gov/shadow`
- `POST /api/gov/approve`
- `POST /api/gov/apply`

### Streaming (SSE)
- `GET /api/stream?topics=journal,effects,plans`
  - `journal`: append-only events with cursor
  - `effects`: adapter progress (ephemeral)
  - `plans`: plan step transitions (ephemeral)

## Security

- Default bind is loopback only.
- Optional token auth can be added for non-local binds.
- Write endpoints are local/admin only (events, receipts, workspace writes,
  governance). Reads may be public when explicitly enabled.

## Notes

- The HTTP API is a thin translation layer over the control kernel; it does not
  bypass schema validation or determinism rules.
- HTTP routes reuse the control handlers directly; the control socket remains a
  separate transport for non-HTTP clients.
- CBOR payloads must be canonical; JSON inputs are canonicalized server-side.

## Open Questions

- Do we expose all control verbs over HTTP or only a curated subset?
- Should the API support `Range` headers for byte reads instead of query params?
- Do we need a dedicated publish update endpoint, or rely on normal events?
