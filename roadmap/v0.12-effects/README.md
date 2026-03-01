# v0.12 Effects

Focused roadmap slice for effect runtime clarity and adapter pluggability.

## Documents

- `roadmap/v0.12-effects/p1-current-state-and-gap-analysis.md`
- `roadmap/v0.12-effects/p2-manifest-effect-bindings-and-host-profiles.md`
- `roadmap/v0.12-effects/p3-adapter-pluggability-architecture-and-rollout.md`
- `roadmap/v0.12-effects/p4-host-sessions-effects.md`
- `roadmap/v0.12-effects/p5-blob-effects-cas-direct-and-journal-refs-only.md`
- `roadmap/v0.12-effects/p6-host-session-repo-io-effects-for-coding-agent-tools.md`

## Scope

1. Clarify current effect/runtime contract (`manifest.effects`, internal vs external dispatch, adapter registry behavior).
2. Introduce manifest-level effect-to-adapter binding model without coupling `defeffect` to implementation details.
3. Define in-process adapter pluggability by logical `adapter_id` routing, with remote execution deferred to future infra work.
4. Define essential `host.session` + `host.exec` effect contracts aligned with current workflow/effect runtime semantics.
5. Move blob-heavy effect paths to CAS-direct I/O with journal refs-only contracts for blob intents/receipts/events.
6. Add host-session-scoped repo I/O/search/edit effects required by coding-agent tool profiles.

## Out of Scope

1. Changes to workflow ABI responsibility boundaries.
2. Removing cap/policy enforcement from kernel.
3. Replacing receipt-driven replay contract.
