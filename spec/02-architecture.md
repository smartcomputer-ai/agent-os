# AgentOS Architecture

This section describes the runtime components of AgentOS and how they work together. It focuses on the kernel, storage, execution, effects, and governance loops. AIR (the control‑plane IR) is referenced where it interfaces with the runtime; its forms and semantics are detailed in the next section.

AgentOS operates in two conceptually distinct modes:
- **runtime**: business as usual; reducers react to events, plans orchestrate effects, receipts flow back
- **design time**: self‑modification—the system proposes, rehearses, and applies changes to its own control plane

These are not separate runtimes. Under the hood, there is one unified system: the same kernel, journal, event processing, and blob storage handle both modes. Design-time changes (proposals, shadow runs, approvals) and runtime operations (domain events, effects, receipts) all flow through the same event log and deterministic stepper. This unification is possible because AIR represents the control plane as data and is stored like any other state. The constitutional loop (propose → shadow → approve → apply) is simply the governed pathway for modifying that control-plane data, after which it becomes active runtime behavior.

## Runtime Model

One world is the unit of computation and ownership. Each world runs a single‑threaded deterministic stepper over an append‑only event journal with periodic snapshots. All control‑plane changes—modules, plans, schemas, policies, capabilities—are expressed as AIR patches and validated by the kernel before use.

Application logic runs inside sandboxed WASM modules (reducers and pure components). External I/O happens through explicit effects executed by adapters; every effect yields a signed receipt that is appended to the journal and used for replay. The kernel also stamps each ingress with deterministic time/entropy and journals those values; modules can opt in to receiving that call context as explicit input. This separation between pure computation and effectful I/O is what enables deterministic replay: the same journal and receipts always produce the same state.

## Components (High Level)

- Kernel (Stepper): deterministic event processor, applies journal entries, enforces policy, and drives plans.
- Kernel Clock: samples wall-clock + monotonic time at ingress and records deterministic timestamps.
- Journal: append‑only log of all events (proposals, approvals, plan application, effect intents, receipts, snapshots).
- Snapshotter: materializes state at intervals for fast restore; replay from journal remains authoritative.
- Store (CAS): content‑addressed object store for AIR nodes, WASM modules, blobs, receipts, and snapshots.
- Workspace Registry: built-in `sys/Workspace@1` reducer and CAS-backed tree nodes for versioned source/artifact trees; exposed via internal `workspace.*` effects (cap-gated).
- AIR Loader/Validator: loads the manifest, validates forms, resolves references, and exposes a typed view to the kernel.
- Plan Engine: executes AIR plans deterministically (evaluates guards, emits effects, raises/awaits events), supports shadow runs.
- Capability Resolver: scoped capability grants (type, scope, expiry) bound to plans and reducers.
- Policy Gate: declarative allow/deny/require‑approval rules over effects and plans.
- WASM Runtime: deterministic sandbox (Wasmtime profile) for reducers and pure components.
- Effect Manager: queues effect intents, dispatches to adapters, ingests receipts, enforces idempotency.
- Adapters: host‑side executors for effect kinds (HTTP, Blob/FS, Timer, LLM ship in v1; custom adapters can register additional kinds/cap types) with signing.
- CLI/Tooling: world lifecycle commands, shadow/diff, approvals, module build/register, and inspection.
- Observability: provenance (“why graph”), journal tailing, receipt viewers, and minimal metrics.

## Kernel and Event Flow

### Deterministic Stepper

The kernel processes exactly one event at a time per world. It applies events to both control‑plane state (AIR manifest and policies) and data‑plane state (reducer states) in a fixed order. When appropriate, the stepper produces derived events such as PolicyDecisionRecorded, Applied, or SnapshotCreated.

### Event Kinds (v1)

The kernel handles several categories of events. The first four are **design-time** events (control-plane evolution); the rest are **runtime** events (normal operation):

