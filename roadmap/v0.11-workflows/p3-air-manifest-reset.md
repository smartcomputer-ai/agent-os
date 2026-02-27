# P3: AIR + Manifest Reset (Remove DefPlan and Trigger-to-Plan Model)

**Priority**: P1  
**Status**: Completed  
**Depends on**: `roadmap/v0.11-workflows/p2-kernel-plan-runtime-cutover.md`

## Implementation Status

### Scope
- [x] Scope 1.1: Removed active AIR `DefPlan`/`PlanStep*`/`PlanEdge`/`Trigger` from `model.rs` and active manifest fields.
- [x] Scope 1.2: Removed plan-specific manifest validation checks.
- [x] Scope 1.3: Removed plan literal normalization from active manifest load path.
- [x] Scope 1.4: Active module authority model uses `module_kind: workflow|pure`.
- [x] Scope 2.1: Removed `manifest.plans` references from active loader/kernel/host paths.
- [x] Scope 2.2: Removed trigger-to-plan manifest wiring and trigger patch ops.
- [x] Scope 2.3: Standardized on `routing.subscriptions` with compatibility aliases for `routing.events`/`reducer`.
- [x] Scope 2.4: Kept continuation routing out of manifest startup wiring.
- [x] Scope 3.1: Removed `defplan` from active AIR schema set.
- [x] Scope 3.2: Updated manifest schema for post-plan sections and routing contract.
- [x] Scope 3.3: Updated patch schema to remove plan/triggers operations.
- [x] Scope 3.4: Updated secret policy schemas to cap-only policy (`allowed_caps`) in active model.
- [x] Scope 3.5: Updated `defmodule` schema to `workflow|pure`.
- [x] Scope 4.1: Store/catalog no longer load `defplan` nodes into active manifest catalogs.
- [x] Scope 4.2: Removed manifest plan-ref hashing/rewriting logic from active loader/runtime paths.
- [x] Scope 4.3: Host world import/export serialization no longer reads/writes plan sections/files.

### Work Items by Crate
- [x] `crates/aos-air-types`: model/validation/schema set reset to post-plan active AIR contract.
- [x] `spec/schemas`: manifest/common/patch/defmodule/defsecret aligned with post-plan manifest and module model.
- [x] `crates/aos-store`: manifest loader and validation pipeline no longer depends on plan nodes.
- [x] `crates/aos-kernel`: removed remaining manifest/control-plane `defplan` dependencies in manifest/governance/patch/runtime assembly.
- [x] `crates/aos-host`: removed remaining manifest loader and world IO dependencies on plan refs.
- [x] `crates/aos-cli`: removed compile-required `manifest.plans`/`manifest.triggers`/`AirNode::Defplan` dependencies from active command wiring.

### Validation
- [x] `cargo check -p aos-air-types -p aos-store -p aos-kernel -p aos-host -p aos-cli`
- [x] `cargo check --tests -p aos-air-types -p aos-store -p aos-kernel -p aos-host -p aos-cli`

### Acceptance Criteria
- [x] AC1: Active AIR model compiles with no plan data structures in `model.rs`.
- [x] AC2: Active manifest schema/model no longer requires or consumes plan refs.
- [x] AC3: Loader/store/kernel active paths no longer parse or depend on `defplan`.
- [x] AC4: World import/export + manifest patching work with the post-plan contract.
- [x] AC5: Manifest routing changes do not own continuation wakeups (origin-identity runtime behavior retained).
- [x] AC6: Manifest fields are not required to recover waiting workflow runtime state.
- [x] AC7: Active AIR schema/model exposes `workflow|pure` module kinds.
- [x] AC8: Startup wiring uses `routing.subscriptions` (with compatibility aliases), not trigger-to-plan startup.

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
4. Replace reducer-era module authority vocabulary with `module_kind: workflow|pure` in active AIR model/schema.

### 2) Replace manifest sections

1. Remove `manifest.plans` references.
2. Remove trigger-to-plan semantics.
3. Introduce/solidify `routing.subscriptions` (replacing `routing.events`) as the workflow orchestration-start wiring model.
4. Keep continuation routing out of manifest wiring; receipt routing (and stream-frame routing if P7 is enabled) is origin-identity-based runtime behavior.

### 3) Update schemas

1. Remove `spec/schemas/defplan.schema.json` from active AIR schema set.
2. Update `spec/schemas/manifest.schema.json` required fields and routing semantics, including `routing.subscriptions` shape and deterministic ordering semantics.
3. Update `spec/schemas/patch.schema.json` to remove plan/triggers patch ops.
4. Add/align schema docs for the generic workflow receipt envelope contract.
5. If P7 is enabled, add/align schema docs for stream-frame envelope identity (`intent_id`, `seq`, `kind`, `payload`) and routing invariants.
6. Add/align schema docs for workflow instance state persistence (`status`, `inflight_intents`, `last_processed_event_seq`, optional `module_version`).
7. Update `defmodule` schema to target `workflow|pure` module kinds and keep `effects_emitted` as workflow allowlist.

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
2. `validate.rs`: remove plan and trigger checks; add module workflow wiring checks and `effects_emitted` allowlist semantics for workflow modules.
3. `schemas.rs` and tests: remove `defplan` references.

### `spec/schemas`

1. Update `manifest.schema.json`, `common.schema.json`, `patch.schema.json` for `routing.subscriptions` contract.
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
5. Manifest routing changes cannot strand in-flight continuations (receipts, and stream frames when P7 is enabled).
6. No manifest field is required to recover workflow waiting state; waiting state comes from persisted instance records.
7. Active AIR schema/model expose only `workflow|pure` module authority kinds in the post-plan model.
8. Manifest startup wiring uses `routing.subscriptions` and no longer relies on `routing.events`.
