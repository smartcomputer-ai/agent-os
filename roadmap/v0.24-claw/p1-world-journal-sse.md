# P1: Single-World Journal SSE Stream

**Priority**: P1
**Effort**: Medium
**Risk if deferred**: High (the chat TUI and other live observers will keep polling hot read APIs, which adds latency, load, and inconsistent cursor semantics)
**Status**: Complete
**Depends on**: `roadmap/v0.24-claw/vision.md`

## Goal

Add a generic, durable, single-world observation API that lets clients wait for or stream world journal progress in near real time.

This is the backend primitive needed by the first terminal chat UI, but it must not be agent-specific. Agent session UIs can derive progress by observing journal records and then reading world state, trace entries, blobs, or session-specific state through existing control APIs.

The P1 slice adds:

- A durable single-world long-poll endpoint.
- A durable single-world Server-Sent Events endpoint.
- World-sequence cursor semantics shared by both APIs.
- Worker-side wakeups after durable journal commit.

The P1 slice does not include multi-world fan-in streams.

## Current Fit

The control surface already exposes hot and journal-oriented reads:

- `GET /v1/worlds/{world_id}/runtime`
- `GET /v1/worlds/{world_id}/journal/head`
- `GET /v1/worlds/{world_id}/journal`
- `GET /v1/worlds/{world_id}/state`
- `GET /v1/worlds/{world_id}/trace`
- `POST /v1/worlds/{world_id}/events`
- `POST /v1/worlds/{world_id}/receipts`

The missing piece is a way for a client to wait until the world advances without polling these routes.

The existing model already has the right durable cursor material:

- `WorldLogFrame` has `world_seq_start`, `world_seq_end`, and journal `records`.
- `JournalBackend::world_tail_frames(world_id, after_world_seq, cursor)` can catch up from durable storage.
- `WorldRuntimeInfo.notify_counter` already represents the next world sequence from the hot world.
- Worker flush finalization is the right place to publish observation wakeups because opened async effects are only dispatched after durable append.

The agent workflow already records useful progress through normal world activity:

- domain events,
- emitted effects,
- stream frames,
- receipts,
- run trace entries,
- lifecycle/status state.

That means the backend should expose a world journal observation primitive, not an `aos-agent` session stream.

## Design Stance

1. Observe the durable world journal.

   The stream is backed by committed journal frames. A wakeup only tells the server that a world probably advanced; the stream still catches up by reading durable journal frames.

2. Use SSE for the live API.

   SSE is a standard HTTP response stream with event names, ids, reconnect behavior, and keepalive comments. It fits a one-way observation channel. Writes remain normal `POST /events` or `POST /receipts`.

3. Keep long-poll as a first-class companion.

   The long-poll endpoint is useful for tests, simple clients, and environments that do not want to hold an SSE stream open.

4. Expose world sequence cursors only.

   Public clients use world journal sequence numbers. They do not see SQLite row ids, Kafka offsets, backend cursors, or partition metadata.

5. Make `from` the next sequence to read.

   If a client has processed records through sequence `124`, its next request uses `from=125`.

6. Treat broadcast delivery as lossy.

   The observer hub may drop wakeups under load. This must not lose data because clients always catch up from durable journal frames.

7. Scope P1 to one world.

   Multi-world fan-in, cross-world ordering, and fleet observation belong in a later design.

## API Contract

### Long-Poll

```http
GET /v1/worlds/{world_id}/journal/wait?from=<seq>&timeout_ms=<ms>&limit=<n>&kind=<kind>
```

Query parameters:

- `from`: optional next world sequence to read. If omitted, the server starts at the current durable head and waits for future records.
- `timeout_ms`: optional wait timeout. Default `30000`. The server may cap this.
- `limit`: optional maximum number of returned journal records. Default `100`. The server may cap this.
- `kind`: optional repeated journal record kind filter. If omitted, all record kinds are eligible.

Response:

```json
{
  "world_id": "01HX...",
  "from": 12,
  "retained_from": 0,
  "head": 15,
  "next_from": 15,
  "timed_out": false,
  "gap": false,
  "entries": [
    {
      "seq": 12,
      "kind": "effect_intent",
      "record": {}
    }
  ]
}
```

Semantics:

- `head` is the current next world sequence after the server checks durable storage.
- `next_from` is the next sequence the client should request.
- `timed_out=true` with `entries=[]` means no matching entries were observed before the timeout.
- If `from` is older than retained journal history, the server must not silently hide it. It returns `gap=true`, includes `retained_from`, and starts from `retained_from`.
- If `kind` filters omit records, `next_from` still advances over scanned durable records so clients do not spin on filtered entries.

