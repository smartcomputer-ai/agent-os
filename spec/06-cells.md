# Cells (Keyed Workflows)

Cells are the keyed-instance model for workflows in AgentOS. They let one workflow module manage many independent instances of the same state machine, where each instance is identified by a key such as `order_id`, `ticket_id`, or `note_id`.

This keeps orchestration logic centralized in one workflow definition while isolating runtime state, mailboxes, and receipt continuations per instance. A keyed workflow behaves like a family of small deterministic workflow instances that share code but not mutable state.

Use cells when the same business process repeats across many entities:
- each entity should have its own state and pending work
- events should route directly to the correct instance
- receipts should resume only the instance that emitted the originating effect
- scheduling should remain deterministic even when many instances are active

## Concepts
- **Workflow module (keyed)**: one workflow module whose state is partitioned by a key. Same `step` export for all keys.
- **Cell**: an instance of a keyed workflow module identified by `key` (bytes). Holds only that substate and its mailbox.
- **Workflow work unit**: scheduler interleaves ready cells and other queued workflow work deterministically.

## ABI (unchanged export, optional context)
- Export: `step(ptr,len) -> (ptr,len)` (canonical CBOR in/out).
- Input envelope: `{ version:1, state: bytes|null, event:{schema:Name, value:bytes, key?:bytes}, ctx?:bytes }`
  - When a module declares `sys/ReducerContext@1`, `ctx` carries `key` and `cell_mode`.
  - In cell mode, the module receives only this cell's state and `key` is required.
- Output envelope: `{ state:bytes|null, domain_events?:[…], effects?:[…], ann?:bytes }`
  - Returning `state=null` in cell mode deletes/GCs the cell.
- Effect authority follows workflow runtime rules:
  - only `module_kind: "workflow"` modules may emit effects
  - emitted kinds must be declared in `abi.workflow.effects_emitted`
  - cap/policy checks still apply after structural allowlist checks

## Manifest & AIR hooks
- `defmodule.key_schema` documents the key type when routed as keyed.
- `manifest.routing.subscriptions[].key_field` marks routed events whose value field contains the key to target a cell: `{ event, module, key_field }`.
- For variant event schemas, `key_field` typically points into the wrapped value (for example `$value.note_id`).
- Startup and domain ingress use `routing.subscriptions`.

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

## Storage, Head View, and Snapshots
- CAS stays immutable `{hash->bytes}`; no named refs.
- Physical backends may still pack many logical blobs into one immutable backing object; this does
  not change the logical CAS contract seen by cells or snapshots.
- Per keyed workflow module, kernel maintains a content-addressed `CellIndex`:
  `key_hash -> { key_bytes, state_hash, size, last_active_ns }`.
- World state/snapshots store only the root hash of each module's `CellIndex`. The root is the persisted base layer, not the full hot head state.

### Cell caching mechanics
- The live head view is layered:
  - base layer: snapshot-anchored `CellIndex` root for the workflow
  - hot cache: per-workflow in-memory `cell_cache` of recently used clean cells
  - delta layer: per-workflow in-memory dirty overrides (`upsert` or `delete`) that shadow both cache and base index
- Read path is `delta -> hot cache -> CellIndex/CAS`.
  - If a cell is loaded from the base index, the kernel inserts it into the hot cache.
  - `list_cells` and `workflow_state_bytes` expose this head view, not just the last snapshot state.
- Write path does not rewrite the `CellIndex` immediately.
  - Workflow output stages cell updates in the delta layer.
  - Deletes become delta tombstones.
  - The current `CellIndex` root remains unchanged until snapshot materialization.

### Cache sizing and spill behavior
- The hot cache is bounded by entry count and defaults to `4096` cells per workflow (`AOS_CELL_CACHE_SIZE` / kernel config).
- Dirty delta entries may keep state bytes resident in memory, but large or old resident entries spill to CAS while remaining logically dirty.
- Spill policy:
  - states `>= 1 MiB` spill to CAS immediately
  - total resident dirty bytes across workflows may grow to `256 MiB`
  - once over that limit, least-recently-accessed resident dirty entries spill until usage drops to `192 MiB`
- Spilling changes only storage residency:
  - logical head state stays in the delta layer
  - reads still see the newest cell state via `state_hash` reload from CAS
  - `CellIndex` roots still do not change until snapshot

### Snapshot materialization
- Snapshot creation requires runtime quiescence, then materializes all pending cell deltas into each workflow's `CellIndex`.
- During materialization:
  - resident dirty states are written to CAS if needed
  - `CellIndex` entries are upserted/deleted
  - a new per-workflow root hash is produced
  - flushed clean entries are promoted back into the hot cache
  - the dirty delta layer is cleared
- Snapshots persist the resulting `cell_index_root` values; the in-memory caches are derived runtime state.
- Replay restores the roots and repopulates caches lazily as cells are read or replayed.
- GC walks from snapshot-pinned roots; no side-channel CAS refs act as roots.

## Journal & Observability
- Journal entries for cell-scoped delivery include module identity plus key correlation for domain and receipt records.
- CLI/inspect supports listing cells, showing a cell state, tailing events, and exporting a single-cell snapshot.
- Trace/diagnose can render per-cell timelines and correlate receipt continuations via intent identity.
