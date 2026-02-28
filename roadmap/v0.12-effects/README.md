# v0.12 Effects

Focused roadmap slice for effect runtime clarity and adapter pluggability.

## Documents

- `roadmap/v0.12-effects/p1-current-state-and-gap-analysis.md`
- `roadmap/v0.12-effects/p2-manifest-effect-bindings-and-host-profiles.md`
- `roadmap/v0.12-effects/p3-adapter-pluggability-architecture-and-rollout.md`
- `roadmap/v0.12-effects/p4-process-sessions-effects.md`

## Scope

1. Clarify current effect/runtime contract (`manifest.effects`, internal vs external dispatch, adapter registry behavior).
2. Introduce manifest-level effect-to-adapter binding model without coupling `defeffect` to implementation details.
3. Define in-process adapter pluggability by logical `adapter_id` routing, with remote execution deferred to future infra work.
4. Define essential `process.session` + `process.exec` effect contracts aligned with current workflow/effect runtime semantics.

## Out of Scope

1. Changes to workflow ABI responsibility boundaries.
2. Removing cap/policy enforcement from kernel.
3. Replacing receipt-driven replay contract.
