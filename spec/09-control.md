# Control Channel Specification (v1)

Status: **experimental, socket-only**. Governance verbs are live. Stdio framing, CBOR framing, and journal streaming remain deferred. 

## Goals

- Provide a deterministic, versioned control plane to interact with a running world/daemon.
- Keep execution semantics unchanged: timers and effect dispatch stay in the daemon; control is I/O + translation.
- Favor local-only transports for now (Unix domain socket in world directory).

## Transport

- Default endpoint: `<world>/.aos/control.sock` (Unix domain socket).
- Permissions: server sets `0600` on the socket path (best-effort).
- Stdio mode, Windows named pipe, and TCP are out of scope for v1.

## Framing

- NDJSON (one JSON object per line, `\n` terminated).
- CBOR framing is deferred.

## Envelope

Request:
```
{
  "v": 1,                 // protocol version (required)
  "id": "<opaque id>",   // client-chosen correlation id (required)
  "cmd": "<verb>",       // method/verb (required)
  "payload": { ... }      // verb-specific data (optional)
}
```

Response:
```
{
  "id": "<same as request>",
  "ok": true|false,
  "result": { ... },       // present when ok=true
  "error": {               // present when ok=false
    "code": "...",
    "message": "..."
  }
}
```

## Error Codes

- `invalid_request` — missing/invalid fields (e.g., wrong version, missing schema/value).
- `decode_error` — malformed JSON/base64.
- `unknown_method` — verb not recognized.
- `host_error` — underlying host/kernel error.
- `timeout` — reserved for client-side timeout reporting (client maps tokio timeout to this).
- `not_running` — reserved for future remote-daemon detection.

## Verbs (v1.1 control/introspection)

- `event-send { schema, value_b64 }` → enqueues a DomainEvent and runs one daemon cycle. `value_b64` must be canonical CBOR. Timers still wait for their deadlines.
- `receipt-inject { intent_hash, adapter_id, payload_b64 }` → injects an effect receipt (CBOR base64 payload).
- `manifest-get { consistency?: "head"|"exact:<h>"|"at_least:<h>" }` → returns `{ manifest, journal_height, snapshot_hash?, manifest_hash }`.
- `state-get { reducer, key_b64?, consistency?: "..."} ` → returns `{ state_b64?, meta:{ journal_height, snapshot_hash?, manifest_hash } }`.
- `state-list { reducer }` → returns `{ cells:[{ key_b64, state_hash, size, last_active_ns }], meta:{ journal_height, snapshot_hash?, manifest_hash } }`.
- `def-get { name }` → returns `{ def, hash }` where `def` is the manifest entry for that name (`defschema`/`defmodule`/`defplan`/`defcap`/`defeffect`/`defpolicy`) and `hash` is the canonical CBOR hash of that def (`sha256:...`); errors if missing.
- `def-list { kinds?: ["defschema"|"defmodule"|"defplan"|"defcap"|"defeffect"|"defpolicy"|"schema"|"module"|"plan"|"cap"|"effect"|"policy"], prefix?: "..." }` → returns `{ defs:[{ kind, name, hash, cap_type?, params_schema?, receipt_schema?, plan_steps?, policy_rules? }], meta }` sorted by name (aliases normalized to `$kind`).
- `journal-head {}` → returns `{ journal_height, snapshot_hash?, manifest_hash }`.
- `workspace-resolve { workspace, version? }` → returns `{ exists, resolved_version?, head?, root_hash? }`.
- `workspace-empty-root { workspace }` → returns `{ root_hash }`.
- `workspace-list { root_hash, path?, scope?, cursor?, limit }` → returns `{ entries:[{ path, kind, hash?, size?, mode? }], next_cursor? }`.
- `workspace-read-ref { root_hash, path }` → returns `{ kind, hash, size, mode }` or `null` when missing.
- `workspace-read-bytes { root_hash, path, range? }` → returns `{ data_b64 }`.
- `workspace-write-bytes { root_hash, path, bytes_b64, mode? }` → returns `{ new_root_hash, blob_hash }`.
- `workspace-remove { root_hash, path }` → returns `{ new_root_hash }`.
- `workspace-diff { root_a, root_b, prefix? }` → returns `{ changes:[{ path, kind, old_hash?, new_hash? }] }`.
- `workspace-annotations-get { root_hash, path? }` → returns `{ annotations?:{ key: hash } }`.
- `workspace-annotations-set { root_hash, path?, annotations_patch:{ key: hash|null } }` → returns `{ new_root_hash, annotations_hash }`.
- `blob-put { data_b64 }` → stores blob in CAS; returns `{ hash: "sha256:..." }`.
- `blob-get { hash }` → returns `{ data_b64 }` (CAS lookup).
- `snapshot {}` → forces snapshot; `result` is empty object.
- `shutdown {}` → graceful drain, snapshot, shutdown; server and daemon stop.
- `gov-propose { patch_b64, description? }` → submits a governance proposal. `patch_b64` is base64 of either (a) `ManifestPatch` CBOR or (b) `PatchDocument` JSON. PatchDocuments are validated against `spec/schemas/patch.schema.json` (with `common.schema.json` embedded) before compilation; ManifestPatch skips schema validation. Returns `{ proposal_id: <u64> }`.
- `gov-shadow { proposal_id }` → runs shadow for a proposal; returns a JSON `ShadowSummary` `{ manifest_hash, predicted_effects?, pending_receipts?, plan_results?, ledger_deltas? }`.
- `gov-approve { proposal_id, decision?, approver? }` → records an approval decision. `decision` is `"approve"` (default) or `"reject"`; `approver` defaults to `"control-client"`. Returns `{}`.
- `gov-apply { proposal_id }` → applies an approved proposal; returns `{}`.

Deferred verbs:
- Stdio/streaming uploads for `put-blob`.
- Journal/event streaming.

## Daemon Integration

- `WorldDaemon` owns `ControlServer`; shutdown via control propagates to daemon loop and server.
- Timers are partitioned in `RunMode::Daemon`; control requests run the daemon path (not batch).
- Socket reuse: CLI checks for a healthy control socket before starting a new daemon; refuses to overwrite a live or unhealthy socket.

## Client Expectations

- Clients must set `v=1` and a unique `id` per request; responses echo `id`.
- NDJSON framing only; one response per request (no streaming).
- Timeouts are client-side (default 5s in the helper); server does not push timeouts.

## Security Model

- Local-only socket; no authentication. Future: uid check via `SO_PEERCRED` (Unix) if needed.
- Socket perms tightened to owner (best-effort); users are responsible for directory permissions.

## Compatibility & Extensibility

- Backward/forward compatibility is gated by the `v` field; unknown versions should be rejected with `invalid_request`.
- New verbs should be additive; clients must treat unknown methods as recoverable errors.

## Known Limitations / TODOs

- Stdio framing and CBOR framing not implemented.
- Streaming blob upload (stdin/file) not implemented; current path is base64 inline.
- Journal tail/streaming pending.
