# P2: HTTP Host Surface (Local UI + API)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (blocks local UI + browser tooling)  
**Status**: Complete

## Goal

Provide a local HTTP server for interactive UI and tooling that:
- Serves published workspace assets using the publish registry (P1).
- Exposes a stable, deterministic API for reads and controlled writes.

## Non-Goals (v0.8)

- Remote, multi-tenant hosting or public auth.
- Arbitrary compute or reducer logic inside the host.
- Streaming (deferred to P4).

## Decision Summary

1) The host runs an optional HTTP server bound to `127.0.0.1` by default.
2) `/api/*` is reserved for control/introspection APIs and is never routed
   through the publish registry.
3) All other routes are resolved via `sys/HttpPublish@1` rules.
4) HTTP handlers call the same in-process control handlers used by the daemon
   (no control socket hop).
5) JSON is the default wire format; CBOR is supported via content negotiation.
6) HTTP support is built-in; enable/disable and bind via host config/env.
7) Streaming endpoints are implemented in P4.

## Implementation Notes

- Crate: `crates/aos-host`.
- Config: `HostConfig.http_server`, `AOS_HTTP_BIND`, `AOS_HTTP_DISABLE`.
- Suggested modules:
  - `crates/aos-host/src/http/mod.rs` (server startup + router)
  - `crates/aos-host/src/http/api.rs` (maps `/api/*` to control handlers)
  - `crates/aos-host/src/http/publish.rs` (publish registry routing + asset serving)
- Integration point: `crates/aos-host/src/modes/daemon.rs` next to control server startup.
- Shared logic: factor control request handling into a reusable helper so both
  control socket and HTTP routes reuse the same validation and kernel calls.

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

Streaming endpoints are defined in P4 (`p4-stream.md`).

### General
- `GET /api/openapi.json` -> OpenAPI document
- `GET /api/docs` -> Swagger UI
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

### Workspace (write)
- `POST /api/workspace/write-bytes`
  - body: `{ root_hash, path, bytes_b64, mode? }`
- `POST /api/workspace/remove`
  - body: `{ root_hash, path }`
- `POST /api/workspace/annotations`
  - body: `{ root_hash, path?, annotations_patch }`

### Blobs
- `POST /api/blob` -> `{ hash }`
- `GET /api/blob/<hash>` -> bytes

### Governance
- `POST /api/gov/propose`
- `POST /api/gov/shadow`
- `POST /api/gov/approve`
- `POST /api/gov/apply`

## Security

- Default bind is loopback only.
- Optional token auth can be added for non-local binds.
- Write endpoint hardening (auth/gating) is deferred; v0.8 relies on loopback
  binding and operator trust.

## Notes

- The HTTP API is a thin translation layer over the control kernel; it does not
  bypass schema validation or determinism rules.
- HTTP routes reuse the control handlers directly; the control socket remains a
  separate transport for non-HTTP clients.
- CBOR payloads must be canonical; JSON inputs are canonicalized server-side.

## Done

- HTTP server bootstraps in daemon mode with shared control handling.
- `/api/*` routes wired for manifest/defs/state/events/receipts/journal/workspace/blob/gov.
- Publish handler integrated as router fallback with normalized path matching.
- CBOR/JSON content negotiation implemented for requests/responses.
