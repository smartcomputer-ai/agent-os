# Plan Reuse (Umbrella Decisions)

**Priority**: P1
**Status**: Active (split into two implementation tracks)  
**Date**: 2026-02-22

## Why this exists

Agent worlds are currently forced to duplicate full plan logic when contracts differ only at boundaries (event envelope shape, local routing expectations, app-specific wrappers). This slows migration to SDK-first patterns and increases drift risk.

The root cause is not just tooling. It is the interaction of:

1. AIR nominal typing,
2. envelope-specific event contracts,
3. lack of in-plan composition primitives in v1.0.

We are addressing this in two tracks, both staying on `air_version: "1"`.

## Decision: Split Scope into Two Roadmap Files

### Track A: Build-Time Reuse and Distribution (now)

Document: `roadmap/v0.10-agent-sdk/p3.3a-plan-import-reuse.md`

Focus:

1. Reuse plans from upstream crates/folders via `aos.sync.json` imports.
2. Standardize SDK plan-pack exports.
3. Define plan interface contracts for imported plans.
4. Keep merge/conflict behavior deterministic and strict.

Why first:

1. Immediate value with no new AIR runtime semantics.
2. Uses existing P3.1 import pipeline (`air.imports`).
3. Unblocks app migrations quickly.

### Track B: Runtime Composition and Typing Boundaries (now, after A baseline)

Document: `roadmap/v0.10-agent-sdk/p3.3b-plan-composition.md`

Focus:

1. Add trigger-level projection/filtering to reduce envelope wrappers.
2. Add explicit sub-plan primitives (`spawn_plan`, `await_plan`, `spawn_for_each`, `await_plans_all`).
3. Keep strict nominal typing and explicit adapters, not implicit structural coercion.

Why second:

1. This removes the remaining duplication structurally.
2. It is kernel/language work, so higher risk and larger blast radius.
3. Clearer after export/import interfaces are in place.

## Agreed Principles

1. Stay on AIR v1 for now (active development phase, breaking changes acceptable).
2. Keep determinism and replay guarantees as non-negotiable constraints.
3. Prefer explicit typed adaptation over implicit typing/polymorphism.
4. Treat imported plan cap slot names as stable API.
5. Keep world-local grants and policy decisions local.

## Recommendation Snapshot

1. Implement both tracks now, sequenced as A then B.
2. Do not introduce template macros as a runtime AIR feature in this phase.
3. Use import/distribution for immediate reuse; use composition primitives for long-term reuse.

## Deliverables

1. `p3.3a-plan-import-reuse.md`: detailed implementation plan for import-based plan reuse.
2. `p3.3b-plan-composition.md`: detailed language/runtime plan for composition and projection.
3. At least one fixture/app migrated to imported plan-pack usage.
4. Follow-on implementation tasks in kernel/validator/loader tracked against these docs.
