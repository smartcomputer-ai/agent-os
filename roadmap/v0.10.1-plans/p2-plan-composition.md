# P2: Plan Composition in AIR v1 (Projection + Subplans)

**Priority**: P2  
**Status**: In Progress (`B1` complete on 2026-02-22; `B2` pending)  
**Depends on**: P1 for distribution baseline  
**Source inspiration**: `roadmap/vX-future/p3-plans-v1.1.md`

## Context

After import-based reuse (P1), duplication still remains whenever app contracts use different envelopes.

Example class:

1. shared core plan expects `WorkspaceSyncRequested@1`,
2. app trigger stream provides `SessionEvent@1` with variant payload,
3. app duplicates full core orchestration to adapt boundaries.

This is a language-composition problem, not just a packaging problem.

## Core Language Design Tension (and our stance)

We are seeing the standard typed language problem set:

1. nominal contracts across module boundaries,
2. function composition across mismatched input/output envelopes,
3. import/reuse without implicit coercion.

### Agreed stance for AOS

1. Keep typing **strict and explicit** (no implicit structural coercions).
2. Keep adaptation points explicit and auditable.
3. Preserve deterministic replay and canonical encoding behavior.

This aligns with AOS aims better than "flexible magic" typing.

## Stay on AIR v1

Per agreement: remain on `air_version: "1"` while in active development.

Interpretation for this work:

1. We may add/adjust AIR semantics in v1 in place.
2. Migration of fixtures/worlds in-repo is acceptable.
3. Determinism and replay guarantees remain mandatory.

## Scope

### In scope

1. Trigger-level projection/filtering to remove envelope wrappers.
2. Subplan composition primitives from v1.1 design:
   - `spawn_plan`, `await_plan`,
   - `spawn_for_each`, `await_plans_all`.
3. Typed handle and journal linkage (`sys/PlanHandle@1`, parent-child relation).
4. Validation rules and failure semantics for composition steps.

### Out of scope

1. Runtime generics/polymorphism for plans.
2. Structural typing/coercion in kernel.
3. Template macro system.

## Part 1: Trigger Projection (solve envelope coupling early)

Add two optional fields to trigger binding semantics:

1. `when: Expr` (bool) evaluated against routed event value.
2. `input_expr: ExprOrValue` evaluated when `when` passes; result becomes plan input.

### Semantics

For each matching trigger entry:

1. Decode/canonicalize event as today.
2. Evaluate `when` (default true if omitted).
3. If false: no plan spawn.
4. If true: evaluate `input_expr` (default `@event` identity if omitted).
5. Validate resulting value against target plan input schema.
6. Spawn plan instance with canonical validated input.

### Why this matters

This directly removes boilerplate wrappers that only:

1. inspect event variant tags,
2. extract nested payload,
3. forward to shared plan.

It keeps adaptation explicit and typed at trigger boundary.

## Part 2: Subplan Composition Primitives

Adopt the model from `p3-plans-v1.1` with v1-compatible implementation.

### New steps

1. `spawn_plan`
2. `await_plan`
3. `spawn_for_each`
4. `await_plans_all`

### Supporting schema

1. `sys/PlanHandle@1` as opaque typed handle.

### Journal linkage

1. Extend plan-start journal record with optional `parent_instance_id`.

### Determinism constraints

1. Child inherits parent pinned manifest hash.
2. Spawn ordering and handle binding deterministic.
3. Await readiness based only on journaled plan end state.

## Typing Strategy (solve 1-3 without generics)

### 1) Envelope coupling

Solve with trigger projection (`when` + `input_expr`), not duplicated plan logic.

### 2) Nominal typing friction

Keep nominal typing strict:

1. Shared plan IO uses stable schema names.
2. Adaptation must be explicit at projection/wrapper boundary.
3. No implicit compatibility between "similar" records.

### 3) No polymorphism

Use explicit composition:

1. thin adapters at trigger boundary,
2. shared core plans called via `spawn_plan`,
3. typed result handling via `await_plan` variants.

This gives reuse while keeping runtime simple and auditable.

## Capability and Policy Considerations

Imported/shared plans can standardize cap slot names, but authority is still world-local.

Consumer world responsibilities remain:

1. bind slot names via `defaults.cap_grants`,
2. allow effects in policy for the relevant plan origins,
3. keep grants minimal and app-specific.

No ambient authority is introduced by composition.

## Implementation Plan

### Milestone B1: Trigger Projection

**Status**: Complete (2026-02-22)

Kernel/runtime:

1. [x] extend trigger model with `when` and `input_expr`,
2. [x] evaluate and type-check at plan-start boundary,
3. [x] add manifest/validator checks for expression refs and bool guard typing.

Tooling/tests:

1. [x] schema updates for trigger definition,
2. [x] loader round-trip tests,
3. [x] integration test replacing an envelope-wrapper plan with projection-only trigger.

### Milestone B2: Subplan Ops

Kernel/runtime:

1. implement new step handlers,
2. add plan instance parent linkage,
3. add await readiness over child plan end records,
4. preserve pinned manifest inheritance.

Validator/type system:

1. step reference checks for handles,
2. result variant inference checks,
3. fan-out/fan-in homogeneity checks (initially strict).

Tests:

1. deterministic replay with nested plan graphs,
2. failure propagation (`Error` variant),
3. fan-out barrier determinism,
4. invariant failure behavior compatibility.

## Migration Pattern

After B1+B2:

1. shared plan remains envelope-neutral in SDK pack,
2. app trigger projects local event into shared input,
3. optional local wrapper plan only when app-specific post-processing is required,
4. wrapper delegates to shared core via `spawn_plan`.

## Risks and Mitigations

1. **Kernel complexity growth**
   - Mitigation: ship B1 first, B2 second with strict test gates.
2. **Handle/type inference bugs**
   - Mitigation: conservative validation rules and narrow initial feature set.
3. **Semantic regressions in existing plans**
   - Mitigation: existing ops unchanged; new fields optional; replay regression suite mandatory.
4. **Overreach into generic language features**
   - Mitigation: explicitly defer polymorphism/templates.

## Validation Gates

1. Unit tests for new trigger and step semantics.
2. Integration tests for parent-child plan flows.
3. Replay parity tests with nested plans.
4. At least one real fixture migrated from duplicated wrapper plan to projected/shared composition pattern.

## Definition of Done

1. Trigger projection (`when`, `input_expr`) implemented and documented.
2. `spawn_plan`/`await_plan` implemented with deterministic replay guarantees.
3. `spawn_for_each`/`await_plans_all` implemented with initial strict typing constraints.
4. At least one duplicated plan flow replaced with shared plan + explicit composition.
