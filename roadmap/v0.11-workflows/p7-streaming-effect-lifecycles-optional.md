# P7 (Optional): Streaming Effect Intents and Continuation Frames

**Priority**: P3 (optional extension)  
**Status**: Complete (Option A++ baseline implemented on 2026-02-26)  
**Depends on**: `roadmap/v0.11-workflows/p1-module-workflow-foundation.md`

## Goal

Support workflow orchestration where one long-lived external operation yields multiple intermediate updates, while preserving:

1. replay-or-die determinism,
2. strict authority boundaries (`effects_emitted` + cap/policy gates),
3. concurrency-safe routing with no cross-talk,
4. strict-quiescence apply safety.

Examples:

1. LLM streaming intents with semantic milestones and tool-call requests.
2. SSH/VM console intents that remain open while emitting output.
3. Human-loop operations with partial progress before terminal completion.

## Hard Constraints

1. Deterministic replay remains mandatory.
2. Capability/policy gates remain mandatory for every external action.
3. Journal/snapshot/quiescence semantics remain auditable and deterministic.
4. Manifest routing (`routing.subscriptions`) is ingress-only for domain events.
5. Continuations of already-emitted effects must be manifest-independent.
6. Kernel does not need to implement SSE/WebSocket transports; adapters may use streaming transports internally.

## Problem Statement

Current runtime semantics are single-settlement per effect intent (`intent_id` maps to one terminal receipt and then in-flight state is removed).

For streaming workloads, that alone is too narrow:

1. intermediate updates can drive additional workflow actions,
2. workflows may need to emit follow-up control intents before terminal completion,
3. multiple long-lived operations may be active concurrently.

## Decision Options

### Option A++ (Recommended): Stream Frames + Terminal Receipt

Keep one terminal receipt per intent, and add adapter-origin stream frames for progress/milestones routed on the same continuation rail as receipts.

Lifecycle:

1. Workflow emits `effect.start` intent `I`.
2. Kernel records in-flight intent metadata (including origin tuple and `emitted_at_seq` fence).
3. Adapter emits stream frames correlated to `I` while intent is open.
4. Kernel routes each frame directly to `(origin_module_id, origin_instance_key)` using pending intent state (not manifest subscriptions).
5. Workflow may emit follow-up control intents (`input`, `cancel`, `close`, etc.) that reference start `intent_id`.
6. Adapter emits one terminal receipt for `I`; kernel settles and removes in-flight state.

Pros:

1. Preserves existing terminal-settlement model.
2. Continuations stay manifest-independent, matching post-plan routing contract.
3. Avoids cross-talk risk from subscription-based fanout.
4. Lower churn than receipt-frame semantic reset.

Cons:

1. Runtime has two continuation record types (stream frame + terminal receipt).
2. Requires clear trace UX separating progress from settlement.

### Option B: Multi-Frame Receipts per Intent (Semantic Reset)

Replace single terminal receipt semantics with ordered receipt frames on one intent.

Pros:

1. Single "adapter datum = receipt frame" rail.
2. Potentially uniform signing/verification and UX.

Cons:

1. Larger kernel/spec rewrite (pending maps, journal invariants, wait semantics, quiescence semantics).
2. Blurs "receipt as settlement" unless non-terminal receipt semantics are added.
3. Higher migration and replay risk in v0.11 timeline.

## Recommended Direction for v0.11

Choose **Option A++** unless there is a hard requirement that every adapter-origin intermediate datum must be represented as a receipt frame.

## Implementation Status (2026-02-26)

- [x] Option A++ selected and implemented (`EffectStreamFrame` continuation rail + single terminal receipt settlement).
- [x] Added builtin `sys/EffectStreamFrame@1` schema and wired builtin schema loading assertions.
- [x] Added `aos-effects::EffectStreamFrame` type and exports.
- [x] Added kernel stream-frame journaling (`JournalKind::StreamFrame`, `JournalRecord::StreamFrame`).
- [x] Added deterministic stream ingestion rules: unknown intent drop, identity/fence mismatch drop, non-monotonic drop, deterministic gap diagnostic, monotonic cursor advance.
- [x] Added workflow continuation delivery for stream frames using pending-intent origin routing (manifest-independent).
- [x] Persisted `last_stream_seq` in workflow in-flight state across snapshot/replay and restored acceptance behavior.
- [x] Added host ingress/control support for stream frames (`ExternalEvent::StreamFrame`, `stream-frame-inject` control command).
- [x] Added trace/observability updates for stream frames and open streaming-intent diagnostics.
- [x] Added/updated tests for stream event shaping, runtime routing/cursor behavior, snapshot/replay cursor restore, and replay decode for stream-frame records.
- [x] Workspace validation passed via `cargo test` (2026-02-26).

