# P4: Process Sessions Effects (Essential)

**Priority**: P4  
**Status**: Complete  
**Date**: 2026-02-28

## Goal

Add an essential process-execution effect family for agent workloads (coding,
build/test, repo operations) using a session model that is:

1. capability-gated at session acquisition time,
2. compatible with workflow runtime/effect handling as implemented today,
3. aligned with P1-P3 (startup preflight, optional manifest bindings, in-process routing first).

## Current Reality Constraints (Must Match)

1. Runtime is workflow-module centric; effects are emitted from workflow outputs.
2. Enqueue path canonicalizes params and applies cap/policy before dispatch.
3. Receipt handling is strict: receipt payload is schema-normalized before delivery; faults are explicit and can fail instances.
4. Host adapter execution is currently in-process only in this slice.
5. `AsyncEffectAdapter` currently returns one terminal `EffectReceipt` per intent (no adapter-native streaming API yet).
6. Effect routing is kind-keyed today; P2 introduces optional `kind -> adapter_id` binding.

References:

- `crates/aos-kernel/src/effects.rs:236`
- `crates/aos-kernel/src/world/runtime.rs:204`
- `crates/aos-kernel/src/world/runtime.rs:447`
- `crates/aos-host/src/adapters/registry.rs:43`

## Design Summary

Introduce a process-session effect family:

1. `process.session.open`
2. `process.exec`
3. `process.session.signal`

Primary security boundary is session definition (`process.session.open`).
Per-command gating is operational (timeouts/size limits), not semantic command allowlisting.

## Minimal Interface (v0.12)

The minimal interface for this slice is exactly these three effects:

1. `process.session.open`
2. `process.exec`
3. `process.session.signal`

`process.session.close` is deferred; use `process.session.signal` with `term`.

## Effect Contracts (Minimal v1)

## `process.session.open`

Purpose: create a sandboxed execution session.

Params (minimal):

1. `target`: runtime-tagged variant (nested object).
2. v0.12 requires only `target.local` with:
   - `mounts?`: optional list of host/guest path bindings with `ro|rw` mode.
   - `workdir?`
   - `env?`
   - `network_mode`: `none|full`
3. `session_ttl_ns?`
4. `labels?`

Receipt (minimal):

1. `session_id`
2. `status`: `ready|error`
3. `started_at_ns`
4. `expires_at_ns?`

## `process.exec`

Purpose: execute a command in an existing session.

Params (minimal):

1. `session_id`
2. `argv: list<text>` (structured args; avoid shell-string semantics)
3. `cwd?`
4. `timeout_ns?`
5. `env_patch?`
6. `stdin_ref?`
7. `output_mode?`: `auto|require_inline`

`output_mode` semantics:

1. `auto` (default): adapter chooses `inline_text`/`inline_bytes`/`blob`.
2. `require_inline`: adapter must return inline output (`inline_text` or
   `inline_bytes`) for available stdout/stderr; if output cannot be safely
   returned inline due to host limits, return `status: error` with a
   machine-readable reason (for example `inline_required_too_large`).

Receipt (minimal):

1. `exit_code`
2. `status`: `ok|timeout|signaled|error`
3. `stdout?`, `stderr?` as `option<variant>`:
   - `inline_text { text }` for small UTF-8 output
   - `inline_bytes { bytes }` for small non-UTF8 output
   - `blob { blob_ref, size_bytes, preview_bytes? }` for large output
4. `started_at_ns`
5. `ended_at_ns`

Adapter chooses arm based on `output_mode` and size/encoding constraints. This
avoids mandatory follow-up blob reads for common small textual outputs while
keeping large outputs CAS-backed under `auto`.

## `process.session.signal`

Purpose: terminate or gracefully stop a session.

Params (minimal):

1. `session_id`
2. `signal` (`term`/`kill`/`int`...)
3. `grace_timeout_ns?`

Receipt (minimal):

1. `status`: `signaled|not_found|already_exited|error`
2. `exit_code?`
3. `ended_at_ns?`

## Capability and Policy Model

## New cap type

Add cap type: `process`.

Add built-in cap + enforcer pair:

