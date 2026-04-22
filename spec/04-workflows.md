# Workflows

Workflows are the orchestration unit in AgentOS. A workflow op is a deterministic state machine that
consumes canonical events, updates state, emits domain events, and requests declared effect ops
through the kernel.

This document describes the active workflow runtime contract: what a workflow owns, how it is
invoked, how effects and receipts move through the system, and which invariants make replay, audit,
and governance reliable.

## 1) Scope

Workflow orchestration is code-defined and event-driven:

- `defop` with `op_kind = "workflow"` is the orchestration/state-machine unit.
- `defmodule` supplies the runtime/artifact used by the workflow op implementation.
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

Workflow ops own:

- domain state
- business invariants
- transition logic
- retry and compensation decisions

Kernel + execution runtime own:

- deterministic stepping
- declared-effect admission
- effect emission and open-work tracking
- continuation admission and receipt ingestion

Executors/adapters own:

- side-effect execution
- non-authoritative progress reporting
- signed receipt production

This split keeps orchestration logic in workflow code while preserving a small deterministic runtime.
The workflow decides what should happen, the kernel decides whether and when it is admitted, and
executors perform external work and return auditable continuations.

The owner/executor seam for open external work is defined in [spec/05-effects.md](05-effects.md).

## 3) Normative Runtime Contract

### 3.1 Effect emission

1. Only workflow ops may originate workflow-emitted effects.
2. Workflow ops must declare `workflow.effects_emitted`.
3. Emitted effects must name effect ops, not semantic effect strings.
4. Kernel rejects undeclared effect ops before enqueue.
5. Multiple effects per step are allowed; deterministic kernel output limits apply.

### 3.2 Deterministic canonicalization

1. Event payloads are schema-validated and canonicalized on ingress.
2. Effect params are schema-validated and canonicalized before intent hashing/enqueue.
3. Receipt payloads are schema-validated/canonicalized before continuation delivery.
4. Journal + snapshot persist canonical CBOR forms used for replay.
5. Runtime decode fallbacks for non-canonical event/receipt payload shapes are not part of the
   active contract.

### 3.3 Continuation routing contract

Receipt continuation routing is keyed by recorded origin identity:

- origin workflow op
- origin workflow op hash when available
- origin instance key
- intent hash identity

Intent identity binds origin instance identity to avoid ambiguous concurrent wakeups. Continuation
routing is manifest-independent. `routing.subscriptions` is for domain-event ingress only.

### 3.4 Receipt envelope contract

Settled effects produce a generic workflow receipt envelope (`sys/EffectReceiptEnvelope@1`) with at
least:

- origin workflow op identity
- origin instance key when keyed
- intent identity
- effect op identity
- executor module/entrypoint identity when resolved
- optional issuer reference echoed from the emitted effect
- receipt payload bytes
- receipt status
- emitted sequence metadata

### 3.5 Receipt fault handling

If receipt payload decoding/normalization fails:

1. The failing intent is settled and removed from pending.
2. If the workflow event schema supports `sys/EffectReceiptRejected@1`, the kernel emits it.
3. If not supported, the kernel marks the workflow instance failed and drops remaining pending
   receipts for that instance.

### 3.6 Persisted workflow instance model

Kernel persists workflow instance runtime state, conceptually including:

- state bytes
- inflight intent set/map
- lifecycle status: `running | waiting | completed | failed`
- last processed sequence marker
- workflow op/module version metadata for diagnostics

Replay must restore this state deterministically.

### 3.7 Apply safety

Manifest apply is blocked when any of the following hold:

1. non-terminal workflow instances exist
2. any workflow has inflight intents
3. effect queue/scheduler still has pending work

No implicit abandonment or clearing of in-flight workflow state occurs during apply.

### 3.8 Governance and shadow semantics

Shadow/governance reporting is bounded to the observed execution horizon:

- observed effects so far
- pending workflow receipts/intents
- workflow instance statuses
- workflow op effect allowlists
- relevant state deltas

Shadow does not promise complete static future-effect prediction for unexecuted branches.

