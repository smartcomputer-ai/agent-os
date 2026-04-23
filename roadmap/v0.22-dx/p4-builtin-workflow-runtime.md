# P4: Builtin Workflow Runtime

Status: in progress.

Completed so far:

1. `crates/aos-kernel/src/workflow.rs` has been refactored into
   `crates/aos-kernel/src/workflow/{mod.rs,wasm.rs,builtin.rs}`.
2. `WorkflowRegistry` now dispatches by `defmodule.runtime.kind` while preserving the existing
   kernel-facing API and WASM execution path.
3. `sys/HttpPublish@1` has been removed from builtin AIR because the workspace HTTP surface no
   longer exists.

Remaining work starts at builtin reducer implementation in `workflow/builtin.rs`.

## Goal

Add a kernel-owned builtin workflow runtime alongside the existing WASM workflow runtime.

Primary outcome:

1. `defmodule.runtime.kind = "builtin"` can back `defworkflow` execution.
2. `sys/Workspace@1` runs as a kernel builtin instead of an `aos-sys` WASM binary.
3. Authoring no longer needs to discover, build, cache, or patch system workflow WASM modules.
4. The `aos-sys` crate can be removed after its shared payload structs move to an existing
   contract crate.
5. The retired `sys/HttpPublish@1` workflow, schemas, fixture routes, and `aos-sys`
   `http_publish` binary are removed instead of migrated.

This is a P4 item, not P5 cleanup, because it changes the workflow execution boundary and removes
the current system-WASM build dependency from normal authoring and runtime tests.

## Current State

AIR already models builtin modules:

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_effects@1",
  "runtime": { "kind": "builtin" }
}
```

However workflow execution is still WASM-only:

1. `crates/aos-kernel/src/world/event_flow.rs` resolves a workflow's module, calls
   `WorkflowRegistry::ensure_loaded`, and then invokes a WASM export.
2. `crates/aos-kernel/src/workflow.rs` stores only Wasmtime `InstancePre` values.
3. `crates/aos-kernel/src/module_runtime.rs` exposes only WASM hash helpers for workflow module
   versioning.
4. `spec/defs/builtin-workflows.air.json` points `sys/Workspace@1` at
   `sys/workspace_wasm@1`.
5. `crates/aos-authoring/src/build.rs` has special system module resolution that looks for built
   `aos-sys` WASM files and emits "build `aos-sys`" recovery hints.

The system workflow logic is small and deterministic today:

1. `crates/aos-sys/src/bin/workspace.rs` validates the workspace name, checks keyed routing,
   enforces `expected_head`, increments `latest`, and inserts commit metadata.

`sys/HttpPublish@1` is retired. The workspace HTTP publish surface no longer exists, so P4 should
delete that workflow and its schemas instead of migrating them to the builtin runtime.

## Target Shape

Workflow invocation remains the same at the kernel boundary:

```text
WorkflowInput -> WorkflowOutput
```

The runtime dispatch behind `WorkflowRegistry` becomes runtime-kind aware:

```text
defworkflow.impl.module
  -> defmodule.runtime.kind
  -> wasm backend | builtin backend | python backend later
```

No special cases should be added to `handle_workflow_event`. That code should still:

1. route and normalize the event,
2. load current workflow state,
3. build `WorkflowInput`,
4. invoke the workflow registry,
5. process `WorkflowOutput`.

Only the registry should know whether the implementation is WASM or builtin.

## Design

### 1. Refactor Workflow Runtime Into A Module Directory

Move the current top-level workflow runtime file into a submodule directory:

```text
crates/aos-kernel/src/workflow/
  mod.rs
  wasm.rs
  builtin.rs
```

Suggested ownership:

1. `workflow/mod.rs`: public `WorkflowRegistry` API and runtime dispatch.
2. `workflow/wasm.rs`: existing Wasmtime compile/cache/preinstantiate path.
3. `workflow/builtin.rs`: kernel builtin workflow selector and reducers.

Keep the kernel-facing API stable during the move.

### 2. Refactor `WorkflowRegistry` Into A Runtime Dispatcher

Keep the existing public kernel-facing calls:

```rust
pub fn ensure_loaded(
    &mut self,
    workflow_name: &str,
    module_def: &DefModule,
) -> Result<(), KernelError>

