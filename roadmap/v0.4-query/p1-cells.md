# Cells (Keyed Reducers) — v1.1 Design

Cells make many parallel instances of the same reducer state machine first‑class. A cell is an instance of a keyed reducer identified by a key (e.g., order_id). The kernel stores state per cell, delivers events per cell, and schedules cells fairly alongside plan runs. This enables Temporal‑like concurrency while preserving determinism, auditability, and homoiconicity.

## Rationale

- Many instances: domains naturally need thousands of concurrent FSMs (orders, jobs, notifications). In v1, reducers can emulate this with a map key→substate inside one state blob; it works but becomes unwieldy (snapshot bloat, GC, listing, exports).
- Clean separation: keep business logic in reducers, orchestration in plans, but make reducer instances (cells) cheap and observable.
- Version pinning: runs and cells pin the manifest era at start; upgrades don’t perturb in‑flight instances.

v1 baseline (ships now)
- Reducers remain non‑keyed; authors store a map<key, substate> inside reducer state and route by key in code.
- Plans raise events back with a key field in the event value; triggers correlate_by that key string.

v1.1 (this spec)
- Keyed reducers become first‑class; the kernel stores per‑cell state, delivers events to (reducer, key), and manages mailboxes and GC.
- Unified reducer ABI stays a single exported `step`; cells are a kernel routing/storage concern (no second export).

## Concepts

- Reducer (keyed): a WASM reducer whose state is partitioned by key. One module implements the FSM for all keys.
- Cell: a specific instance of a keyed reducer, identified by `key`. Holds only that instance’s state and mailbox.
- Run: an instance of a plan (orchestration). Runs and cells interleave deterministically.

## Unified Reducer ABI (v1 and v1.1)

Reducers export a single function; the kernel passes an envelope that includes optional key and a mode flag. Authors can write per‑cell logic; an SDK wrapper adapts to both modes.

- Export: `step(ptr, len) -> (ptr, len)`
- Input (canonical CBOR): `{ version: 1, state: bytes|null, event: { schema: Name, value: bytes }, ctx: { key?: bytes, cell_mode: bool } }`
  - `ctx.cell_mode=false` (v1): `state` is the whole reducer state (often a map<key,substate>); `ctx.key` may be present as a hint.
  - `ctx.cell_mode=true` (v1.1): `state` is this cell’s substate (or null if first event); `ctx.key` must be present.
- Output (canonical CBOR): `{ state: bytes|null, domain_events?: [ { schema: Name, value: bytes } ], effects?: [ ReducerEffect ], ann?: bytes }`
  - In cell mode, returning `state=null` signals delete/GC this cell.
  - Effects from reducers remain micro‑effects only (e.g., blob.put, timer.set); external orchestration belongs to plans.

ReducerEffect shape: `{ kind: EffectKind, params: bytes (CBOR), cap_slot?: text }`

## AIR Changes (v1.1 addenda)

- defmodule (optional clarity): may declare `key_schema: SchemaRef` to document expected key shape. ABI remains a single `step`.
- manifest.routing.events: entries targeting a keyed reducer include a `key_field` telling the kernel where to find the key in the event value.
  - Non‑keyed: `{ event: SchemaRef, reducer: Name }`
  - Keyed: `{ event: SchemaRef, reducer: Name, key_field: "key" }`
- defplan StepRaiseEvent adds a `key: Expr` for keyed reducers:
  - `{ id, op: "raise_event", reducer: Name, key: Expr, event: Expr }`
- manifest.triggers (already present): a trigger can specify `correlate_by` (e.g., `"key"`) so runs inherit an id they can use for await_event filters and observability.

Note: v1 can already carry a `key` field in event values without kernel‑managed cells; v1.1 makes the key first‑class for routing/storage.

## Kernel Semantics

- Routing
  - For keyed routes, the kernel extracts `key = event.value[key_field]` and targets (reducer, key). If the cell doesn’t exist, it is created with `state=null` in the first call.
  - For non‑keyed routes, events target the reducer as in v1 (monolithic state).
- Scheduling
  - Maintain ready queues for cells and runs. On each tick, pick exactly one ready entity (fair round‑robin) and process a single step. Determinism holds because effects only occur via gated, journaled intents/receipts.
- Mailboxes
  - Each cell has a mailbox for DomainEvents and ReceiptEvents. Delivery appends to the world journal; the cell is marked ready.
- Deletion/GC
  - If a keyed reducer returns `state=null`, the kernel deletes the cell’s state file and index entry. Policy may also apply TTL/idle retention.
- Version pinning
  - Cells inherit the manifest hash at creation time (era). They continue under that era even if the world upgrades; new cells use the new era.

## Storage Layout and Snapshots (as implemented)

- CAS stays dumb (`hash → bytes`) and has **no names/refs**.  
- Per-key state is stored as CAS blobs; we track `{ state_hash, last_active_ns, size }` per cell (and a monolithic hash for non-keyed reducers).  
- CAS‑backed persistent `CellIndex` now exists per keyed reducer: `key_hash → { key_bytes, state_hash, size, last_active_ns }`. Updates create new nodes and return a new **root hash**. The kernel/world state stores only that root (Git-like ref), never inside CAS. Lookup = root → index → `state_hash` → `blob.get`; updates write a fresh root.  
- Snapshots embed the per-reducer **cell_index_root**; replay restores the root and the index is used for keyed loads. Legacy snapshots without a root are upgraded on load by rebuilding an empty index.  
- GC (future) will walk from snapshot-pinned index roots to reach all cell state blobs; no side-channel CAS refs are roots.  
- The earlier path-based layout is superseded by CAS blobs + `CellIndex`; no named blobs in CAS.

