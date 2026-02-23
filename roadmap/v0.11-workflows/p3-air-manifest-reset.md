# P3: AIR + Manifest Reset (Remove DefPlan and Trigger-to-Plan Model)

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.11-workflows/p2-kernel-plan-runtime-cutover.md`

## Goal

Remove plans as a first-class AIR concept and reset manifest wiring to module-only orchestration entrypoints.

This is the formal control-plane break where `defplan` and plan references disappear from type models, validators, schemas, and loaders.
Temporary between-phase breakage is expected and acceptable while executing P1 -> P5 serially.

## Hard-Break Assumptions

1. Old manifests containing `plans`/`triggers` are unsupported.
2. Old patch docs referencing plan/triggers operations are unsupported.
3. AIR semantic reset is allowed without strict version migration machinery.

## Scope

### 1) Remove plan model/types from AIR

1. Remove `DefPlan`, `PlanStep*`, `PlanEdge`, and plan enums from model definitions.
2. Remove plan-specific validation logic.
3. Remove plan literal normalization paths.

### 2) Replace manifest sections

1. Remove `manifest.plans` references.
2. Remove trigger-to-plan semantics.
3. Introduce/solidify module workflow subscription/routing model for orchestration start.
4. Keep receipt return routing out of manifest wiring; receipt routing is origin-identity-based runtime behavior.

### 3) Update schemas

1. Remove `spec/schemas/defplan.schema.json` from active AIR schema set.
2. Update `spec/schemas/manifest.schema.json` required fields and routing semantics.
3. Update `spec/schemas/patch.schema.json` to remove plan/triggers patch ops.
4. Add/align schema docs for the generic workflow receipt envelope contract.

### 4) Update storage + loaders

1. Remove plan node loading from catalog paths.
2. Remove manifest hashing/rewriting logic for plan refs.
3. Remove plan references from host world import/export serialization.

## Out of Scope

1. Governance/shadow summary model cleanup.
2. Trace/CLI UX cleanup beyond compile-required changes.
3. Final docs/tutorial rewrite.

## Work Items by Crate

### `crates/aos-air-types`

1. `model.rs`: remove plan structures and plan-oriented manifest fields.
2. `validate.rs`: remove plan and trigger checks; add module workflow wiring checks.
3. `schemas.rs` and tests: remove `defplan` references.

### `spec/schemas`

1. Update `manifest.schema.json`, `common.schema.json`, `patch.schema.json`.
2. Remove or archive `defplan.schema.json`.

### `crates/aos-store`

1. `manifest.rs`: remove plan node loading, normalization, and validation expectations.
2. Keep deterministic canonicalization behavior.

### `crates/aos-kernel` and `crates/aos-host`

1. Remove remaining plan refs in `manifest.rs`, `manifest_runtime.rs`, `manifest_loader.rs`, `world_io.rs`.
2. Ensure world open succeeds with the new manifest contract.

## Acceptance Criteria

1. AIR model compiles with no plan data structures.
2. No manifest schema field requires or accepts plan refs.
3. Loader/store/kernel paths no longer parse or depend on `defplan`.
4. World import/export and manifest patching work with the new contract.
5. Manifest routing changes cannot strand in-flight receipts.