**Design-time events (dual-keyed):**
- **Proposed** {proposal_id, patch_hash, author, manifest_base, description?}
- **ShadowReport** {proposal_id, patch_hash, manifest_hash, effects_predicted, pending_receipts?, plan_results?, ledger_deltas?}
- **Approved** {proposal_id, patch_hash, approver, decision:"approve"|"reject"}
- **Applied** {proposal_id, patch_hash, manifest_hash_new}
- **Manifest** {manifest_hash} (appended whenever the active manifest changes; replay uses these to swap manifests in-order)

**Runtime events:**
- **EffectQueued** {intent_hash, kind, params_ref, cap_ref}
- **ReceiptAppended** {intent_hash, receipt_ref, status}
- **PolicyDecisionRecorded** {subject, decision, rationale_ref}
- **SnapshotCreated** {height, snapshot_ref}

### Ordering and Fences

Receipts include the intent_hash and a logical height fence; late receipts that arrive after a rollback are ignored. Idempotency keys ensure at‑least‑once adapter retries do not duplicate state transitions.

## Storage and Snapshots

### Journal

The journal consists of segment files with monotonic sequence numbers. Events are length‑prefixed, canonical CBOR. Segments are validated on load; corrupt segments are quarantined with clear diagnostics.

Manifest updates are also recorded as `Manifest` journal entries. These are appended on first boot (empty journal) and whenever the active manifest changes (governance apply or `aos push`). Replay applies manifest records in-order, swapping the active manifest without emitting new entries.

### Snapshots

Snapshots persist control‑plane AIR state (manifest hash + content), reducer state bytes (canonical CBOR by declared schema), and pinned blob roots. They are created periodically or on demand. Restore operations replay from the last snapshot to head, using the journal as the authoritative source; any later `Manifest` entries swap manifests during replay.

### Content‑Addressed Store (CAS)

Nodes (AIR terms, receipts) and blobs (WASM modules, large payloads) are addressed by SHA‑256 of their canonical encoding. Deduplication across worlds is possible via shared backing stores; world manifests pin required roots.

## Control Plane Interfaces

### AIR Manifest

The manifest is read-only at runtime and serves as the authoritative catalog of defmodule, defplan, defschema, defcap, and defpolicy objects. Built-in catalogs provide `sys/*` schemas/effects/caps/modules; external manifests may reference `sys/*` entries but may not define them. Updates occur only via approved Proposed → Applied events.

### AIR Loader/Validator

Authoring ergonomics and determinism meet here. The loader accepts either JSON lens described in the AIR spec—concise schema-directed “sugar” JSON or the tagged canonical overlay used by tools/agents—and canonicalizes both to typed values before anything touches the kernel. Built-in catalogs are merged for `sys/*` schemas/effects/caps/modules; external `sys/*` definitions are rejected. Every value is normalized (set dedupe/order, map ordering, numeric range checks, decimal128, option/variant envelopes) and then encoded as canonical CBOR with the declared schema hash bound in. That canonical form is the only thing the kernel, store, and hash engine ever see.

After canonicalization, the loader validates references, shapes, capability declarations, and plan graphs, then exposes a typed view for the kernel. Tooling hooks (`air fmt`, `air diff`, `air patch`) operate on the same canonical CBOR but can render either lens for humans.

### Capabilities (Constraints-Only)

Capabilities are passed to plans and modules by grant name; no ambient authority exists. At enqueue time the kernel enforces parameter constraints (hosts, models, max_tokens ceilings) via deterministic **cap enforcer modules** (defaulting to `sys/CapAllowAll@1` when a defcap omits one) and checks expiry against `logical_now_ns`. Budgets and ledger accounting are deferred to a future milestone.

### Policy Gate (v1)

The policy gate evaluates allow/deny decisions for effects based on origin-aware rules. Origin metadata (origin_kind: plan or reducer, origin_name) is attached to all effect intents. Rule evaluation is first-match-wins; if no rule matches, the default is deny. Decisions are journaled as PolicyDecisionRecorded events.

Deferred to v1.1+: approvals (require_approval), rate limits (rpm), and identity/principal support.