pub fn invoke_export(
    &self,
    workflow_name: &str,
    entrypoint: &str,
    input: &WorkflowInput,
) -> Result<WorkflowOutput, KernelError>
```

Internally, replace the WASM-only module map with a runtime-aware map.

Low-churn first implementation:

```rust
enum LoadedWorkflow {
    Wasm {
        module_name: String,
        wasm_hash: String,
        instance_pre: Arc<wasmtime::InstancePre<()>>,
    },
    Builtin {
        module_name: String,
    },
}
```

`ensure_loaded` behavior:

1. `ModuleRuntime::Wasm`: preserve the existing compile/cache/preinstantiate path.
2. `ModuleRuntime::Builtin`: verify that `(module_name, entrypoint)` resolves to a known builtin
   workflow reducer and cache a `LoadedWorkflow::Builtin`.
3. `ModuleRuntime::Python`: return a clear unsupported-runtime error for now.

`invoke_export` behavior:

1. `LoadedWorkflow::Wasm`: call the existing Wasmtime export path.
2. `LoadedWorkflow::Builtin`: call the builtin reducer registry.

This keeps Python as an obvious later extension without introducing a trait object hierarchy before
there is a second external runtime.

### 3. Add Builtin Workflow Reducers

`workflow/builtin.rs` should expose one narrow function:

```rust
pub(crate) fn invoke_builtin_workflow(
    module_name: &str,
    entrypoint: &str,
    input: &WorkflowInput,
) -> Result<WorkflowOutput, KernelError>
```

Initial supported selectors:

1. `("sys/workspace_builtin@1", "step")`

Each builtin should:

1. verify `input.version`,
2. decode `input.event.value` as the workflow event type,
3. decode `input.state` or use the default state,
4. apply the deterministic reducer,
5. encode the new state into `WorkflowOutput { state: Some(bytes), ..Default::default() }`.

The reducers should not emit effects or domain events in this phase. If a future builtin workflow
needs those, it should still return them through `WorkflowOutput` and let the existing kernel output
admission path enforce declarations.

### 4. Move Shared System Contract Types

Move the reusable system workflow payload structs out of `aos-sys`.

Preferred destination: `crates/aos-effect-types/src/workspace.rs` or a sibling module in
`aos-effect-types`.

Add missing types:

1. `WorkspaceVersion`
2. `WorkspaceCommitMeta`
3. `WorkspaceHistory`
4. `WorkspaceCommit`

Then update current users:

1. `crates/aos-cli/src/authoring.rs` should import `WorkspaceCommit` and
   `WorkspaceCommitMeta` from `aos-effect-types`.
2. builtin reducers should use the same structs.
3. duplicated private workspace state structs in kernel/node can be removed opportunistically when
   doing so does not widen the patch too much.

### 5. Update Builtin AIR Defs

Change `spec/defs/builtin-modules.air.json`:

```json
{
  "$kind": "defmodule",
  "name": "sys/workspace_builtin@1",
  "runtime": { "kind": "builtin" }
}
```

Change `spec/defs/builtin-workflows.air.json`:

```json
"impl": {
  "module": "sys/workspace_builtin@1",
  "entrypoint": "step"
}
```

Remove `sys/HttpPublish@1`, `sys/http_publish_wasm@1`, and the `sys/HttpPublish*` schema family.
The workspace HTTP publish surface no longer exists, so there is no replacement builtin workflow.

The `sys/Workspace@1` workflow name, event schema, state schema, key schema, and effect allowlist
stay unchanged.

### 6. Generalize Workflow Module Versioning

Replace workflow-specific WASM hash calls with a runtime-neutral helper.

Current behavior uses the WASM artifact hash as `module_version`. New behavior should be:

1. WASM workflow: module artifact hash.
2. Builtin workflow: canonical hash of the `defmodule`, or `None` if we decide that the versioned
   module name is enough.
3. Python workflow later: Python artifact root hash.

Recommendation: use the canonical `defmodule` hash for builtin workflows. It gives snapshots,
shadow reports, and diagnostics a stable version string without inventing a new version field.

### 7. Remove System WASM Resolution From Authoring

After builtin AIR defs point at builtin modules, `resolve_placeholder_modules` should only patch
WASM modules.

Remove:

1. `SysModuleSpec`
2. `SYS_MODULES`
3. `resolve_sys_module`
4. `resolve_sys_module_wasm_hash`
5. "build system modules with `cargo build -p aos-sys --target wasm32-unknown-unknown`" hints

`ModuleRuntime::Builtin` should never be treated as an unresolved placeholder.

### 8. Remove `aos-sys`

Once all users have moved to `aos-effect-types` and builtin reducers live in the kernel:

1. remove `crates/aos-sys`,
2. remove it from workspace members and workspace dependencies,
3. remove `aos-cli` / `aos-smoke` dependency entries,
4. remove tests that build `aos-sys` WASM,
5. update docs that mention building system workflow WASM,
6. remove the already-retired `http_publish` binary and any stale `sys/HttpPublish*` references.

This removal can be the final commit of P4 or a small P5 follow-up if the runtime migration is
large enough on its own.

## Tests

Minimum test coverage:

1. Unit tests for builtin `sys/Workspace@1`:
   - first commit creates version 1,
   - `expected_head` mismatch fails,
   - invalid workspace name fails,
   - keyed route key must match event workspace.
2. Kernel integration test where a manifest routes `sys/WorkspaceCommit@1` to
   `sys/Workspace@1` and no system WASM blob exists.
3. Replay test for builtin workflow state:
   - submit workspace commit,
   - snapshot or record journal,
   - replay from genesis,
   - assert byte-identical state/snapshot.
4. Authoring/build test proving builtin modules do not require placeholder WASM patching.
5. Existing workspace CLI/control tests continue to pass without building `aos-sys`.
6. Final smoke test with `crates/aos-smoke/fixtures/09-workspaces` to prove the workspace fixture
   runs through builtin `sys/Workspace@1` without `sys/HttpPublish@1`.

## Acceptance Criteria

1. A world can use `sys/Workspace@1` without any `aos-sys` WASM artifacts in `target/`,
   `.aos/cache/sys-modules`, or `<world>/modules`.
2. `cargo build -p aos-sys --target wasm32-unknown-unknown` is no longer required by normal tests,
   fixtures, or authoring flows.
3. `defmodule.runtime.kind = "builtin"` is accepted for workflow execution and rejects unknown
   builtin selectors with a typed kernel error.
4. WASM workflows continue to execute through the existing cache/preinstantiate path.
5. Snapshot/replay behavior remains deterministic for both WASM and builtin workflows.
6. The `aos-sys` crate is removed or has a short explicit follow-up with no runtime dependency.
7. `crates/aos-smoke/fixtures/09-workspaces` passes as the final end-to-end smoke check.
8. No builtin AIR, smoke fixture, or authoring path references `sys/HttpPublish@1` or
   `sys/http_publish_wasm@1`.

## Non-Goals

1. Python workflow execution.
2. Builtin effect runtime refactoring.
3. A public plugin API for third-party kernel builtins.
4. Changing workflow state/event schemas.
5. Changing effect declaration or receipt continuation semantics.