## CAS Semantics and Naming (design decision)

- Do **not** extend CAS with “named blobs” or Git-like refs; CAS is `{ hash → bytes }` only.  
- All refs live in **world state** (manifest/snapshots/kernel metadata). For cells, the only ref is the per-reducer `cell_index_root`.  
- Agent-visible named artifacts (code bundles, patches, prompts) use the **Query Catalog** reducer (`name → hash`, metadata) with payloads stored in CAS.  
- This keeps determinism/replay: refs are journaled and snapshot-pinned; CAS holds immutable content only, shared safely across worlds.


## Journal and Observability

- Journal entries carry reducer and key for cell‑scoped events:
  - `DomainEvent { reducer: Name, key_ref?: Hash, schema: Name, value_ref: Hash }`
  - `ReceiptDelivered { reducer: Name, key_ref?: Hash, intent_hash, receipt_ref }`
- Why‑graph surfaces per‑cell timelines and correlates receipts/effects via intent_hash and correlate_by keys.
- CLI/inspect supports: list cells, show cell state, tail cell events, export a single cell’s snapshot.

## Status & TODO

Done (in codebase)
- Keyed routing/ABI: manifests use `key_field`; plans support `raise_event.key`; triggers can `correlate_by`.
- Reducer calls carry `cell_mode` and key; `state=null` deletes the cell.
- CAS-backed `CellIndex` per keyed reducer, with only the root hash stored in kernel state/snapshots; keyed load/save/delete go through the index.
- Snapshots capture index roots; replay restores them. Legacy snapshots rebuild an empty index on load.
- Tests: CellIndex unit coverage; kernel unit tests for root updates, delete, and snapshot restore; integration test `keyed_reducer_integration_flow` (host) exercises routed keyed events, index iteration, snapshot+replay, and state retrieval.
- CAS naming decision: CAS remains `{hash→bytes}` only; agent-visible names stay in Query Catalog.
- Scheduler fairness: round-robin between plan and reducer queues.

Planned / TODO
- List/get cell helper surfaces for inspect/CLI built on `CellIndex::iter`.
- GC/TTL for cells driven from index roots.
- Observability: richer why-graph views per cell; exports.

## Plans and Cells

- Triggers start runs when a reducer emits a DomainIntent; the trigger’s `correlate_by` may copy the event key into run context for filtering.
- StepRaiseEvent (keyed): plans must supply the key to target the correct cell: `key: Expr`.
- StepAwaitEvent (optional): plans may await subsequent domain events; the kernel matches against the keyed mailbox using the run’s correlation (e.g., `event.key == @plan.input.key`).



## SDK Guidance (Authoring Reducers Once)

- Authors implement per‑cell logic; an SDK provides a wrapper that:
  - In v1 (cell_mode=false): decodes a monolithic map<key,substate>, extracts substate for ctx.key, applies logic, writes back.
  - In v1.1 (cell_mode=true): passes just the substate; writes/deletes the cell directly.
- This preserves a single reducer binary across v1 and v1.1.

## Security and Policy

- Capability slots/bindings unchanged. Reducer‑sourced effects continue to be limited to micro‑effects and are policy‑gated as before.
- Plans remain the only place for high‑risk external effects and human approvals.

## Migration Path (v1 → v1.1)

- Preconditions: events include a stable key field; reducer code already treats state as a map<key,substate>.
- Steps:
  - Add `key_field` to manifest routes; (optionally) add `key_schema` to defmodule.
  - Flip kernel to cell_mode=true for that reducer.
  - Run a one‑time migration tool that spills the monolithic map into per‑cell files and rebuilds the index.
- No reducer binary changes required if the SDK wrapper was used from the start.

## Examples

- Routing entry (keyed):
  - `{ "event": "com.acme/ChargeRequested@1", "reducer": "com.acme/OrderSM@2", "key_field": "key" }`
- Trigger with correlation:
  - `{ "event": "com.acme/ChargeRequested@1", "plan": "com.acme/charge_flow@3", "correlate_by": "key" }`
- Plan step targeting a cell:
  - `{ "id":"apply", "op":"raise_event", "reducer":"com.acme/OrderSM@2", "key": { "ref":"@plan.input.key" }, "event": { "record": { "order_id": { "ref":"@plan.input.order_id" } } } }`

## Non‑Goals (v1.1)

- Cross‑world cell routing (defer to a “colony” design).
- Query engine over cell state (list/index only; richer queries can mount snapshots externally).
- Multiple keys per reducer (support exactly one key_field per reducer; require migration to change it).

## Summary

Cells elevate reducer instances to first‑class objects: per‑key state, mailboxes, scheduling, storage, and observability, all while preserving a single, stable reducer ABI. Start with v1’s map‑in‑state; when scale demands, switch to v1.1 keyed reducers simply by enabling cell_mode in the kernel and adjusting routing—no binary churn, no semantic drift, and deterministic replay throughout.
