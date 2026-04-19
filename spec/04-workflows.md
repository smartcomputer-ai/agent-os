# Workflows

Workflows are the orchestration unit in AgentOS. A workflow module is a deterministic state machine that consumes canonical events, updates its state, emits domain events, and requests external effects through the kernel's capability and policy gates.

This document describes the active workflow runtime contract: what a workflow owns, how it is invoked, how effects and receipts move through the system, and which invariants make replay, audit, and governance reliable.

## 1) Scope

Workflow orchestration is code-defined and event-driven:
- `defmodule` with `module_kind: "workflow"` is the orchestration/state-machine unit.
- `pure` modules are deterministic compute helpers and do not emit effects.
- Manifest startup and domain ingress wiring use `routing.subscriptions`.

In practice, a workflow owns the end-to-end progression of a business process:
- it receives a domain event or receipt continuation
- it decides the next state transition
- it emits follow-up domain events for other workflows or observers
- it requests side effects when external work is required
- it resumes when receipts arrive for previously emitted intents

Workflow instances may be unkeyed or keyed. Keyed workflows partition state by instance key and use
cells, described below.

## 2) Responsibility Split

Workflow modules own:
- domain state
- business invariants
- transition logic
- retry/compensation policy decisions

Kernel + execution runtime own:
- deterministic stepping
- capability checks
- policy checks
- effect emission and open-work tracking
- continuation admission and receipt ingestion

Executors/adapters own:
- side-effect execution
- non-authoritative progress reporting
- signed receipt production

This split keeps orchestration logic in workflow code while preserving a small deterministic runtime:
- workflow code decides what should happen
- the kernel decides whether and when it is allowed to happen
- executors perform the effect and return auditable continuations

The owner/executor seam for open external work is defined in
[spec/05-effects.md](05-effects.md).

## 3) Normative Runtime Contract

### 3.1 Authority and effect emission

1. Only workflow modules may originate module-emitted effects.
2. `pure` modules cannot emit effects.
3. Workflow modules must declare `abi.workflow.effects_emitted`.
4. Kernel rejects undeclared effect kinds before capability/policy evaluation.
5. Multiple effects per step are allowed; deterministic kernel output limits apply.

### 3.2 Deterministic canonicalization

1. Event payloads are schema-validated and canonicalized on ingress.
2. Effect params are schema-validated and canonicalized before intent hashing/enqueue.
3. Receipt payloads are schema-validated/canonicalized before continuation delivery.
4. Journal + snapshot persist canonical CBOR forms used for replay.
5. Runtime decode fallbacks for non-canonical event/receipt payload shapes are not part of the active contract.

### 3.3 Continuation routing contract

1. Receipt continuation routing is keyed by recorded origin identity:
- `origin_module_id`
- `origin_instance_key`
- `intent_id`/intent hash identity
2. Intent identity preimage includes origin instance identity to avoid ambiguous concurrent wakeups.
3. Continuation routing is manifest-independent.
4. `routing.subscriptions` is for domain-event ingress only.

### 3.4 Receipt envelope contract

Settled effects produce a generic workflow receipt envelope (schema family includes `sys/EffectReceiptEnvelope@1`) with at least:
- origin module identity
- origin instance key (if keyed)
- intent identity
- effect kind
- optional issuer reference echoed from the emitted effect
- receipt payload bytes
- receipt status
- emitted sequence metadata

### 3.5 Receipt fault handling

If receipt payload decoding/normalization fails:
1. The failing intent is settled (removed from pending).
2. If workflow event schema supports `sys/EffectReceiptRejected@1`, kernel emits it.
3. If not supported, kernel marks the workflow instance failed and drops remaining pending receipts for that instance (fault isolation, no global clogging).

### 3.6 Persisted workflow instance model

Kernel persists workflow instance runtime state (conceptually including):
- state bytes
- inflight intent set/map
- lifecycle status: `running|waiting|completed|failed`
- last processed sequence marker
- module version/hash metadata (for diagnostics)

Replay must restore this state deterministically.

### 3.7 Apply safety (strict quiescence)

Manifest apply is blocked when any of the following hold:
1. non-terminal workflow instances exist
2. any workflow has inflight intents
3. effect queue/scheduler still has pending work

No implicit abandonment/clearing of in-flight workflow state during apply.

### 3.8 Governance and shadow semantics

Shadow/governance reporting is bounded to observed execution horizon:
- observed effects so far
- pending workflow receipts/intents
- workflow instance statuses
- module effect allowlists
- relevant state/ledger deltas

No guarantee of complete static future-effect prediction for unexecuted branches.

