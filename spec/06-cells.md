# Cells (Keyed Reducers)

Status: **implemented** (kernel/storage/control). Cells make many instances of the same reducer FSM first-class while preserving the unified reducer ABI.

## Concepts
- **Reducer (keyed)**: single reducer module whose state is partitioned by a key. Same `step` export for all keys.
- **Cell**: an instance of a keyed reducer identified by `key` (bytes). Holds only that substate and its mailbox.
- **Run**: plan instance. Scheduler interleaves ready runs and ready cells deterministically.

## ABI (unchanged export, optional context)
- Export: `step(ptr,len) -> (ptr,len)` (canonical CBOR in/out).
- Input envelope: `{ version:1, state: bytes|null, event:{schema:Name, value:bytes, key?:bytes}, ctx?:bytes }`
  - When a reducer declares `sys/ReducerContext@1`, `ctx` carries `key` and `cell_mode`.
  - `cell_mode=false` (v1 compatibility): reducer receives whole state (often a map<key,substate>); `key` is advisory.
  - `cell_mode=true` (cells): reducer receives only this cell's state; `key` is required.
- Output envelope: `{ state:bytes|null, domain_events?:[…], effects?:[…], ann?:bytes }`
  - Returning `state=null` in cell mode deletes/GCs the cell.
- ReducerEffect unchanged; reducers remain limited to micro-effects.

## Manifest & AIR hooks
- `defmodule.key_schema` documents the key type when routed as keyed.
- `manifest.routing.events[].key_field` marks routed events whose value field contains the key to target a cell: `{ event, reducer, key_field }`. For variant event schemas, `key_field` should typically point into the wrapped value (e.g., `$value.note_id`).
- Plan `raise_event` publishes bus events; keyed routing derives the target key via `key_field` on the routing entry.
- Triggers may set `correlate_by` so runs inherit a key for later `await_event` filters.

### Future: explicit key override (v1.1+ option)

If we add an explicit key override (e.g., envelope key or a future plan field), it should be **strictly typed and unambiguous**:

- Allowed only when **all routing targets for the event schema share the same `key_schema`**.
- The provided key must **typecheck to `key_schema`** and **match** any key derived from `key_field` when present.
- The payload‑derived key remains the source of truth unless an explicit “key‑only route” mode is introduced.

## Routing, Mailboxes, Scheduling
- On ingest, kernel extracts `key = event.value[key_field]` (validated against `key_schema`) and targets `(reducer,key)`. For variant payloads, the canonical wrapper is `{"$tag": "...", "$value": ...}` so the key path is usually `$value.<field>`.
- If the cell is missing, kernel calls reducer with `state=null` (creation). `state=null` on output deletes it.
- Each cell has its own mailbox for DomainEvents and ReceiptEvents; delivery appends to the journal and marks the cell ready.
- Scheduler uses fair round-robin across ready cells and plan runs: one step per tick, preserving determinism.

## Storage and Snapshots (CAS-backed index)
- CAS stays immutable `{hash→bytes}`; **no named refs**.
- Per keyed reducer, kernel maintains a content-addressed `CellIndex`: `key_hash → { key_bytes, state_hash, size, last_active_ns }`. The world state stores only the **root hash** of this index.
- Cell state is stored as CAS blobs; load/save/delete go through the index and emit a new root.
- Snapshots persist the per-reducer `cell_index_root`; replay restores roots and uses the index for keyed loads. Legacy snapshots without a root rebuild an empty index on load.
- Future GC walks from snapshot-pinned roots; no side-channel CAS refs act as roots.

## Journal & Observability
- Journal entries carry reducer + key for cell-scoped events:
  - `DomainEvent { reducer:Name, key_ref?:Hash, schema:Name, value_ref:Hash }`
  - `ReceiptDelivered { reducer:Name, key_ref?:Hash, intent_hash, receipt_ref }`
- CLI/inspect supports listing cells, showing a cell's state, tailing events, and exporting a single cell snapshot.
- Why-graph can render per-cell timelines and correlate receipts via intent_hash and correlate_by keys.

