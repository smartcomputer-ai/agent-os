# P4: Builtin Workflow Runtime

Status: implemented for the v0.22 DX slice.

## Outcome

P4 moved the deterministic system workspace workflow out of system WASM and into the kernel
workflow runtime.

Primary changes:

1. `defmodule.runtime.kind = "builtin"` can back `defworkflow` execution.
2. `sys/Workspace@1` runs through the kernel builtin module `sys/builtin_workspaces@1`.
3. Authoring no longer discovers, builds, caches, or patches system workflow WASM modules.
4. Shared workspace commit/history contract types live in `aos-effect-types`.
5. `aos-sys` was removed from the workspace.
6. The retired `sys/HttpPublish@1` workflow, schemas, smoke fixture route, and binary were removed.

This is P4 rather than P5 cleanup because it changes the workflow execution boundary and removes
the previous system-WASM build dependency from authoring and runtime tests.

## Implemented Shape

Workflow invocation still uses the same kernel boundary:

```text
WorkflowInput -> WorkflowOutput
```

Runtime dispatch now happens behind `WorkflowRegistry`:

```text
defworkflow.impl.module
  -> defmodule.runtime.kind
  -> wasm backend | builtin backend | python backend later
```

`handle_workflow_event` still performs routing, normalization, state loading, input construction,
registry invocation, and output admission. The runtime distinction is contained inside the workflow
registry.

## Code Layout

The workflow runtime now lives under:

```text
crates/aos-kernel/src/workflow/
  mod.rs
  wasm.rs
  builtin.rs
```

Ownership:

1. `workflow/mod.rs`: public `WorkflowRegistry` API and runtime-kind dispatch.
2. `workflow/wasm.rs`: existing Wasmtime compile/cache/preinstantiate path.
3. `workflow/builtin.rs`: kernel builtin workflow selector and deterministic reducers.

## Runtime Dispatch

`WorkflowRegistry` keeps the existing kernel-facing calls:

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

Behavior by module runtime:

1. `ModuleRuntime::Wasm`: uses the existing WASM backend and cache path.
2. `ModuleRuntime::Builtin`: validates and dispatches to a known kernel builtin reducer.
3. `ModuleRuntime::Python`: returns an unsupported-runtime error until Python workflows exist.

The initial builtin selector is:

```text
("sys/builtin_workspaces@1", "step")
```

Unknown builtin selectors fail as kernel manifest/runtime errors instead of falling back to WASM.

## Workspace Builtin

`sys/Workspace@1` is still the public workflow name. Its implementation module is now
`sys/builtin_workspaces@1`.

The builtin reducer:

1. verifies workflow ABI input version,
2. decodes `WorkspaceCommit`,
3. decodes existing `WorkspaceHistory` or starts from the default state,
4. validates workspace name and keyed-route key,
5. enforces `expected_head`,
6. increments `latest`,
7. stores `WorkspaceCommitMeta`,
8. returns the new canonical state in `WorkflowOutput.state`.

The reducer does not emit effects or domain events in this slice. Future builtin workflows should
still return effects/events through `WorkflowOutput` so the existing kernel admission path enforces
declarations and origin rules.

## Contract Types

Workspace system contract structs moved to `crates/aos-effect-types/src/workspace.rs`:

1. `WorkspaceVersion`
2. `WorkspaceCommitMeta`
3. `WorkspaceHistory`
4. `WorkspaceCommit`

Current users import these from `aos-effect-types`; `aos-sys` is no longer needed as a shared
contract crate.

## AIR Defs

Builtin AIR now declares:

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_workspaces@1",
  "runtime": { "kind": "builtin" }
}
```

`sys/Workspace@1` points at:

```json
"impl": {
  "module": "sys/builtin_workspaces@1",
  "entrypoint": "step"
}
```

`sys/Workspace@1` keeps its workflow name, event schema, state schema, key schema, and effect
allowlist. `sys/HttpPublish@1`, `sys/http_publish_wasm@1`, and the `sys/HttpPublish*` schema
family were deleted because the workspace HTTP publish surface no longer exists.

## Module Versioning

Workflow module versioning is runtime-neutral:

1. WASM workflow: module artifact hash.
2. Builtin workflow: canonical hash of the `defmodule`.
3. Python workflow later: Python artifact root hash.

This keeps snapshots, shadow reports, and diagnostics stable without inventing a separate builtin
version field.

## Authoring

`resolve_placeholder_modules` only patches unresolved WASM modules now.

Removed authoring paths:

1. system module specs,
2. system module cache lookup,
3. system WASM hash resolution,
4. recovery hints that asked users to build system workflow WASM.

`ModuleRuntime::Builtin` is no longer treated as an unresolved placeholder.

## Verification

The implementation was verified with:

1. `cargo test -p aos-air-types -p aos-kernel`
2. `cargo check -p aos-authoring -p aos-cli -p aos-smoke -p aos-agent-eval`
3. `cargo test -p aos-kernel --features e2e-tests --test workspace_e2e`
4. `cargo run -p aos-smoke -- workspaces`

The final smoke path uses `crates/aos-smoke/fixtures/09-workspaces` and runs through builtin
`sys/Workspace@1` without `sys/HttpPublish@1`.

## Acceptance Criteria

1. A world can use `sys/Workspace@1` without any system WASM artifacts in `target/`,
   `.aos/cache/sys-modules`, or `<world>/modules`.
2. Building an `aos-sys` WASM target is no longer required by tests, fixtures, or authoring flows.
3. `defmodule.runtime.kind = "builtin"` is accepted for workflow execution.
4. WASM workflows continue to execute through the existing cache/preinstantiate path.
5. Snapshot/replay behavior remains deterministic for WASM and builtin workflows.
6. `aos-sys` is removed from workspace membership and crate dependencies.
7. `crates/aos-smoke/fixtures/09-workspaces` passes as the final end-to-end smoke check.
8. Builtin AIR, smoke fixtures, and authoring paths no longer reference `sys/HttpPublish@1` or
   `sys/http_publish_wasm@1`.

## Non-Goals

1. Python workflow execution.
2. Builtin effect runtime refactoring.
3. A public plugin API for third-party kernel builtins.
4. Changing workflow state/event schemas.
5. Changing effect declaration or receipt continuation semantics.
