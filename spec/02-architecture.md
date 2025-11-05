# AgentOS + AIR: Architecture

This section describes the runtime components of AgentOS and how they work together. It focuses on the kernel, storage, execution, effects, and governance loops. AIR (the control‑plane IR) is referenced where it interfaces with the runtime; its forms and semantics are detailed in the next section.

## Runtime Model

- One world is the unit of computation and ownership.
- Each world runs a single‑threaded deterministic stepper over an append‑only event journal with periodic snapshots.
- All control‑plane changes (modules, plans, schemas, policies, capabilities) are expressed as AIR patches and validated by the kernel before use.
- Application logic runs inside sandboxed WASM modules (reducers and pure components).
- External I/O happens through explicit effects executed by adapters; every effect yields a signed receipt that is appended to the journal and used for replay.

## Components (High Level)

- Kernel (Stepper): deterministic event processor, applies journal entries, enforces policy, and drives plans.
- Journal: append‑only log of all events (proposals, approvals, plan application, effect intents, receipts, snapshots).
- Snapshotter: materializes state at intervals for fast restore; replay from journal remains authoritative.
- Store (CAS): content‑addressed object store for AIR nodes, WASM modules, blobs, receipts, and snapshots.
- AIR Loader/Validator: loads the manifest, validates forms, resolves references, and exposes a typed view to the kernel.
- Plan Engine: executes AIR plans deterministically (evaluates guards, emits effects, raises/awaits events), supports shadow runs.
- Capability Ledger: scoped capability tokens (type, scope, budget, expiry) bound to principals and plans.
- Policy Gate: declarative allow/deny/require‑approval rules over effects and plans; budgets settle on receipts.
- WASM Runtime: deterministic sandbox (Wasmtime profile) for reducers and pure components.
- Effect Manager: queues effect intents, dispatches to adapters, ingests receipts, enforces idempotency.
- Adapters: host‑side executors for effect kinds (HTTP, Blob/FS, Timer, LLM in v1) with signing.
- CLI/Tooling: world lifecycle commands, shadow/diff, approvals, module build/register, and inspection.
- Observability: provenance (“why graph”), journal tailing, receipt viewers, and minimal metrics.

## Kernel and Event Flow

- Deterministic Stepper
  - Processes exactly one event at a time per world.
  - Applies events to control‑plane state (AIR manifest, ledger, policies) and data‑plane state (reducer states) in a fixed order.
  - Produces derived events when appropriate (e.g., PolicyDecisionRecorded, PlanApplied, SnapshotCreated).

- Event Kinds (typical, v1)
  - ProposalSubmitted {patches, proposer}
  - ShadowRunCompleted {proposal_id, predicted_effects, diffs}
  - ApprovalRecorded {proposal_id, approver, decision}
  - PlanApplied {manifest_root, plan_id}
  - EffectQueued {intent_hash, kind, params_ref, cap_ref}
  - ReceiptAppended {intent_hash, receipt_ref, status}
  - PolicyDecisionRecorded {subject, decision, rationale_ref}
  - SnapshotCreated {height, snapshot_ref}

- Ordering and Fences
  - Receipts include the intent_hash and a logical height fence; late receipts after rollback are ignored.
  - Idempotency keys ensure at‑least‑once adapter retries do not duplicate state transitions.

## Storage and Snapshots

- Journal
  - Segment files (monotonic sequence numbers); events are length‑prefixed, canonical CBOR.
  - Validated on load; corrupt segments quarantined with clear diagnostics.

- Snapshots
  - Persist: control‑plane AIR state (manifest), reducer state bytes (canonical CBOR by declared schema), and pinned blob roots.
  - Created periodically or on demand; restore replays from last snapshot to head.

- Content‑Addressed Store (CAS)
  - Nodes (AIR terms, receipts) and blobs (WASM modules, large payloads) addressed by SHA‑256 of canonical encoding.
  - Deduplication across worlds is possible via shared backing stores; world manifests pin required roots.