## 4) Runtime Flow

1. Domain event is appended and canonicalized.
2. Router evaluates `routing.subscriptions` and delivers to matching workflow modules.
3. Workflow `step` runs deterministically with current state + event.
4. Workflow returns new state, domain events, and effect intents.
5. Kernel enforces `effects_emitted` allowlist, then caps and policy, and records open work.
6. Executors may run allowed external work independently and emit stream frames and a terminal
   receipt.
7. Kernel canonicalizes admitted continuations and routes them to the recorded origin instance.

## 5) Workflow Module Contract

Workflow modules declare workflow ABI fields under `abi.workflow`:
- `state`: state schema
- `event`: event schema
- `context` (optional)
- `effects_emitted` (required for effecting modules)
- `cap_slots` (optional slot -> cap type)

Manifest binds slots via `module_bindings`.

## 6) Routing Contract

`routing.subscriptions` maps event schema -> module:
- required fields are `event`, `module`; `key_field` is used for keyed module delivery
- deterministic evaluation order is manifest order
- matching subscriptions fan out in order

Continuation delivery from receipts does not use this routing table.

## 7) Keyed Workflows (Cells)

Cells are the keyed-instance model for workflows. They let one workflow module manage many
independent instances of the same state machine, where each instance is identified by a key such as
`order_id`, `ticket_id`, or `note_id`.

This keeps orchestration logic centralized in one workflow definition while isolating runtime state,
mailboxes, and receipt continuations per instance. A keyed workflow behaves like a family of small
deterministic workflow instances that share code but not mutable state.

Use cells when the same business process repeats across many entities:

- each entity should have its own state and pending work
- events should route directly to the correct instance
- receipts should resume only the instance that emitted the originating effect
- scheduling should remain deterministic even when many instances are active

### 7.1 Concepts

- **Workflow module (keyed)**: one workflow module whose state is partitioned by a key. Same
  `step` export for all keys.
- **Cell**: an instance of a keyed workflow module identified by `key` bytes. Holds only that
  substate and its mailbox.
- **Workflow work unit**: scheduler unit for ready cells and other queued workflow work.

### 7.2 ABI and Context

Keyed workflows use the same `step(ptr,len) -> (ptr,len)` export and canonical CBOR envelopes as
unkeyed workflows:

- Input envelope: `{ version:1, state: bytes|null, event:{schema:Name, value:bytes, key?:bytes}, ctx?:bytes }`
- Output envelope: `{ state:bytes|null, domain_events?:[...], effects?:[...], ann?:bytes }`

When a module declares `sys/WorkflowContext@1`, `ctx` carries `key` and `cell_mode`.
In cell mode, the module receives only that cell's state and `key` is required.
Returning `state=null` in cell mode deletes the cell.

Effect authority is unchanged:

- only `module_kind: "workflow"` modules may emit effects
- emitted kinds must be declared in `abi.workflow.effects_emitted`
- capability and policy checks still apply after structural allowlist checks

### 7.3 Manifest Hooks and Routing

`defmodule.key_schema` documents the key type when a workflow module is routed as keyed.
`manifest.routing.subscriptions[].key_field` marks routed events whose value field contains the key
to target a cell:

```json
{ "event": "com.acme/OrderEvent@1", "module": "com.acme/order_workflow@1", "key_field": "order_id" }
```

For variant event schemas, `key_field` typically points into the wrapped value, for example
`$value.note_id`.

On domain ingress, the kernel extracts `key = event.value[key_field]`, validates it against
`key_schema`, and targets `(module, key)`. If the cell is missing, the kernel calls the workflow
module with `state=null` so the workflow can create the instance.

`routing.subscriptions` and `key_field` are domain-ingress routing only. Receipt continuation
delivery does not use this routing table.

### 7.4 Mailboxes, Scheduling, and Receipt Continuation

Each cell has its own mailbox for domain events and receipt events. Delivery appends to the journal
and marks the cell ready. The scheduler uses deterministic fair round-robin across ready cells and
other queued workflow work.

Receipt continuation routing is manifest-independent and keyed by recorded origin identity:

- `origin_module_id`
- `origin_instance_key`
- `intent_id` / intent hash identity

For keyed workflows, `origin_instance_key` maps directly to the target cell. This is what prevents
receipt cross-delivery between concurrent instances of the same workflow module.

### 7.5 Storage, Head View, and Snapshots

CAS stays immutable as logical `hash -> bytes`. Physical backends may pack many logical blobs into
one immutable backing object, but that does not change the logical CAS contract seen by cells or
snapshots.

Per keyed workflow module, the kernel maintains a content-addressed `CellIndex`:

