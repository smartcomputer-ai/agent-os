# P4: HTTP Streaming (SSE)

**Priority**: P4  
**Effort**: Medium  
**Risk if deferred**: Low (UI polish only)  
**Status**: Draft

## Goal

Expose a simple SSE endpoint for ephemeral progress updates that do not affect
world determinism, enabling local UI to follow journal tails and host runtime
activity.

## Non-Goals (v0.8)

- Durable, replayable streaming logs (use journal APIs instead).
- Arbitrary websocket protocols beyond SSE.
- Any reducer-side logic or state changes.

## Decision Summary

1) Add `GET /api/stream` using SSE.
2) `topics` query param selects `journal`, `effects`, and/or `plans`.
3) Journal events are derived from the kernel journal and include a cursor.
4) Effect/plan events are best-effort host runtime notifications (not journaled).
5) Clients reconnect and resume via cursor when needed.

## API Surface

`GET /api/stream?topics=journal,effects,plans&from=<cursor>`

- `topics`: comma-separated list; defaults to `journal` (`effects` emits
  `effect_intent`/`effect_receipt`, `plans` emits `plan_result`/`plan_ended`).
- `from`: optional journal cursor (sequence). If omitted, stream starts at head.
- SSE `event` types:
  - `journal`: data is a `JournalTailEntry` exactly as returned in
    `/api/journal.entries` (`{ kind, seq, record }`); SSE `id` is `seq`.
  - `effect_intent`: data matches the `record` object inside a
    `JournalTailEntry` with `kind = "intent"` (JSON shape of
    `EffectIntentRecord`).
  - `effect_receipt`: data matches the `record` object inside a
    `JournalTailEntry` with `kind = "receipt"` (JSON shape of
    `EffectReceiptRecord`).
  - `plan_result`: data matches `PlanResultRecord` JSON from the journal.
  - `plan_ended`: data matches `PlanEndedRecord` JSON from the journal.

## Implementation Notes

- Use `axum::response::sse` with JSON payloads per event.
- Journal topic should reuse `journal-list` control logic and tail-scan for new
  entries (poll or subscribe if a kernel hook exists).
- Effect/plan topics should be wired to in-process host events only; never write
  to the journal.
- The stream is best-effort; clients must tolerate missed events and reconnect.
- Define SSE framing (`event`, `id`, `data`), heartbeat cadence, and resume
  behavior (align cursor with `/api/journal`; consider `Last-Event-ID`).

## Tests

- SSE endpoint establishes and streams journal tail entries.
- `from` cursor resumes without duplication.
- Effect/plan events do not appear in the journal.