## Control Plane Interfaces

- AIR Manifest (read‑only at runtime)
  - The authoritative catalog of defmodule, defplan, defschema, defcap, and defpolicy objects.
  - Updates only via approved ProposalSubmitted → PlanApplied events.

- AIR Loader/Validator
  - Parses text form (JSON/S‑expression) and produces canonical CBOR.
  - Validates references, shapes, capability declarations, and plan graphs.
  - Exposes a typed view for the kernel (do not deep dive here; see AIR section).

- Capability Ledger
  - Records grant/revoke events and current balances/budgets per capability token.
  - Capabilities are passed to plans/modules by handle; no ambient authority.

- Policy Gate
  - Evaluates allow/deny/require‑approval decisions for effects and plan application.
  - Budgets, rate limits, and approvals are enforced; decisions are logged as events.

## Triggers And Events

- Triggers (manifest)
  - Manifest contains `triggers`: mappings from DomainIntent event schemas to plan names, plus optional `correlate_by` keys.
  - When a reducer emits a DomainIntent event, the kernel appends it to the journal and starts the configured plan with that event as input.

- Communication pattern
  1) Reducer emits DomainIntent (e.g., `ChargeRequested`) as a domain event.
  2) Trigger starts a plan instance with the event as `@plan.input` and records correlation id if provided.
  3) Plan emits one or more effects under capabilities; Policy Gate evaluates allow/deny/approval; Effect Manager dispatches.
  4) Adapter executes the effect and appends a signed receipt; the plan `await_receipt` step resumes with the receipt value.
  5) Plan `raise_event` publishes a result DomainEvent (e.g., `PaymentResult`) to the target reducer; the reducer consumes it and advances its typestate.
  6) Optional: plans may `await_event` for subsequent reducer‑produced events to continue orchestration in one instance.

- Governance and observability
  - All external I/O crosses `emit_effect` and is policy/capability‑gated; receipts are signed and journaled.
  - State changes occur only via events → reducers; plans never mutate reducer state directly.
  - Correlation keys allow tying receipts/effects to domain entities in the “why graph”.

## Compute Layer

- WASM Runtime
  - Deterministic profile: no threads, no wall‑clock/time syscalls, stable float behavior, seeded RNG from journal.
  - Limited hostcalls for pure intrinsics (serialization, hashing) and foreign‑memory copy.

- Module Registry
  - Modules are content‑addressed WASM artifacts registered in the manifest with declared interfaces.
  - Types
    - Reducer: state machine reacting to events.
      - ABI: step(state_bytes, event_bytes) → (state_bytes, effects[], annotations)
    - Pure Component: pure function.
      - ABI: run(input_bytes) → output_bytes

- Keyed Reducers (Cells)
  - v1.1 adds first‑class "cells": many instances of the same reducer FSM keyed by an id (e.g., order_id).
  - Unified reducer ABI stays a single `step` export; the kernel provides an envelope with optional key and a `cell_mode` flag.
  - Routing uses `manifest.routing.events[].key_field`; storage keeps per‑cell state files; scheduler interleaves cells and plan runs.
  - See: spec/05-cells.md

- Router/Inbox
  - Deterministic routing hooks trigger reducers or plans based on event kinds and manifest routing tables.
  - Reducers emit effect intents (not side‑effects) which enter the outbox.

## Effects, Adapters, and Receipts

- Effect Manager
  - Maintains an outbox of effect intents; each intent is typed and references a capability handle.
  - Dispatches to adapters with idempotency keys and deadlines.
  - Retries with backoff for transient failures; final status captured in receipt.

- Adapters (v1)
  - HTTP: http.request(method, url, headers, body_ref) → receipt(status, headers, body_ref, timings)
  - Blob/FS: fs.blob.{put,get} → receipt(root_hash, size)
  - Timer: timer.set(at/after) → receipt(delivered_at)
  - LLM: llm.generate(model, params, input_ref) → receipt(output_ref, token_usage, cost, provider_id)
  - Each adapter signs receipts (ed25519/HMAC) including intent_hash, inputs/outputs hashes, timings, and cost.