```text
key_hash -> { key_bytes, state_hash, size, last_active_ns }
```

World state and snapshots store only the root hash of each module's `CellIndex`. The root is the
persisted base layer, not the full hot head state.

The live head view is layered:

- base layer: snapshot-anchored `CellIndex` root for the workflow
- hot cache: per-workflow in-memory `cell_cache` of recently used clean cells
- delta layer: per-workflow in-memory dirty overrides (`upsert` or `delete`) that shadow both cache
  and base index

The read path is `delta -> hot cache -> CellIndex/CAS`. If a cell is loaded from the base index, the
kernel inserts it into the hot cache. `list_cells` and `workflow_state_bytes` expose this head view,
not just the last snapshot state.

The write path does not rewrite the `CellIndex` immediately. Workflow output stages cell updates in
the delta layer, and deletes become delta tombstones. The current `CellIndex` root remains unchanged
until snapshot materialization.

### 7.6 Cache Sizing and Spill Behavior

The hot cache is bounded by entry count and defaults to `4096` cells per workflow
(`AOS_CELL_CACHE_SIZE` / kernel config).

Dirty delta entries may keep state bytes resident in memory, but large or old resident entries spill
to CAS while remaining logically dirty. Spill policy:

- states `>= 1 MiB` spill to CAS immediately
- total resident dirty bytes across workflows may grow to `256 MiB`
- once over that limit, least-recently-accessed resident dirty entries spill until usage drops to
  `192 MiB`

Spilling changes only storage residency:

- logical head state stays in the delta layer
- reads still see the newest cell state via `state_hash` reload from CAS
- `CellIndex` roots still do not change until snapshot materialization

### 7.7 Snapshot Materialization

Snapshot creation requires runtime quiescence, then materializes all pending cell deltas into each
workflow's `CellIndex`.

During materialization:

- resident dirty states are written to CAS if needed
- `CellIndex` entries are upserted/deleted
- a new per-workflow root hash is produced
- flushed clean entries are promoted back into the hot cache
- the dirty delta layer is cleared

Snapshots persist the resulting `cell_index_root` values. In-memory caches are derived runtime
state. Replay restores roots and repopulates caches lazily as cells are read or replayed.

GC walks from snapshot-pinned roots. No side-channel CAS refs act as roots.

### 7.8 Journal and Observability

Journal entries for cell-scoped delivery include module identity plus key correlation for domain and
receipt records. CLI/inspect supports listing cells, showing a cell state, tailing events, and
exporting a single-cell snapshot. Trace/diagnose can render per-cell timelines and correlate receipt
continuations via intent identity.

## 8) Conceptual Patterns

### Pattern A: Single workflow state machine

Best when business transitions, retries, and compensations are tightly coupled.

### Pattern B: Multi-module choreography

Best when contexts/teams are split; modules communicate through domain events.

### Pattern C: Timer + receipt driven progression

Best for deadlines, backoff, and long-running lifecycle checkpoints.

## 9) Minimal Examples

### 9.1 Workflow transition sketch (Rust, conceptual)

```rust
enum Pc { Idle, AwaitingCharge, Done, Failed }

match (state.pc, event) {
    (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
        state.order_id = order_id;
        state.pc = Pc::AwaitingCharge;
        effects.push(emit("payment.charge", params, Some("payments")));
    }
    (Pc::AwaitingCharge, Event::EffectReceiptEnvelope { status, .. }) => {
        state.pc = if status == "ok" { Pc::Done } else { Pc::Failed };
    }
    _ => {}
}
```

### 9.2 Manifest routing + binding sketch

```json
{
  "routing": {
    "subscriptions": [
      {
        "event": "com.acme/OrderEvent@1",
        "module": "com.acme/order_workflow@1",
        "key_field": "order_id"
      }
    ]
  },
  "module_bindings": {
    "com.acme/order_workflow@1": {
      "slots": { "payments": "cap_payments" }
    }
  }
}
```

## 10) Reliability Checklist

1. Include stable correlation fields in events/effect params.
2. Use explicit idempotency keys for externally visible effects.
3. Treat all continuation payloads as schema-bound inputs.
4. Keep terminal states and duplicate fences in module state.
5. Model retries with explicit attempt/backoff state.

## 11) Testing Checklist

1. Transition tests: `(state,event)->(state,events,effects)`.
2. Receipt progression tests for `ok/error/timeout/fault` paths.
3. Replay-or-die snapshot equivalence tests.
4. Concurrency tests: no cross-delivery between keyed instances.
5. Apply-safety tests: strict-quiescence block/unblock behavior.
