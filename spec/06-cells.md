# Cells (Keyed Workflows)


## Concepts
- **Workflow module (keyed)**: one workflow module whose state is partitioned by a key. Same `step` export for all keys.
- **Cell**: an instance of a keyed workflow module identified by `key` (bytes). Holds only that substate and its mailbox.
- **Workflow work unit**: scheduler interleaves ready cells and other queued workflow work deterministically.

## ABI (unchanged export, optional context)
- Export: `step(ptr,len) -> (ptr,len)` (canonical CBOR in/out).
- Input envelope: `{ version:1, state: bytes|null, event:{schema:Name, value:bytes, key?:bytes}, ctx?:bytes }`
  - When a module declares `sys/ReducerContext@1` (legacy name retained), `ctx` carries `key` and `cell_mode`.
  - `cell_mode=false` (compatibility mode): module receives whole state (often `map<key, substate>`); `key` is advisory.
  - `cell_mode=true` (cells): module receives only this cell's state; `key` is required.
- Output envelope: `{ state:bytes|null, domain_events?:[…], effects?:[…], ann?:bytes }`
  - Returning `state=null` in cell mode deletes/GCs the cell.
- Effect authority follows workflow runtime rules:
  - only `module_kind: "workflow"` modules may emit effects
  - emitted kinds must be declared in `abi.reducer.effects_emitted`
  - cap/policy checks still apply after structural allowlist checks

## Manifest & AIR hooks
- `defmodule.key_schema` documents the key type when routed as keyed.
- `manifest.routing.subscriptions[].key_field` marks routed events whose value field contains the key to target a cell: `{ event, module, key_field }`.
- For variant event schemas, `key_field` typically points into the wrapped value (for example `$value.note_id`).
- Startup and domain ingress use `routing.subscriptions`; plan triggers are not part of the active model.

### Future: explicit key override (v1.1+ option)

If we add an explicit key override, it should be strictly typed and unambiguous:
- allowed only when all routing targets for the event schema share the same `key_schema`
- provided key must typecheck to `key_schema` and match any key derived from `key_field` when present
- payload-derived key remains source of truth unless an explicit key-only route mode is introduced

## Routing, Mailboxes, Scheduling
- On domain ingress, kernel extracts `key = event.value[key_field]` (validated against `key_schema`) and targets `(module, key)`.
- For variant payloads, canonical wrapper is `{"$tag":"...", "$value": ...}` so the key path is usually `$value.<field>`.
- If the cell is missing, kernel calls the workflow module with `state=null` (creation). `state=null` on output deletes it.
- Each cell has its own mailbox for DomainEvents and ReceiptEvents; delivery appends to the journal and marks the cell ready.
- Scheduler uses fair round-robin across ready cells and queued workflow work, preserving determinism.

## Receipt Continuation for Keyed Cells
- Receipt continuation routing is manifest-independent and keyed by recorded origin identity (`origin_module_id`, `origin_instance_key`, `intent_id`).
- For keyed workflows, `origin_instance_key` maps directly to the target cell.
- `routing.subscriptions` and `key_field` are domain-ingress routing only; they are not used for receipt continuation.

## Storage and Snapshots (CAS-backed index)
- CAS stays immutable `{hash->bytes}`; no named refs.
- Per keyed workflow module, kernel maintains a content-addressed `CellIndex`:
  `key_hash -> { key_bytes, state_hash, size, last_active_ns }`.
- World state stores only the root hash of each module's cell index.
- Cell state is stored as CAS blobs; load/save/delete go through the index and emit a new root.
- Snapshots persist per-module `cell_index_root`; replay restores roots and uses the index for keyed loads.
- Future GC walks from snapshot-pinned roots; no side-channel CAS refs act as roots.

## Journal & Observability
- Journal entries for cell-scoped delivery include module identity plus key correlation for domain and receipt records.
- CLI/inspect supports listing cells, showing a cell state, tailing events, and exporting a single-cell snapshot.
- Trace/diagnose can render per-cell timelines and correlate receipt continuations via intent identity.
