# P1: Kernel Refactor (aos-kernel Runtime Decomposition)

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (replay/snapshot correctness and maintainability continue to degrade)  
**Status**: In Progress

## Progress Update (2026-02-13)

- [x] Scope item 1 (`world.rs` split by runtime concern) is complete.
- [x] Scope item 5 (`internal_effects.rs` decomposition) is complete via `crates/aos-kernel/src/internal_effects/{mod,introspect,workspace,governance}.rs`.
- [x] Scope item 6 (`plan.rs` execution decomposition) is complete via `crates/aos-kernel/src/plan/{mod,step_handlers,codec,readiness,waits}.rs`.
- Decision: keep scenario-heavy `world` tests co-located in their relevant modules for now; moving them to `crates/aos-kernel/tests/` is not required for P1.

## Goal

Refactor `aos-kernel` to reduce structural complexity, isolate correctness-critical paths, and make replay/snapshot behavior easier to reason about and test.

Primary outcomes:

1. Reduce `world.rs` blast radius by splitting responsibilities into focused modules.
2. Remove duplicated runtime assembly logic (startup vs manifest swap).
3. Make replay/snapshot and tail semantics explicit and testable.
4. Decouple governance patch utilities from `world` internals.
5. Keep deterministic behavior and existing AIR/governance invariants intact.

## Non-Goals (P1)

- New distributed runtime semantics (handled by later infra milestones).
- New external adapter behavior.
- Policy model changes.
- Manifest schema changes unrelated to refactor.

## Scope (Now)

### 1) Split `world.rs` by runtime concern

Current file size is too large and mixes unrelated concerns.

Target layout:

- `crates/aos-kernel/src/world/mod.rs` (type definitions + public surface)
- `crates/aos-kernel/src/world/bootstrap.rs`
- `crates/aos-kernel/src/world/event_flow.rs`
- `crates/aos-kernel/src/world/plan_runtime.rs`
- `crates/aos-kernel/src/world/snapshot_replay.rs`
- `crates/aos-kernel/src/world/governance_runtime.rs`
- `crates/aos-kernel/src/world/query_api.rs`
- `crates/aos-kernel/src/world/manifest_runtime.rs` (router/schema/cap binding assembly)

Notes:

- Keep `Kernel<S>` as the owning type.
- Move impl blocks without changing external behavior.
- Preserve deterministic journal ordering and normalization boundaries.

### 2) Introduce a single runtime assembly path

`from_loaded_manifest_with_config` and `apply_loaded_manifest` currently rebuild overlapping runtime components separately.

Create a shared builder/input-output shape, e.g. `RuntimeAssembly`, that computes:

- schema index
- reducer schemas
- router
- capability resolver artifacts
- plan cap handles
- module cap bindings
- policy gate
- effect manager dependencies

Apply from both startup and manifest swap paths to reduce drift and fix bugs once.

### 3) Isolate snapshot/replay logic behind a dedicated module boundary

Move snapshot creation, validation, load, baseline promotion, and replay handling into `snapshot_replay.rs` with explicit invariants.

Required invariants to preserve:

- baseline promotion requires `receipt_horizon_height == snapshot.height`
- root completeness checks are fail-closed
- replay path remains deterministic (`baseline + tail` equivalence)

Add targeted integration tests for:

- exact manifest reads from snapshot height
- replay across manifest changes after baseline
- tail scan sequencing around baseline bootstrap

### 4) Extract governance/patch helpers out of `world`

`canonicalize_patch`, manifest ref normalization, and named-ref diff helpers should move into a governance-focused utility module.

Goals:

- remove `governance_effects -> world` dependency
- eliminate duplicated diff/ref helper logic
- keep patch canonicalization usable from both governance runtime and internal effects

### 5) Decompose `internal_effects.rs` dispatcher

Split internal effect handling into focused modules:

- `internal/introspect.rs`
- `internal/workspace.rs`
- `internal/governance.rs`
- `internal/mod.rs` (dispatch only)

Keep receipt encoding/status semantics unchanged.

### 6) Decompose `plan.rs` execution loop

Refactor `PlanInstance::tick` into per-step handlers and shared helper routines.

Suggested split:

- execution loop/state transitions
- step handlers (`assign`, `emit_effect`, `await_receipt`, `await_event`, `raise_event`, `end`)
- value/literal/cbor conversion codec helpers

This should reduce local complexity and make invariant violations easier to diagnose.

### 7) Test placement for `world` runtime

`world.rs` previously embedded a very large test block. After decomposition into `world/*`,
tests may remain co-located with the runtime concerns they validate.

For P1, co-located tests in `world/*.rs` are acceptable and preferred.
Moving these scenarios into `crates/aos-kernel/tests/` can be revisited later if needed.

## Execution Plan

1. Create `world/` module tree and move code by concern without behavior changes.
2. Introduce shared runtime assembly type and wire startup/swap through it.
3. Extract governance patch utilities and update imports.
4. Split internal effects dispatcher.
5. Split plan execution helpers.
6. Keep `world` tests co-located by concern (no required migration to integration tests in P1).
7. Run replay-or-die checks and confirm snapshot/tail invariants.

## Acceptance Criteria

- `world.rs` replaced by decomposed `world/` module tree.
- No duplicated runtime assembly logic between startup and manifest swap.
- Replay/snapshot/tail tests are green and deterministic.
- Governance patch preprocessing no longer depends on `world` module internals.
- Test placement is explicit and consistent (co-located `world` tests are acceptable for P1).

## Suggestions To Look Into (From Current Review)

1. Investigate and fix current `aos-kernel` red tests before or during refactor, especially replay/snapshot/tail failures.
2. Decide whether manifest apply should be blocked when there are in-flight plans/pending receipts/effect queue entries, or support safe migration semantics explicitly.
3. Avoid swallowing secret injection errors in `EffectManager::drain`; prefer explicit error surfaces for unresolved secret paths.
4. Replace panic paths in manifest/governance parsing (e.g., secret name parsing) with typed `KernelError` returns.
5. Revisit public submit APIs that currently discard routing/validation errors; consider returning `Result` consistently.
6. Review snapshot compatibility policy: strict fail-closed validation vs transitional compatibility for older snapshots.
7. Clarify journal height/tail scan semantics around baseline bootstrap to prevent off-by-one and bootstrap-edge regressions.