## Normative Contract (Option A++)

### 1) Intent identity is deterministic at emit time

1. `intent_id` is the canonical correlation identity for a streaming operation.
2. No "first progress/start response yields session id" handshake is required.
3. `session_id` is not a required runtime/schema field for AOS correlation.
4. Adapters may keep provider-native handles internally (for example `provider_session_id`) and may include them in payloads for diagnostics.

### 2) Stream frame envelope is receipt-routed continuation data

Each stream frame must carry:

1. `origin_module_id`,
2. `origin_instance_key`,
3. `intent_id`,
4. `effect_kind`,
5. `emitted_at_seq` (fence from original intent enqueue),
6. `seq` (monotonic per intent stream),
7. `kind` (semantic milestone, not transport chunk),
8. `payload` (inline small value or content-addressed reference for large data).

### 3) Continuation routing is manifest-independent

1. Stream frames are not routed via `routing.subscriptions`.
2. Kernel accepts/routes stream frames using pending intent state by `intent_id`.
3. Routing target is `(origin_module_id, origin_instance_key)` from recorded in-flight metadata.

### 4) Follow-up control actions are explicit effect intents

Workflow follow-ups are normal effect intents and must pass all standard gates:

1. module `effects_emitted` allowlist,
2. capability checks,
3. policy checks.

Typical actions:

1. `effect.input`,
2. `effect.modify`,
3. `effect.pause` / `effect.resume`,
4. `effect.cancel`,
5. `effect.close`.

### 5) Deterministic ingestion, fencing, and dedup

Kernel acceptance rules:

1. reject/drop frame if no in-flight intent for `intent_id`,
2. reject/drop frame if `emitted_at_seq` does not match recorded in-flight fence,
3. reject/drop frame if `seq <= last_stream_seq` for that intent stream,
4. accept frame if `seq > last_stream_seq`, then advance cursor deterministically.

Gap policy:

1. gap detection (`seq > last_stream_seq + 1`) must be deterministic,
2. implementation may accept-with-diagnostic or reject-by-policy, but behavior must be explicit and replay-stable.

### 6) Snapshot/replay persistence requirements

Persist enough metadata to make restart behavior byte-identical:

1. in-flight intent identity metadata,
2. per-intent `last_stream_seq` cursor.

Full frame history remains in journal; snapshot does not need to duplicate historical frames.

### 7) Terminal settlement semantics remain unchanged

1. exactly one terminal receipt per intent (`ok|denied|faulted`),
2. terminal receipt removes in-flight state for that `intent_id`,
3. terminal settlement remains the unambiguous completion point for governance/quiescence.

### 8) Quiescence and upgrade safety

1. Open streaming intents count as in-flight intents and block apply.
2. Apply is allowed only after terminal settlement (or deterministic cancel path that reaches terminal settlement).
3. Diagnostics for blocked apply must report owning workflow instance, `intent_id`, `effect_kind`, age, and stream cursor.

## Normative Contract (If Option B Is Chosen Instead)

If multi-frame receipts are adopted, these invariants are required:

1. `inflight_intents[intent_id]` persists until terminal frame.
2. frames carry `intent_id`, `seq`, `terminal`, and optional `frame_kind`.
3. exactly one terminal frame per intent.
4. duplicates are idempotent by `(intent_id, seq)`.
5. quiescence treats non-terminal intents as in-flight.
6. snapshot/replay restores per-intent frame cursor byte-identically.

## Example Patterns

### 1) LLM interleaving with tools

