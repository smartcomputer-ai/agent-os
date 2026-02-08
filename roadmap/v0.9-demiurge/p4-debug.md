# P4: Developer Observability and Traceability

**Priority**: P4  
**Effort**: Medium  
**Risk if deferred**: High (feature work stays slow because failures are opaque)  
**Status**: In Progress

## Goal

Make any stalled app flow diagnosable in minutes (not hours) by exposing deterministic, queryable execution traces across:
- reducer events/state changes,
- plan execution/waits,
- effect intents/receipts,
- cap/policy decisions.

This should work for Demiurge first, but remain app-agnostic and reusable across all AOS apps.

## Current Gaps (Why Progress Is Slow)

- `/api/journal` currently returns only `effect_intent` and `effect_receipt` records, not the full event/plan/decision chain.
- The kernel already journals rich records (`domain_event`, `cap_decision`, `policy_decision`, `plan_result`, `plan_ended`), but they are not exposed through control/API tooling.
- Shell can show chat state but cannot answer "what is this request waiting on right now?".
- CLI has `journal head` and `replay`, but no practical trace/tail command for request-level debugging.
- Daemon logs are useful during local development but are not sufficient as the primary debugging surface (ephemeral, noisy, hard to correlate).

## Design Principles

- One generic substrate, multiple UX adapters: kernel/host expose generic trace data; CLI and shell provide app-friendly views.
- Correlation by lineage, not app semantics: use `event_hash`, `intent_hash`, plan ids, and `correlate_by` values.
- Deterministic first: traces should be derivable from journal + current in-memory wait state.
- Incremental delivery: ship the highest leverage primitive first, then UX.

## Decision Summary

1. Build a generic trace primitive in host/kernel surfaces, not Demiurge-specific logic.
2. Expand journal access to include all journal record kinds (with filters), not only intents/receipts.
3. Add a request-level lineage query that combines journal history with live wait queues.
4. Ship CLI first (`aos trace`, `aos journal tail`) so debugging works before shell UX lands.
5. Add a lightweight Demiurge "Debug" panel in shell that consumes the same generic trace API.

## Phase Plan

## Phase 1: High-Leverage Substrate (Do This First)

### 1) Full Journal Tail Access

Expose full journal records through control + HTTP:
- Extend control `journal-list` to return all record kinds.
- Add kind filters (`domain_event`, `effect_intent`, `effect_receipt`, `cap_decision`, `policy_decision`, `plan_result`, `plan_ended`, ...).
- Keep seq ordering stable and include decoded summaries.

Why first:
- Very high leverage, low conceptual risk.
- Reuses existing journaling already in kernel.
- Immediately unlocks offline and replay debugging.

### 2) Generic Trace Query

Add `trace-get` in control and `GET /api/debug/trace` in HTTP.

Inputs:
- `event_hash=...` (primary)
- optional correlation mode: `schema + correlate_by + value`

Output:
- Root event metadata (`schema`, `event_hash`, seq, key).
- Linked intents (origin reducer/plan, kind, seq).
- Cap/policy decisions for each intent.
- Receipts (`status`, `adapter_id`, full payload).
- Raised/result events and `plan_result`/`plan_ended`.
- Live wait snapshot:
  - pending plan receipts,
  - waiting events,
  - pending reducer receipts,
  - queued effects.
- Derived terminal status: `completed | waiting_receipt | waiting_event | failed | unknown`.

Why this is the key bottleneck fix:
- It answers the single most important developer question: "where exactly did this flow stop?"
- It is generic and works for Demiurge, shell workflows, and future apps.

### 3) Full Payload Visibility (v1)

- No redaction in v1.
- Return full decoded payloads and params in trace/journal debug views.
- Keep this explicit in docs as a local-only experimental debugging choice.

## Phase 2: CLI Ergonomics (Immediately After Phase 1)

Add developer-facing commands:
- `aos journal tail --from <seq> --limit <n> --kinds ...`
- `aos trace --event-hash <hash>`
- `aos trace --schema <schema> --correlate-by <field> --value <json>`
- `aos trace --follow` (optional) to re-query until terminal status.

CLI output modes:
- default concise timeline (human-readable),
- `--json` for automation,
- `--out <file>` for trace artifacts attached to bug reports.

Why this matters now:
- Most debugging is currently terminal-driven.
- It avoids waiting on shell UI work before gaining velocity.

## Phase 3: Shell Debug UX (Thin Adapter)

In Demiurge shell:
- Add a per-message/request "Debug" drawer.
- Show trace timeline and current wait reason.
- Link to hashes (event/intent/output_ref) for copy/paste into CLI.
- Show actionable hints for common failure classes:
  - policy/cap denied,
  - adapter error/timeout,
  - plan waiting for missing receipt/event.

Important:
- No Demiurge-specific tracing in kernel.
- Shell only maps `chat_id/request_id` to the generic trace query.

## Phase 4: Testing and Regression Guardrails

Add deterministic test coverage for the new debug surfaces:
- Host/control integration tests for full journal tail and trace query.
- Replay parity test: trace derived from replayed world matches original terminal classification.
- Demiurge e2e test assertions based on trace status (not only final state shape).
- Update `apps/demiurge/scripts/smoke_introspect_manifest.sh` to optionally fail with emitted trace artifact on timeout.

## What We Should Not Do Yet

- Do not build a full distributed logging/telemetry stack.
- Do not start with SSE streaming as the primary fix; first make causality queryable.
- Do not add app-specific trace logic in kernel.
- Do not depend on console debug logs for core debugging workflows.

## Success Criteria

- A stalled Demiurge request can be diagnosed with one command/API call.
- Developers can identify exact stop point and reason (wait, deny, timeout, adapter error).
- Shell and CLI present consistent answers because both use the same trace substrate.
- New apps can reuse the exact same debugging surfaces without custom kernel work.

## Practical Next Slice (Recommended)

Implement in this order:
1. Extend control/API journal tail to full record kinds.
2. Add `trace-get` + `/api/debug/trace` with terminal status classification.
3. Add `aos trace` CLI command.
4. Add Demiurge debug drawer in shell.
5. Add trace-based integration tests and smoke artifact output.

This is the minimum path that materially increases development velocity without introducing heavy new infrastructure.

## Implementation Status (2026-02-06)

Completed:
- Phase 1.1 full journal tail in control/HTTP:
  - `journal-list` now returns all journal record kinds with optional `kinds` filtering.
  - `GET /api/journal` forwards `kinds`.
- Phase 1.2 generic trace query:
  - `trace-get` control command and `GET /api/debug/trace` endpoint implemented.
  - Includes root metadata, bounded journal window, live wait snapshots, and derived terminal state.
- Phase 2 CLI baseline:
  - `aos journal tail --from --limit --kinds [--out]`
  - `aos trace --event-hash [--window-limit] [--follow] [--out]`
  - `aos trace --schema --correlate-by --value` correlation mode
- Phase 3 shell debug UX baseline:
  - Per-message Debug drawer in Demiurge chat.
  - Drawer fetches `/api/debug/trace` using generic correlation fields.
  - Shows timeline, wait state, copyable hashes, and basic failure hints.
- Phase 4 initial smoke artifact support:
  - `apps/demiurge/scripts/smoke_introspect_manifest.sh` now emits debug artifacts on failure:
    - `journal-tail.json`
    - best-effort `trace.json` for latest Demiurge domain event

Remaining:
- Replay parity test for trace classification and broader trace-driven e2e assertions.