- Receipt Handling
  - ReceiptAppended events advance plans and reducers waiting on effects.
  - Budgets decrement on receipt; policy re‑checks may occur if costs exceed thresholds.
  - Replay consumes recorded receipts to produce identical state without re‑executing effects.

## Constitutional Loop (Change Lifecycle)

1. Propose: submit AIR patches forming a proposal; kernel validates and records ProposalSubmitted.
2. Shadow: kernel clones state, runs a shadow simulation with stubbed receipts; records ShadowRunCompleted with diffs and predicted effects/costs.
3. Approve: humans or policy record ApprovalRecorded; may include capability grants/budgets.
4. Apply: kernel commits the new manifest root (PlanApplied); routing tables and capability bindings update atomically.
5. Execute: normal event flow resumes; new plans/modules are active under policy; effects produce receipts; audit trails accumulate.

## Shadow Runs

- Deterministic rehearsal of a proposal:
  - Uses a copy of the current state and manifest; effects are stubbed or use canned receipts.
  - Produces a typed diff of control‑plane and reducer states, predicted effect counts/costs, and required capabilities.
  - No changes persist until approval; outputs drive least‑privilege capability synthesis.

## Determinism and Safety

- Determinism
  - Canonical CBOR for all persisted values.
  - No access to time/randomness in modules; all nondeterminism isolated to the effect boundary and recorded via receipts.

- Safety
  - Capability scoping: tokens encode scope, expiry, and budgets; passed explicitly.
  - Policy gates: enforce allow/deny/approval and quotas before dispatch; budgets settle on receipt.
  - Rollback: move the head to a prior snapshot; receipts include fences to ignore late arrivals.

## Packaging and On‑Disk Layout (v1)

- world/
  - manifest.air.json (text) and manifest.air.cbor (canonical)
  - .store/{nodes, blobs}/sha256/<hash>
  - journal/{00001.log, 00002.log, …}
  - snapshots/snap-<ts>-<height>.cbor
  - modules/<name>@<ver>-<hash>.wasm
  - receipts/<height>-<intent-hash>.cbor

## Tooling and Dev Experience

- CLI
  - world init/info; propose/shadow/diff/approve/apply; run/tail; receipts ls/show; cap grant/revoke; policy set.
- SDK
  - Rust helpers for reducers and pure components; test harness for deterministic replay.
- Inspectors
  - Provenance (“why graph”) and plan visualizer (text‑first for v1).

## Scaling Model

- One world = one thread; scale out by running many worlds.
- Heavy/parallel work happens in adapters; receipts rejoin the single thread via events.
- Cross‑world coordination (deferred) can use conventional messaging with capability delegation.

## Failure Handling

- Adapter retries with exponential backoff; idempotency preserves correctness.
- Timeouts yield receipts with status=timeout; plans can gate on acceptable statuses.
- Dead‑letter policies for intents that exceed retry/cost limits; audit retains full trail.

## Security Posture

- Minimal trusted base: kernel, validator, and receipt verification code are small and testable.
- Supply chain: modules and manifests are content‑addressed; optional SBOM and signature checks at registration.
- Secrets: kept in capability tokens and adapter configuration; never embedded in WASM modules.

## Putting It Together (End‑to‑End)

- A plan registered in AIR wires modules and declares allowed effects and required capabilities.
- A proposal patches the manifest; shadow run quantifies diffs and costs; approval grants least‑privilege capabilities.
- Apply commits the manifest; execution proceeds; effects produce signed receipts; budgets decrement; snapshots capture state; replay reproduces it exactly.

This architecture yields a substrate where agents can co‑author and safely evolve systems: deterministic at the core, explicit and auditable at the edges, and unified by a small typed control plane.
