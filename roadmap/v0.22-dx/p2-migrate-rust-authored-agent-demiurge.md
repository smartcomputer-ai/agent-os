# P2: Migrate `aos-agent` And Demiurge To Rust-Authored AIR

Status: planned.

## Goal

Use the Rust-authored AIR lane from P1 for the main reusable agent package and the Demiurge world,
while keeping most smoke fixtures on the hand-authored AIR lane.

Primary outcome:

1. `aos-agent` contract schemas and `SessionWorkflow` definitions are generated from Rust.
2. `worlds/demiurge` defines its local schemas, workflow, module, manifest refs, and routing in
   Rust.
3. Demiurge imports `aos-agent` AIR through Cargo package discovery instead of custom
   import config.
4. The hand-authored AIR path remains exercised by most smoke fixtures.

## Design Stance

### 0) Use the procedural macro authoring surface from P1

The migrations should use the primary Rust DX:

1. `#[derive(AirSchema)]` on contract/state/event types,
2. focused `#[aos(...)]` field/type annotations for AIR names, refs, and special semantics,
3. `#[aos_workflow(...)]` on workflow structs for module/workflow/routing metadata.

Do not migrate `aos-agent` or Demiurge through a temporary hand-written schema macro format unless
the proc-macro implementation is blocked. These migrations are meant to validate the best-DX path.

### 1) `aos-agent` becomes the reusable Rust-authored package example

`aos-agent` should demonstrate how an ordinary Rust package exports reusable AOS contracts and a
workflow implementation.

Its Rust source should be the source of truth for:

1. contract schema definitions,
2. `aos.agent/SessionWorkflow@1`,
3. `aos.agent/SessionWorkflow_wasm@1`,
4. routing subscriptions for `SessionIngress`,
5. emitted effect allowlists.

Generated AIR may still be checked in temporarily during the migration, but CI should prove it is
derived from Rust and not edited by hand.

### 2) Demiurge becomes the world-level Rust-authored example

Demiurge should show the target developer experience:

1. define domain event/state types in Rust,
2. implement the reducer in Rust,
3. configure the workflow and routing beside the reducer,
4. depend on `aos-agent` normally through Cargo,
5. keep local config only for operational concerns such as secrets and workspace sync.

The target local config direction is described in `p3-world-config-and-air-discovery.md`: optional
`aos.world.json` for local build/sync/secrets, visible discovered package hashes in build/check
output, and no AIR lock file in the first version.

### 3) Smoke fixtures continue to protect hand-authored AIR

Most smoke fixtures should stay hand-authored. That is intentional coverage, not legacy debt.

Add only a small number of Rust-authored fixtures:

1. one minimal local workflow fixture,
2. optionally one fixture that imports a Rust-authored package.

The majority of fixtures should continue to catch regressions in authored JSON loading,
placeholder module patching, manifest ref rewriting, and low-level AIR diagnostics.

## Non-Goals

- Do not migrate all smoke fixtures.
- Do not remove `air/` directory support.
- Do not change kernel workflow/effect execution semantics.
- Do not introduce Python runtime support.
- Do not add custom effect authoring beyond emitted effect declarations.

## Phase 2A: Prepare `aos-agent` Contract Types

Annotate existing `aos-agent` contract types with `#[derive(AirSchema)]` and focused `#[aos(...)]`
metadata so generated AIR can match the current checked-in AIR.

Scope:

1. `SessionId`, `RunId`, `ToolBatchId`,
2. tool config and registry contracts,
3. session config/run config contracts,
4. lifecycle and host command contracts,
5. ingress and lifecycle domain events,
6. pending effect state contracts,
7. `SessionState`,
8. `SessionWorkflowEvent`.

Required migration rules:

1. preserve existing AIR names and versions,
2. preserve variant arm names,
3. preserve record field names,
4. preserve `time`, `hash`, and `uuid` semantics where current AIR uses them,
5. use explicit refs for all cross-schema relationships,
6. generate deterministic schema order for stable diffs.

Temporary compatibility rule:

```text
generated aos-agent AIR must hash-identically match the existing hand-authored definitions unless
there is an intentional AIR change reviewed as part of the migration.
```

## Phase 2B: Generate `aos-agent` Workflow And Package Exports

Move `SessionWorkflow` AIR metadata into Rust.

Rust-authored source should define:

1. workflow name: `aos.agent/SessionWorkflow@1`,
2. module name: `aos.agent/SessionWorkflow_wasm@1`,
3. state schema: `aos.agent/SessionState@1`,
4. event schema: `aos.agent/SessionWorkflowEvent@1`,
5. context schema: `sys/WorkflowContext@1`,
6. key schema: `aos.agent/SessionId@1`,
7. emitted effects,
8. `SessionIngress` routing with `key_field = "session_id"`.

This should be expressed through the `#[aos_workflow(...)]` authoring surface rather than separate
JSON or sync metadata.

Add package metadata to `crates/aos-agent/Cargo.toml` so downstream packages can discover these
exports.