## 4) Runtime Flow

1. Domain event is appended and canonicalized.
2. Router evaluates `routing.subscriptions` and delivers to matching workflow ops.
3. Workflow entrypoint runs deterministically with current state + event.
4. Workflow returns new state, domain events, and effect intents.
5. Kernel enforces `workflow.effects_emitted`, validates effect op params, then records open work.
6. The unified node publishes opened async effects only after durable frame flush.
7. Executors emit stream frames and terminal receipts.
8. Kernel canonicalizes admitted continuations and routes them to the recorded origin instance.

## 5) Workflow Op Contract

Workflow ops declare:

- `workflow.state`: state schema
- `workflow.event`: event schema
- `workflow.context`: optional context schema
- `workflow.annotations`: optional annotation schema
- `workflow.key_schema`: optional key schema for cells
- `workflow.effects_emitted`: required list of effect op names
- `impl.module` and `impl.entrypoint`: runtime implementation target

`sys/WorkflowContext@1` includes deterministic time/entropy, journal metadata, manifest hash,
workflow op identity, optional workflow op hash, optional key, and `cell_mode`.

## 6) Routing Contract

`routing.subscriptions` maps event schema to workflow op:

- required fields are `event` and `op`
- `key_field` is used for keyed workflow delivery
- deterministic evaluation order is manifest order
- matching subscriptions fan out in order

A subscription is deliverable when its event schema exactly equals the target workflow op's
`workflow.event`, or when the workflow event schema is a variant whose arm references the
subscription event schema. In the variant-arm case, runtime delivery wraps the incoming event as that
variant arm before invoking the workflow.

Continuation delivery from receipts does not use this routing table.

## 7) Keyed Workflows (Cells)

Cells are the keyed-instance model for workflows. They let one workflow op manage many independent
instances of the same state machine, where each instance is identified by a key such as `order_id`,
`ticket_id`, or `note_id`.

Use cells when:

- each entity should have isolated state and pending work
- events should route directly to the correct instance
- receipts should resume only the instance that emitted the originating effect
- scheduling should remain deterministic across many active instances

### 7.1 Concepts

- **Workflow op (keyed)**: one workflow op whose state is partitioned by key.
- **Cell**: an instance of a keyed workflow op identified by key bytes.
- **Workflow work unit**: scheduler unit for ready cells and queued workflow work.

### 7.2 ABI and context

Keyed workflows use the same canonical CBOR envelopes as unkeyed workflows:

- Input: `{ version:1, state: bytes|null, event:{schema:Name, value:bytes, key?:bytes}, ctx?:bytes }`
- Output: `{ state:bytes|null, domain_events?:[...], effects?:[...], ann?:bytes }`

When a workflow declares `sys/WorkflowContext@1`, `ctx` carries `key` and `cell_mode`. In cell mode,
the workflow receives only that cell's state and `key` is required. Returning `state = null` in cell
mode deletes the cell.

Effect authority is structural:

- only workflow ops may emit effects
- emitted effect ops must be declared in `workflow.effects_emitted`
- the effect op must be present in the active manifest

### 7.3 Manifest hooks and routing

`workflow.key_schema` documents the key type for a keyed workflow op.
`manifest.routing.subscriptions[].key_field` marks routed events whose value field contains the key
to target a cell:

```json
{
  "event": "com.acme/OrderEvent@1",
  "op": "com.acme/order.step@1",
  "key_field": "order_id"
}
```

For variant event schemas, `key_field` typically points into the wrapped value, for example
`$value.note_id`.

On domain ingress, the kernel extracts the key from the event value, validates it against
`workflow.key_schema`, and targets `(workflow_op, key)`. If the cell is missing, the kernel invokes
the workflow with `state = null` so the workflow can create the instance.

### 7.4 Mailboxes, scheduling, and receipt continuation

Each cell has its own mailbox for domain events and receipt events. Delivery appends to the journal
and marks the cell ready. The scheduler uses deterministic fair round-robin across ready cells and
other queued workflow work.

Receipt continuation routing is manifest-independent and keyed by recorded origin identity:

