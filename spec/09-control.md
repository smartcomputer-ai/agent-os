# Control Channel Specification (v1)

Status: **experimental, socket-only**. Workflow-era governance/trace verbs are live. Stdio framing, CBOR framing, and streaming remain deferred.

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

- `event-send { schema, value_b64, key_b64? }` -> enqueues a DomainEvent and runs one daemon cycle. `value_b64` must be canonical CBOR.
- `receipt-inject { intent_hash, adapter_id, payload_b64 }` -> injects a receipt payload for an existing intent (`status=ok` path; primarily for tests/debug).
- `manifest-get { consistency?: "head"|"exact:<h>"|"at_least:<h>" }` -> returns `{ manifest_b64, meta }`.
- `state-get { reducer, key_b64?, consistency?: "head"|"exact:<h>"|"at_least:<h>" }` -> returns `{ state_b64?, meta }`.
- `state-list { reducer }` -> returns `{ cells:[{ key_b64, state_hash_hex, size, last_active_ns }], meta }`.
- `def-get|defs-get { name }` -> returns `{ def, hash }`.
- `def-list|defs-list { kinds?, prefix? }` -> returns `{ defs:[...], meta }`.
- `journal-head {}` -> returns `{ meta }`.
- `journal-list { from?, limit?, kinds? }` -> returns `{ from, to, entries:[{ kind, seq, record }] }`.
- `trace-get { event_hash }` or `trace-get { schema, correlate_by, value, window_limit? }` -> returns root event, journal window, live wait diagnostics, terminal classification, and meta.
- `trace-summary {}` -> returns workflow-era totals, continuation snapshots, and strict-quiescence counters.
- `workspace-resolve { workspace, version? }` -> returns `{ exists, resolved_version?, head?, root_hash? }`.
- `workspace-empty-root { workspace }` -> returns `{ root_hash }`.
- `workspace-list { root_hash, path?, scope?, cursor?, limit }` -> returns `{ entries:[{ path, kind, hash?, size?, mode? }], next_cursor? }`.
- `workspace-read-ref { root_hash, path }` -> returns `{ kind, hash, size, mode }` or `null`.
- `workspace-read-bytes { root_hash, path }` -> returns `{ data_b64 }`.
- `workspace-write-bytes { root_hash, path, bytes_b64, mode? }` -> returns `{ new_root_hash, blob_hash }`.
- `workspace-remove { root_hash, path }` -> returns `{ new_root_hash }`.
- `workspace-diff { root_a, root_b, prefix? }` -> returns `{ changes:[{ path, kind, old_hash?, new_hash? }] }`.
- `workspace-annotations-get { root_hash, path? }` -> returns `{ annotations?:{ key: hash } }`.
- `workspace-annotations-set { root_hash, path?, annotations_patch:{ key: hash|null } }` -> returns `{ new_root_hash, annotations_hash }`.
- `blob-put { data_b64 }` -> stores blob in CAS; returns `{ hash }` (hex hash string).
- `blob-get { hash_hex }` -> returns `{ data_b64 }`.
- `snapshot {}` -> forces snapshot; returns `{}`.
- `shutdown {}` -> graceful drain, snapshot, shutdown.
- `gov-propose { patch_b64, description? }` -> submits proposal from ManifestPatch CBOR or PatchDocument JSON; returns `{ proposal_id }`.
- `gov-shadow { proposal_id }` -> returns bounded workflow-era `ShadowSummary` observations (observed horizon, not full static future prediction).
- `gov-approve { proposal_id, decision?: "approve"|"reject", approver? }` -> returns `{}`.
- `gov-apply { proposal_id }` -> applies approved proposal; returns `{}`. Apply may fail with `host_error` when strict-quiescence blockers exist.
- `gov-apply-direct { patch_b64 }` -> applies patch directly; returns `{ manifest_hash }`.
- `gov-list { status?: "pending"|"approved"|"applied"|"rejected"|"submitted"|"shadowed"|"all" }` -> returns `{ proposals, meta }`.
- `gov-get { proposal_id }` -> returns `{ proposal, meta }`.

`meta` shape is:
- `{ journal_height, snapshot_hash?, manifest_hash, active_baseline_height, active_baseline_receipt_horizon_height? }`

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
- Some payload field names remain legacy for compatibility.
- `state-get/state-list` still use `reducer` as the module identifier.
- Trace summaries currently expose `pending_reducer_receipts` counters even in workflow-era runtime output.
- Journal kind labels for historical plan entries are surfaced as `legacy_plan_started|legacy_plan_result|legacy_plan_ended`.

## Known Limitations / TODOs

- Stdio framing and CBOR framing not implemented.
- Streaming blob upload (stdin/file) not implemented; current path is base64 inline.
- Journal/event streaming pending.