## Triggers and Events (Runtime)

This section describes **runtime** behavior: how reducers, plans, and effects collaborate to perform domain work.

### Triggers

The manifest contains `triggers`: mappings from DomainIntent event schemas to plan names, plus optional `correlate_by` keys. When a reducer emits a DomainIntent event, the kernel appends it to the journal and starts the configured plan with that event as input.

### Communication Pattern

The typical runtime flow between reducers and plans follows six steps:

1. **Reducer emits DomainIntent** (e.g., `ChargeRequested`) as a domain event.
2. **Trigger starts plan** with the event as `@plan.input` and records a correlation id if provided.
3. **Plan emits effects** under capabilities; the kernel checks: (a) capability grant constraints, (b) policy decision (origin-aware). The Effect Manager dispatches if allowed.
4. **Adapter executes** the effect and appends a signed receipt; the plan's `await_receipt` step resumes with the receipt value.
5. **Plan raises result** via `raise_event`, publishing a DomainEvent (e.g., `PaymentResult`) to the bus; routing delivers it to reducers, which advance their typestate.
6. **Optional continuation**: plans may `await_event` for subsequent reducer‑produced events to continue orchestration in one instance. The wait is future-only, first-match-per-waiter, and broadcast (events are not consumed). When a trigger set a `correlate_by` key, `await_event` **must** include a `where` predicate (typically matching that key) to avoid cross-talk between concurrent plan instances.

### Governance and Observability

All external I/O crosses `emit_effect` and is policy/capability‑gated; receipts are signed and journaled. State changes occur only via events → reducers; plans never mutate reducer state directly. Correlation keys allow tying receipts and effects to domain entities in the "why graph".

## Compute Layer

### WASM Runtime

The WASM runtime uses a deterministic profile: no threads, no ambient clock or randomness, stable float behavior, and limited hostcalls for pure intrinsics (serialization, hashing) and foreign‑memory copy. When a module declares a context schema, the kernel passes in deterministic time/entropy captured at ingress; there are no direct syscalls.

### Module Registry

Modules are content‑addressed WASM artifacts registered in the manifest with declared interfaces. Two types exist in v1:

- **Reducer**: state machine reacting to events.
  - ABI: `step(envelope) → envelope`, where the input envelope includes state/event bytes and an optional call context (if declared).
- **Pure Component**: pure function.
  - ABI: `run(envelope) → envelope`, where the input envelope includes input bytes and an optional call context (if declared).

### Keyed Reducers (Cells)

Version 1.1 adds first‑class "cells": many instances of the same reducer FSM keyed by an id (e.g., order_id). The ABI stays a single `step` export; the kernel passes an envelope with an optional call context, and for keyed reducers that context includes `key` and `cell_mode`. Routing uses `manifest.routing.events[].key_field`; triggers may set `correlate_by`. Per‑cell state is stored via a CAS‑backed `CellIndex` whose **root hash** is kept in world state/snapshots; returning `state=null` deletes the cell. Scheduler round‑robins between ready cells and plan runs. See [spec/06-cells.md](06-cells.md) for the full v1.1 behavior and migration notes.

### Router and Inbox

Deterministic routing hooks trigger reducers or plans based on event kinds and manifest routing tables. Reducers emit effect intents (not side‑effects) which enter the outbox.

## Effects, Adapters, and Receipts

### Effect Manager

The Effect Manager maintains an outbox of effect intents; each intent is typed and references a capability handle. **Before hashing/enqueueing**, it decodes effect params CBOR using the effect kind's param schema, canonicalizes to the AIR `$tag/$value` form, and re-encodes; the canonical bytes are stored, hashed, and passed to adapters. Non-conforming params are rejected early. Plans, reducers, and internal tooling all traverse this same normalizer so authoring sugar cannot perturb intent identity or policy decisions. The Effect Manager dispatches to adapters with idempotency keys and deadlines, retrying with backoff for transient failures. The final status is captured in a receipt. `introspect.*` effects are handled by an internal kernel adapter that synthesizes receipts deterministically (no I/O) while still passing through the same intent/receipt journal path.