Recommended validation:

```text
aos air generate --package aos-agent
aos air check --package aos-agent
cargo test -p aos-agent
cargo test -p aos-authoring
```

## Phase 2C: Switch `aos-agent` AIR Files To Generated Ownership

Choose one of two acceptable storage models.

Preferred final model:

```text
Rust source is authoritative.
Generated AIR is not checked in, except possibly golden test output.
```

Pragmatic migration model:

```text
Generated AIR is checked in under crates/aos-agent/air/.
CI fails if generated output differs.
Files include a short generated-file header where the parser allows it.
```

The pragmatic model is useful if downstream import tooling still expects a stable directory during
the transition.

Either way:

1. consumers must be able to import `aos-agent` through package metadata,
2. direct explicit import of `crates/aos-agent/air` should still work while smoke fixtures migrate,
3. generated definitions must pass the same import hash logic used today.

## Phase 2D: Migrate Demiurge Local AIR To Rust

Move Demiurge-local definitions from `worlds/demiurge/air` into Rust derives and annotations.

Rust-authored source should define:

1. `demiurge/TaskConfig@1`,
2. `demiurge/TaskSubmitted@1`,
3. `demiurge/TaskStatus@1`,
4. `demiurge/TaskFailure@1`,
5. `demiurge/PendingStage@1`,
6. `demiurge/TaskFinished@1`,
7. `demiurge/State@1`,
8. `demiurge/WorkflowEvent@1`,
9. `demiurge/Demiurge_wasm@1`,
10. `demiurge/Demiurge@1`,
11. Demiurge routing subscriptions.

The reducer code should stay recognizably local. This migration should remove external JSON glue,
not rewrite Demiurge behavior.

Required checks:

1. generated local schemas match the existing authored schemas unless intentionally changed,
2. emitted effects remain exactly declared,
3. routing still delivers `TaskSubmitted` and `SessionLifecycleChanged` to Demiurge,
4. receipt and stream continuation handling remains manifest-independent.

## Phase 2E: Remove Demiurge's Manual `aos-agent` AIR Import

After `aos-agent` package discovery works, remove Demiurge's manual AIR import wiring.

Current responsibilities that should remain:

1. secret sources and bindings,
2. workspace sync entries,
3. module pull behavior if still needed.

Responsibilities that should move out:

1. `air.imports` entry for `aos-agent`,
2. import lock payload for `aos-agent`,
3. local `air/manifest.air.json` refs that are derivable from Rust world metadata.

The local config file should become operational configuration, not the primary world definition.
Do not keep a backwards-compatibility phase for `aos.sync.json`; migrate remaining operational
settings directly to optional `aos.world.json`. AIR dependency identity should be visible in
build/check output; a dedicated lock file is deferred.

## Phase 2F: Keep Smoke Fixtures Mostly Hand-Authored

Do not bulk migrate `crates/aos-smoke/fixtures`.

Keep most fixtures as authored AIR JSON so they continue to cover:

1. manifest loading from `air/`,
2. placeholder module hash resolution,
3. explicit CLI/test-harness import overrides,
4. discovered AIR dependency/hash diagnostics,
5. hand-authored schema and workflow mistakes,
6. generated bundle/export/import behavior.

Add a small Rust-authored fixture only where it gives new coverage:

1. minimal Rust-authored counter or timer workflow,
2. optional package-discovery fixture that imports `aos-agent` through Cargo metadata.

## Phase 2G: Documentation And Examples

Update practical docs once migrations land.

Required docs:

1. `AGENTS.md` top-level authoring guidance,
2. Demiurge README,
3. `aos-agent` AIR export README or replacement note,
4. an authoring guide that explains:
   - when to use Rust-authored AIR,
   - when to use hand-authored AIR,
   - how to print generated AIR,
   - how to check generated AIR in CI,
   - how Cargo package discovery works.

## Testing

Required tests:

1. `aos-agent` generated AIR equals expected definitions,
2. `aos-agent` package metadata is discoverable,
3. Demiurge builds without manual `aos-agent` import config,
4. Demiurge replay/smoke behavior remains unchanged,
5. hand-authored smoke fixtures still pass,
6. explicit CLI/test-harness import overrides work for fixtures that need them.

Suggested target set:

```text
cargo test -p aos-air-types
cargo test -p aos-authoring
cargo test -p aos-agent
cargo test -p aos-kernel
cargo test -p aos-cli
cargo test -p aos-smoke
```

Use narrower targets while landing sub-phases, but P2 should close with the broader authoring and
smoke surface green.

## Exit Criteria

P2 is complete when:

1. `aos-agent` AIR is generated from Rust and exported through Cargo package metadata,
2. Demiurge's local world definition is Rust-authored,
3. Demiurge imports `aos-agent` through Cargo discovery,
4. Demiurge's local config no longer owns AIR import wiring for `aos-agent`,
5. most smoke fixtures remain hand-authored and continue to pass,
6. generated AIR is inspectable and CI-checkable.