1. `sys/process@1` (`cap_type: "process"`)
2. `sys/CapEnforceProcess@1` (pure enforcer module)

Cap schema should constrain session-open envelope:

1. allowed `target` variant arms (v0.12: `local` only),
2. per-arm constraints (for `local`: optional mount path/mode constraints, network modes, identity/privilege policy),
3. resource ceilings.

Use a tagged nested shape (`target`) instead of a flat cross-runtime record.
This keeps future runtime expansion (`docker`/`microvm`/`remote`) compatible
with canonical schema normalization and current cap-enforcer mechanics.

## Gating posture

1. Main gate is `process.session.open`.
2. `process.exec`/signal operations are authorized within opened-session scope + operational limits.
3. Policy remains origin-aware and can force broker workflows.

Note: `origin_scope` is part of `defeffect`, but current runtime enforcement focus is module allowlist + cap/policy checks. Do not rely on `origin_scope` alone as the security boundary in this phase.

## Streaming Progress Model

Desired model is "single terminal receipt + progress events while running".

Given current host adapter API shape, land in two steps:

## Step A (in-scope now)

1. Terminal receipts only for `open`/`exec`/`signal`.
2. Under `output_mode: auto`, return small stdout/stderr inline (`inline_text`/`inline_bytes`) and put large outputs in CAS (`blob` arm with `blob_ref`); callers may request `require_inline` when they need immediate inline output.

## Step B (follow-up)

1. Add adapter->host streaming channel support (e.g. `EffectStreamFrame` path).
2. Map stream frames to workflow events via existing strict receipt/frame delivery machinery.

This keeps v0.12 grounded while preserving the target UX.

## Determinism and Replay

1. External process execution is never replayed.
2. Replay uses journaled intents/receipts (and stream frames when implemented).
3. If receipts/events carry blob refs for logs/artifacts, those blobs must be retained/exported as replay dependencies.

## Routing and Host Integration

1. P2/P3 route process kinds via optional `effect_bindings` (`kind -> adapter_id`).
2. Compatibility fallback remains kind-keyed in rollout mode.
3. P1 preflight should treat required external process kinds like any other external effect.

## v0.12 Implementation Slice

## In scope

1. Add `process` cap type + cap enforcer contract.
2. Add `defeffect`/schemas for:
   - `process.session.open`
   - `process.exec`
   - `process.session.signal`
3. In-process adapter implementation for local runtime.
4. Receipt-first output contract: inline for small UTF-8/bytes, blob refs for large outputs.

## Deferred

1. Remote/distributed process runtime.
2. Container/microVM orchestration guarantees.
3. Adapter-native real-time streaming API.
4. PTY-grade interactive protocol.

## Non-Goals

1. Fine-grained command-level semantic sandboxing as the primary security boundary.
2. Full remote execution fabric in this roadmap slice.
3. Replacing existing effect/receipt determinism model.

## Decision in One Sentence

Adopt a capability-gated `process.session` effect family where `open` defines
the security boundary, `exec` performs work inside that boundary, and outcomes
flow through strict receipt-first journaling (with streaming as a follow-up
API extension).

## Completion Notes (2026-02-28)

1. Added built-in effect definitions and schemas for:
   - `process.session.open`
   - `process.exec`
   - `process.session.signal`
2. Added new built-in capability `sys/process@1` (`cap_type: "process"`) with
   `sys/ProcessCapParams@1` schema and pure cap enforcer module
   `sys/CapEnforceProcess@1`.
3. Implemented `cap_enforce_process` in `aos-sys` and wired default kernel
   cap-type-to-enforcer mapping (`process -> sys/CapEnforceProcess@1`).
4. Implemented in-process host adapters for local process sessions:
   - open adapter (`process.session.open`)
   - exec adapter (`process.exec`)
   - signal adapter (`process.session.signal`)
5. `process.exec` output contract is live:
   - `output_mode=auto`: inline small outputs, CAS blob refs for large outputs
   - `output_mode=require_inline`: returns payload `status:error` with
     `error_code=inline_required_too_large` when output cannot be safely inlined.
6. Host profile defaults and startup route preflight include process kinds the
   same way as other external effects, with compatibility routing retained.