### Adapters (v1)

Four adapters ship in v1:

- **HTTP**: `http.request(method, url, headers, body_ref) → receipt(status, headers, body_ref, timings)`
- **Blob**: `blob.{put,get} → receipt(blob_ref, size)`
- **Timer**: `timer.set(deliver_at_ns) → receipt(delivered_at_ns)` (logical time)
- **LLM**: `llm.generate(model, params, input_ref) → receipt(output_ref, token_usage, cost, provider_id)`

Each adapter signs receipts (ed25519/HMAC) including intent_hash, inputs/outputs hashes, timings, and cost.

The effect catalog is **not closed**: the core schemas leave `EffectKind` and `CapType` open. V1 ships the above built-ins plus their capability types (`http.out`, `blob`, `timer`, `llm.basic`, `secret`), but additional adapters can register new kinds/cap types as soon as the runtime knows how to map them to schemas and receipts.

Version 1.2 will add **WASM-based adapters**: custom effect implementations that run as WASM modules with a non-deterministic profile, including WASI and other host capabilities. These enable extensible effect types while maintaining the receipt-based audit boundary—adapters can be deployed, upgraded, and sandboxed like any other module, but they operate outside the deterministic replay guarantees of reducers.

### Receipt Handling

ReceiptAppended events advance plans and reducers waiting on effects. During replay, the system consumes recorded receipts to produce identical state without re‑executing effects.

## Constitutional Loop (Design Time)

The constitutional loop governs **design-time** changes: how the system proposes, rehearses, approves, and applies modifications to its own control plane:

1. **Propose**: Submit AIR patches forming a proposal; the kernel validates and records **Proposed**, storing both the monotonic `proposal_id` (correlation key) and the content-addressed `patch_hash`.
2. **Shadow**: The kernel clones state and runs a shadow simulation with stubbed receipts; it records **ShadowReport** with predicted effects/costs and diffs (optionally as an opaque summary blob for compatibility).
3. **Approve/Reject**: Humans or policy record **Approved** with a `decision` (approve/reject) plus approver identity. A rejected proposal cannot be applied.
4. **Apply**: The kernel commits the new manifest root (**Applied**), recording `manifest_hash_new` alongside the ids for auditability; routing tables and capability bindings update atomically. Apply is only permitted after an approve decision.
5. **Execute**: Normal event flow resumes; new plans and modules are active under policy; effects produce receipts; audit trails accumulate.

### Shadow Runs

Shadow runs provide deterministic rehearsal of a proposal. They use a copy of the current state and manifest; effects are stubbed or use canned receipts. The output is a typed diff of control‑plane and reducer states, predicted effect counts and costs, and required capabilities. No changes persist until approval; outputs drive least‑privilege capability synthesis.

## Determinism and Safety

### Determinism