1. Start `llm.session.start` intent `I`.
2. Adapter emits stream frame `kind=tool_call.requested`.
3. Workflow emits tool intent `T1`; on `T1` receipt emits `llm.session.input { intent_id: I, ... }`.
4. Adapter emits more stream frames and later terminal receipt for `I`.

### 2) Console intent with explicit close

1. Start `vm.open` or `ssh.open` intent `I`.
2. Adapter emits `opened/stdout/stderr` stream frames for intent `I`.
3. Workflow emits follow-up `exec` intents and eventual `close/cancel` intent.
4. Adapter emits terminal receipt for start intent `I` when the provider session is truly closed.

## Out of Scope

1. SSE/WebSocket servers inside kernel.
2. Provider token-level or byte-level transport guarantees.
3. Backward compatibility with plan-era streaming assumptions.

## Work Items by Crate

### `crates/aos-air-types` / `spec/`

1. Define stream-frame envelope schema aligned with receipt identity fields plus `seq/kind/payload`.
2. Document intent-only correlation contract for streaming start intents (no separate `session_id` field).
3. Document control-intent schema conventions that reference start `intent_id`.
4. If Option B: update effect/receipt lifecycle model for receipt frames.

### `crates/aos-kernel`

1. Add a journaled stream-frame continuation record kind.
2. Extend pending intent index with per-intent stream cursor (`last_stream_seq`).
3. Route stream frames through pending-intent origin mapping (manifest-independent).
4. Wake workflow instances on stream-frame arrival in addition to receipts/domain ingress.
5. Include open streaming intents and stream cursors in strict-quiescence diagnostics.
6. If Option B: replace terminal-only receipt bookkeeping with frame lifecycle state.

### `crates/aos-effects`

1. Keep terminal receipt model unchanged for Option A++.
2. Add stream-frame intent/record types and validation helpers.
3. If Option B: add receipt-frame model and terminal semantics.

### `crates/aos-host` adapters

1. Coalesce provider transport chunks into semantic milestones (`kind`).
2. Emit stream frames with deterministic `intent_id`, `seq`, and fence metadata; include optional `provider_session_id` only as adapter payload metadata.
3. Support follow-up control intents (`input/modify/cancel/close`) keyed by start `intent_id`.

### `crates/aos-cli` / trace tools

1. Show stream frames grouped by `intent_id` and origin instance.
2. Clearly distinguish progress frames from terminal receipts.
3. Show apply-block reasons for open streaming intents with stable routing identity.

### Tests (`aos-kernel`, `aos-host`, fixtures)

1. Concurrent streaming intents do not cross-deliver frames or receipts.
2. Interleaving works: stream frame from `I` triggers follow-up effect while `I` remains open.
3. Restart/replay preserves `last_stream_seq` and acceptance behavior.
4. Late/duplicate/out-of-order frames are handled deterministically.
5. Strict-quiescence blocks apply while streaming intents are open and unblocks on terminal settlement.

## Acceptance Criteria

1. Workflows can react to intermediate adapter updates and emit new intents before terminal receipt.
2. Continuations (stream frames + receipts) route correctly without manifest subscription dependency.
3. Replay from genesis produces byte-identical snapshots for streaming workloads.
4. Governance/trace diagnostics identify open streaming intents and explicit apply-block reasons.
5. Exactly one terminal settlement remains the completion point per start intent.

## Acceptance Status (2026-02-26)

- [x] Workflows can react to intermediate stream updates before terminal receipt via `sys/EffectStreamFrame@1` continuation delivery.
- [x] Stream frames and receipts route via pending-intent origin identity, independent of `routing.subscriptions`.
- [x] Replay/snapshot behavior preserves stream cursor state (`last_stream_seq`) and deterministic acceptance.
- [x] Trace surfaces open streaming intents and stream cursors for quiescence/apply-block diagnostics.
- [x] Single terminal receipt settlement semantics remain unchanged.

## Decision Gate

Default for v0.11:

1. Implement Option A++ (intent-id-only correlation, stream frames on receipt-routed continuation rail, one terminal receipt). **Selected and completed.**
2. Escalate to Option B only if there is a strict product/security requirement that all intermediate adapter-origin data be modeled as receipt frames.
