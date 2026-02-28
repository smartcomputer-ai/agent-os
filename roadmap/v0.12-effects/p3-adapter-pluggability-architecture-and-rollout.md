# P3: Adapter Pluggability Architecture and Rollout

**Priority**: P3  
**Status**: Complete  
**Date**: 2026-02-28

## Goal

Deliver a minimal pluggable host adapter architecture after P1/P2:

1. route by `adapter_id` (from P2 bindings),
2. keep execution in-process in this slice,
3. preserve deterministic replay and existing kernel receipt semantics.

## Constraints to preserve

1. Kernel remains deterministic; no external execution inside kernel.
2. Receipts remain the state transition boundary.
3. Cap/policy checks remain pre-dispatch in kernel.
4. Internal effects (`workspace.*`, `introspect.*`, `governance.*`) remain kernel-handled.

## Already Implemented (do not redesign in P3)

Receipt handling is already significantly stricter than earlier docs assumed:

1. effect params are canonicalized before enqueue (`normalize_effect_params`),
2. receipt payloads are normalized against effect receipt schema before delivery,
3. invalid/delivery-failed receipts follow explicit fault handling (`EffectReceiptRejected` path or instance fail/settle).

References:

- `crates/aos-kernel/src/effects.rs:236`
- `crates/aos-kernel/src/world/runtime.rs:204`
- `crates/aos-kernel/src/world/runtime.rs:447`

## Remote Execution Status

There is no remote adapter runtime in the current system. P3 in this roadmap
slice does not require remote-provider implementation or pilot validation.

Remote-worker contracts are deferred to a future infra slice once transport and
worker runtime exist.

## Rollout Plan

## Phase 3.1: Route by `adapter_id` without behavior change

1. Add `adapter_id` route resolution layer (P2 integration).
2. Keep existing in-process adapters and legacy fallback behavior working.
3. Preserve current adapter timeout/error behavior.

Exit criteria:

1. all current smoke fixtures pass unchanged,
2. explicit route diagnostics exist.

## Phase 3.2: Hardening

1. optional strict mode: require explicit external bindings and disable legacy fallback,
2. operator-facing route diagnostics.

## Testing Strategy

1. Unit:
   - route resolution precedence,
   - missing-route diagnostics,
   - timeout/error mapping by adapter route.
2. Integration:
   - in-process execution path,
   - compatibility fallback and strict-mode behavior.
3. Replay:
   - replay invariance across route-configuration modes.
4. Policy/cap:
   - denied intents never leave host.

## Risks

1. Implicit fallback can mask configuration drift.
2. Route indirection can obscure operator debugging without clear diagnostics.

## Non-Goals

1. No changes to kernel receipt normalization/fault semantics.
2. No changes to workflow ABI effect model.
3. No remote worker/provider implementation in this slice.

## Deliverables / DoD

1. Host can resolve and dispatch by `adapter_id`.
2. P2 binding resolution integrated with compatibility fallback.
3. Existing strict receipt pipeline remains unchanged and replay-safe.
4. Startup compatibility failures (P1/P2) stay in place.

## Completion Notes (2026-02-28)

1. Host dispatch resolves external effects by manifest `adapter_id` binding first, with compatibility fallback to effect kind route when strict mode is off.
2. Optional strict routing mode is implemented via host config/env (`strict_effect_bindings`, `AOS_STRICT_EFFECT_BINDINGS`) and fails world-open when any external kind lacks an explicit manifest binding.
3. Startup preflight diagnostics now produce structured route state:
   - `world_requires` (`kind -> route`)
   - `host_provides` (`adapter_id -> adapter kind`)
   - `compatibility_fallback_kinds`
4. Operator-facing diagnostics are exposed in `trace-summary` output under `adapter_routes`.
5. Host profile defaults now include logical vault routes (`vault.put.default`, `vault.rotate.default`) and matching stub adapters are registered for rollout compatibility.