Canonical CBOR is used for all persisted values. [CBOR (Concise Binary Object Representation)](https://cbor.io/) is a binary serialization format similar to JSON but with a deterministic encoding—the same data structure always produces the same byte sequence, which is essential for content addressing and replay guarantees. 

Modules have no ambient access to time or randomness; all nondeterminism is isolated to the effect boundary and recorded via receipts. Deterministic time/entropy are provided only via the optional call context, sampled at ingress and journaled. This ensures that replaying the same journal and receipts always produces identical state.

### Safety

**Capability scoping**: Tokens encode scope and expiry; they are passed explicitly.

**Policy gates**: Enforce allow/deny/approval decisions and quotas before dispatch.

**Rollback**: Move the head to a prior snapshot; receipts include fences to ignore late arrivals after a rollback.

## Packaging and On‑Disk Layout (v1)

- world/
  - manifest.air.json (text) and manifest.air.cbor (canonical)
  - .aos/store/{nodes, blobs}/sha256/<hash>
  - journal/{00001.log, 00002.log, …}
  - snapshots/snap-<ts>-<height>.cbor
  - modules/<name>@<ver>-<hash>.wasm
  - receipts/<height>-<intent-hash>.cbor

## Tooling and Dev Experience

### CLI

The command‑line interface provides: world init/info; propose/shadow/diff/approve/apply; run/tail; receipts ls/show; cap grant/revoke; policy set; workspace inspection and edits (`aos ws`); filesystem sync via `aos push`/`aos pull` with `aos.sync.json`.

### Workspace Sync (`aos push` / `aos pull`)

Workspace sync is driven by a map file, `aos.sync.json` (world root by default). It declares how AIR assets, reducer builds, module exports, and workspace trees map to local directories.

Example:
```json
{
  "version": 1,
  "air": { "dir": "air" },
  "build": { "reducer_dir": "reducer", "module": "demo/Reducer@1" },
  "modules": { "pull": false },
  "workspaces": [
    {
      "ref": "reducer",
      "dir": "reducer",
      "ignore": ["target/", ".git/", ".aos/"],
      "annotations": {
        "README.md": { "sys/commit.title": "Notes Reducer" },
        "src/lib.rs": { "sys/lang": "rust" },
        "": { "sys/commit.message": "sync from local" }
      }
    }
  ]
}
```

Notes:
- `workspaces[].ref` uses `<workspace>[@<version>][/path]`; `aos push` rejects versioned refs.
- `annotations` values can be strings or JSON; strings are stored as UTF-8 blobs, JSON values are stored as canonical CBOR.
- Workspace paths are URL-safe; filesystem sync encodes per-segment using `~`-hex on UTF-8 bytes for non URL-safe segments or segments starting with `~`. Non-UTF-8 names are rejected for determinism.

### SDK

Rust helpers for reducers and pure components, plus a test harness for deterministic replay.

### Inspectors

Provenance ("why graph") and plan visualizer (text‑first for v1).

## Scaling Model

One world equals one thread; scale out by running many worlds. Heavy or parallel work happens in adapters; receipts rejoin the single thread via events. Cross‑world coordination (deferred) can use conventional messaging with capability delegation.

There is are some ideas how we can bring later parallelism into the the plan and reducer runtime. See [spec/06-parallelism.md](06-parallelism.md)

## Failure Handling

Adapters retry with exponential backoff; idempotency preserves correctness. Timeouts yield receipts with status=timeout; plans can gate on acceptable statuses. Dead‑letter policies handle intents that exceed retry or cost limits; the audit trail retains the full history. Failures are also propagated back to reducers so they have a chance to recover.

## Security Posture

The minimal trusted base—kernel, validator, and receipt verification code—is small and testable. Modules and manifests are content‑addressed; optional SBOM and signature checks occur at registration. Secrets are kept in capability tokens and adapter configuration; they are never embedded in WASM modules.

## Putting It Together (End‑to‑End)

Here's how all the pieces fit together in a typical **design‑time** change workflow:

1. **Register**: A plan in AIR wires modules and declares allowed effects and required capabilities.
2. **Propose**: A proposal patches the manifest with changes.
3. **Shadow**: A shadow run quantifies diffs and predicted costs.
4. **Approve**: Approval grants least‑privilege capabilities based on shadow results.
5. **Apply**: The kernel commits the new manifest root; routing tables and capability bindings update atomically.
6. **Execute**: Execution proceeds; effects produce signed receipts; snapshots capture state.
7. **Replay**: Replay from journal and receipts reproduces the exact same state.

Once applied, changes become **runtime** behavior: reducers emit domain intents, triggers start plans, plans orchestrate effects under policy and capabilities, adapters produce signed receipts, and results flow back as events. Both modes—design time and runtime—share the same deterministic kernel and journal. The homoiconic control plane (AIR) is what makes this possible: the system can inspect, simulate, and safely modify its own definition using the same event‑sourced substrate it uses for application logic.

This architecture yields a substrate where agents can co‑author and safely evolve systems: deterministic at the core, explicit and auditable at the edges, and unified by a small typed control plane.