- origin workflow op
- origin instance key
- intent hash identity

For keyed workflows, `origin_instance_key` maps directly to the target cell. This prevents receipt
cross-delivery between concurrent instances of the same workflow op.

### 7.5 Storage, head view, and snapshots

CAS stays immutable as logical `hash -> bytes`. Physical backends may pack many logical blobs into
one immutable backing object, but that does not change the logical CAS contract.

Per keyed workflow op, the kernel maintains a content-addressed `CellIndex`:

```text
key_hash -> { key_bytes, state_hash, size, last_active_ns }
```

The live head view is layered:

- base layer: snapshot-anchored `CellIndex` root
- hot cache: recently used clean cells
- delta layer: dirty overrides (`upsert` or `delete`)

Reads use `delta -> hot cache -> CellIndex/CAS`. Writes stage cell updates in the delta layer until
snapshot materialization.

### 7.6 Cache sizing and spill behavior

The hot cache is bounded by entry count and defaults to `4096` cells per workflow
(`AOS_CELL_CACHE_SIZE` / kernel config).

Dirty delta entries may keep state bytes resident in memory, but large or old resident entries spill
to CAS while remaining logically dirty. Spilling changes only storage residency; logical head state
stays in the delta layer.

### 7.7 Snapshot materialization

Snapshot creation requires runtime quiescence, then materializes pending cell deltas into each
workflow's `CellIndex`.

During materialization:

- resident dirty states are written to CAS if needed
- `CellIndex` entries are upserted/deleted
- new per-workflow root hashes are produced
- flushed clean entries are promoted back into hot cache
- dirty delta layers are cleared

Snapshots persist the resulting `cell_index_root` values. In-memory caches are derived runtime
state. Replay restores roots and repopulates caches lazily.

GC walks from snapshot-pinned roots. No side-channel CAS refs act as roots.

### 7.8 Journal and observability

Journal entries for cell-scoped delivery include workflow op identity plus key correlation for
domain and receipt records. CLI/inspect supports listing cells, showing cell state, tailing events,
and tracing per-cell timelines. Trace/diagnose correlates receipt continuations via intent identity.

## 8) Conceptual Patterns

### Pattern A: Single workflow state machine

Best when business transitions, retries, and compensations are tightly coupled.

### Pattern B: Multi-op choreography

Best when contexts or teams are split; workflow ops communicate through domain events.

### Pattern C: Timer + receipt driven progression

Best for deadlines, backoff, and long-running lifecycle checkpoints.

## 9) Minimal Examples

### 9.1 Workflow transition sketch

```rust
enum Pc { Idle, AwaitingCharge, Done, Failed }

match (state.pc, event) {
    (Pc::Idle, Event::OrderCreated { order_id, amount_cents }) => {
        state.order_id = order_id;
        state.pc = Pc::AwaitingCharge;
        effects.push(emit("payment/charge@1", params, Some("payments")));
    }
    (Pc::AwaitingCharge, Event::EffectReceiptEnvelope { status, .. }) => {
        state.pc = if status == "ok" { Pc::Done } else { Pc::Failed };
    }
    _ => {}
}
```

### 9.2 Manifest routing sketch

```json
{
  "routing": {
    "subscriptions": [
      {
        "event": "com.acme/OrderEvent@1",
        "op": "com.acme/order.step@1",
        "key_field": "order_id"
      }
    ]
  }
}
```

## 10) Reliability Checklist

1. Include stable correlation fields in events and effect params.
2. Use explicit idempotency keys for externally visible effects.
3. Treat all continuation payloads as schema-bound inputs.
4. Keep terminal states and duplicate fences in workflow state.
5. Model retries with explicit attempt/backoff state.

## 11) Testing Checklist

1. Transition tests: `(state,event) -> (state,events,effects)`.
2. Receipt progression tests for `ok/error/timeout/fault` paths.
3. Replay-or-die snapshot equivalence tests.
4. Concurrency tests: no cross-delivery between keyed instances.
5. Apply-safety tests: strict-quiescence block/unblock behavior.