### SSE Stream

```http
GET /v1/worlds/{world_id}/journal/stream?from=<seq>&kind=<kind>
```

Headers:

```http
Content-Type: text/event-stream
Cache-Control: no-cache
```

Cursor resolution:

1. Use explicit `from` if present.
2. Otherwise, if `Last-Event-ID` is present, use `Last-Event-ID + 1`.
3. Otherwise, start at the current durable head and stream future records.

Event types:

```text
event: journal_record
id: 124
data: {"world_id":"01HX...","seq":124,"kind":"effect_receipt","next_from":125,"record":{}}
```

```text
event: world_head
id: 129
data: {"world_id":"01HX...","head":130,"next_from":130,"retained_from":0}
```

```text
event: gap
data: {"world_id":"01HX...","requested_from":4,"retained_from":20,"next_from":20}
```

Keepalive:

```text
: keepalive
```

Semantics:

- `journal_record` is emitted for each matching journal record.
- `id` on `journal_record` is the journal record sequence.
- `world_head` may be emitted after a catch-up batch or heartbeat. When `kind` filters hide records, `world_head` advances the SSE reconnect id over records the client chose not to receive.
- `gap` is emitted when the requested cursor is older than retained history. The stream continues from `retained_from`.
- Keepalive comments have no semantic data and must not advance the cursor.
- A reconnecting client can use the browser/EventSource `Last-Event-ID` behavior or explicitly pass `from`.

## Backend Design

Add a worker-owned observer hub, conceptually:

```rust
struct WorldAdvanced {
    universe_id: UniverseId,
    world_id: WorldId,
    world_epoch: u64,
    next_world_seq: u64,
}
```

The hub should use a lossy wakeup mechanism such as `tokio::sync::broadcast`. It is not the source of truth. It only wakes waiters and streams so they can re-check durable journal frames.

Notification point:

- Notify after the journal frame commit succeeds.
- Use the committed world's next sequence as `next_world_seq`.
- Do not notify from pre-commit hot state.
- Do not dispatch opened async effects before the durable commit contract that already exists today.

Route behavior:

1. Resolve `from`.
2. Read durable tail frames from `from`.
3. Flatten frames into per-record entries with world sequence numbers.
4. Apply optional kind filters.
5. Return or emit entries.
6. If there are no entries, subscribe to the observer hub and wait for either a matching world wakeup, timeout, client disconnect, or keepalive interval.
7. On wakeup, repeat from durable storage.

The existing synchronous control backend should not be stretched into a blocking streaming abstraction. Prefer a small observation facade or route state extension that can access:

- the journal backend,
- durable world head metadata,
- the observer hub,
- control auth/context if needed later.

## Scope

- Add shared response/query types for world journal waiting and streaming.
- Add a worker-side single-world observer hub.
- Publish observer wakeups from durable flush finalization.
- Add `GET /journal/wait`.
- Add `GET /journal/stream`.
- Add tests for cursor handling, gap handling, kind filtering, long-poll wakeups, and SSE reconnect ids.
- Document the client contract for the future `aos-cli` chat TUI.

## Non-Goals

- Multi-world streams.
- WebSocket support.
- Agent-specific session event routes.
- Bidirectional stream writes.
- Backend cursor exposure.
- Binary CBOR SSE payloads.
- Durable subscription state stored in the world.

## Test Plan

Unit tests:

- Flatten `WorldLogFrame` ranges into per-record entries with correct sequence numbers.
- Advance `next_from` over filtered records.
- Resolve cursor from `from`, `Last-Event-ID`, and current head.
- Report retained-history gaps without hiding them.

HTTP/control tests:

- `journal/wait` returns immediately when durable records already exist.
- `journal/wait` blocks until a submitted event is durably flushed.
- `journal/wait` times out cleanly with the current head.
- `journal/stream` emits catch-up records from `from`.
- `journal/stream` emits reconnect-safe ids.
- `journal/stream` keeps working if a broadcast wakeup is missed and a later wakeup arrives.

Integration smoke:

- Start a local node.
- Submit a normal world event.
- Observe it through SSE.
- Submit an `aos.agent/SessionInput@1` message.
- Observe generic journal progress through SSE, then read agent session state through existing state APIs.

## Open Questions

- Whether `notify_counter` should be renamed or aliased as `next_world_seq` in public JSON to make cursor semantics clearer.
- Whether retained-history gaps should eventually become `409 Conflict` for strict clients. P1 should prefer `200` plus `gap=true` or a `gap` SSE event so simple live clients can continue.
