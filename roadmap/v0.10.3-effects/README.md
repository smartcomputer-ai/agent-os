# v0.10.3 Effects

Focused roadmap slice for effect runtime clarity and adapter pluggability.

## Documents

- `roadmap/v0.10.3-effects/p1-current-state-and-gap-analysis.md`
- `roadmap/v0.10.3-effects/p2-manifest-effect-bindings-and-host-profiles.md`
- `roadmap/v0.10.3-effects/p3-adapter-pluggability-architecture-and-rollout.md`
- `roadmap/v0.10.3-effects/p4-implementation-task-breakdown.md`

## Scope

1. Clarify current effect/runtime contract (`manifest.effects`, internal vs external dispatch, adapter registry behavior).
2. Introduce manifest-level effect-to-adapter binding model without coupling `defeffect` to implementation details.
3. Define pluggable adapter execution architecture (in-process + remote worker + optional WASI plugin model) and rollout.

## Out of Scope

1. Changes to reducer/plan responsibility boundaries.
2. Removing cap/policy enforcement from kernel.
3. Replacing receipt-driven replay contract.
