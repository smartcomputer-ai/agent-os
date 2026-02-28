# P4: Process Sessions Effects (Essential)

**Priority**: P4  
**Status**: Proposed  
**Date**: 2026-02-27

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
4. `process.session.close` (optional alias; can map to signal semantics)

Primary security boundary is session definition (`process.session.open`).
Per-command gating is operational (timeouts/size limits), not semantic command allowlisting.

## Effect Contracts (v1)

## `process.session.open`

Purpose: create a sandboxed execution session.

Params (conceptual):

1. `runtime`: `local` in initial implementation; container/microvm/remote reserved.
2. `identity`: user/uid/gid/privileged flags.
3. `mounts`: host path refs + guest path + mode (`ro`/`rw`).
4. `workdir`, `env`.
5. `network` mode (`none`/`restricted`/`full`).
6. `resources` limits (cpu/mem/time ceilings).
7. `labels` for audit/trace correlation.

Receipt (conceptual):

1. `session_id`
2. runtime metadata (`adapter_id`, host/runtime details)
3. timestamps/cost summary

## `process.exec`

Purpose: execute a command in an existing session.

Params:

1. `session_id`
2. `argv: list<text>` (structured args; avoid shell-string semantics)
3. `stdin_ref?`
4. `timeout_ns?`
5. optional `cwd` and `env_patch`

Receipt:

1. `exit_code`
2. `stdout_ref?`, `stderr_ref?` (prefer refs, not large inline bytes)
3. timings/cost

## `process.session.signal` / `process.session.close`

Purpose: terminate or gracefully stop a session.

Params:

1. `session_id`
2. `signal` (`term`/`kill`/`int`...)
3. optional graceful timeout

Receipt:

1. terminal status (`exited`/`killed`/`not_found`...)
2. optional exit code and end timestamp

## Capability and Policy Model

## New cap type

Add cap type: `process`.

Add built-in cap + enforcer pair:

1. `sys/process@1` (`cap_type: "process"`)
2. `sys/CapEnforceProcess@1` (pure enforcer module)

Cap schema should constrain session-open envelope:

1. allowed runtime modes,
2. privileged/root allowance,
3. mount path/mode constraints,
4. network modes,
5. resource ceilings.

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
2. Put large stdout/stderr/progress artifacts in CAS and return refs in receipts.

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
   - optional `process.session.close`
3. In-process adapter implementation for local runtime.
4. Receipt-first contract with blob refs for large outputs.

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
