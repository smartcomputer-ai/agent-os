# P3: Adapter Pluggability Architecture and Rollout

**Priority**: P3  
**Status**: Proposed  
**Date**: 2026-02-22

## Goal

Define a pluggable adapter architecture that supports:

1. local/native adapters,
2. cloud-hosted adapters running outside the world host process,
3. deterministic replay guarantees (receipts remain boundary),
4. future coding-agent effects from `p4-agent-effects`.

Reference driver:

- `roadmap/v0.10-agent-sdk/p4-agent-effects.md`

## Constraints to preserve

1. Kernel determinism: never execute external nondeterministic work in kernel.
2. Replay contract: world state evolution uses journal + recorded receipts.
3. Cap/policy authority remains kernel-side pre-dispatch.
4. Adapter identity and receipt payload remain auditable.

## Adapter Execution Models

Support three models behind one route abstraction:

1. `InProcessAdapter`:
   - current `AsyncEffectAdapter` pattern in `aos-host`.
   - best for local dev and simple deployments.
2. `RemoteWorkerAdapter`:
   - host forwards intent to durable queue/RPC worker.
   - worker returns receipt envelope.
   - best for cloud scale and operational isolation.
3. `WasiPluginAdapter` (optional/future):
   - sandboxed plugin execution for constrained adapters.
   - useful for portable plugin logic, not mandatory for cloud-native Rust.

No model changes reducer/plan AIR semantics.

## Proposed Host Abstraction

Introduce adapter provider interface:

```rust
trait AdapterProvider {
    fn id(&self) -> &str;
    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt>;
}
```

Registry becomes `adapter_id -> AdapterProvider`, then `kind -> adapter_id`
comes from P2 binding resolution.

This replaces direct `kind -> adapter` coupling while preserving existing
adapter implementations.

## Remote Worker Contract (minimum)

Intent dispatch payload:

1. `intent_hash`
2. `kind`
3. `cap_name`
4. `params_cbor`
5. `idempotency_key`
6. optional correlation metadata

Receipt ingress payload:

1. `intent_hash`
2. `adapter_id`
3. `status`
4. `payload_cbor`
5. `cost_cents?`
6. `signature`

Worker requirements:

1. at-least-once processing + dedupe by `intent_hash`,
2. terminal receipt emission,
3. bounded retry with explicit timeout semantics.

This aligns with existing infra queueing direction in `roadmap/infra.md`.

## Signature and trust model

Today many adapters use stub signatures. For hosted pluggability:

1. define adapter key material and signing policy per provider id,
2. host verifies signatures before receipt ingress (or records unsigned with
   explicit status in non-production mode),
3. keep adapter id stable across retries for audit consistency.

## Rollout Plan

## Phase 3.1: Internal refactor without behavior change

1. Add `adapter_id` route resolution layer (P2 integration).
2. Wrap existing in-process adapters as providers.
3. Keep existing defaults working.

Exit criteria:

1. all current smoke fixtures pass unchanged,
2. explicit route diagnostics exist.

## Phase 3.2: Remote worker provider for one effect family

Start with `llm.generate` or `exec.shell`:

1. host provider sends intents to worker queue,
2. worker executes provider client and emits receipt,
3. host ingresses receipt via existing `handle_receipt` path.

Exit criteria:

1. parity tests between in-process and remote-provider route,
2. deterministic replay from journal receipts remains identical.

## Phase 3.3: Expand to P4 coding effects

Add adapters for:

1. `exec.shell`,
2. `build.rust_wasm`,
3. optional `workspace.*` high-throughput operations that remain kernel-internal
   if deterministic.

This step depends on the finalized effect defs/caps from P4.

## Phase 3.4: Strict mode

1. require explicit route binding for every external effect in production mode,
2. disable legacy fallback mapping,
3. enforce signature policy for remote providers.

## Testing Strategy

1. Unit:
   - route resolution and precedence,
   - missing-route diagnostics,
   - provider registry behavior.
2. Integration:
   - in-process provider execution path,
   - remote-provider happy path + retry + timeout.
3. Replay:
   - journal equivalence checks across provider modes (given same receipt stream).
4. Policy/cap:
   - ensure denied intents never leave host.

## Operational Notes

1. Record adapter provider metrics by `adapter_id` and `kind`.
2. Include route and provider information in trace surfaces.
3. Add health checks for remote providers and fail world-open if required routes
   are unavailable.

## Risks

1. Implicit fallback can mask configuration drift.
2. Multiple routing layers can obscure operator debugging without clear logs.
3. Signature rollout may break existing tests unless phased with compatibility
   mode.

## Non-Goals

1. No change to reducer micro-effect boundary.
2. No business logic in adapters/plans beyond orchestration.
3. No mandatory WASI dependency for cloud adapter execution.

## Deliverables / DoD

1. Provider-based adapter architecture merged.
2. Route resolution from manifest bindings + host profiles merged.
3. At least one remote-provider effect family running in smoke/e2e path.
4. Startup compatibility failures replace runtime `adapter.missing` surprises.
5. Replay invariants preserved.

