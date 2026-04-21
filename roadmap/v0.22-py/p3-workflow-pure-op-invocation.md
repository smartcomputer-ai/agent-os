# P3: Workflow And Pure Invocation By Op

Status: planned.

## Goal

Move workflow routing and pure invocation from module identity to op identity.

This phase changes who the kernel considers to be the workflow. A workflow instance should be keyed by workflow op name, not by implementation module name.

## Work

- Change `routing.subscriptions` to route events to workflow ops.
- Build workflow schemas from `DefOp.workflow`.
- Store workflow state, cell indexes, inflight metadata, and receipt continuation context by workflow op name.
- Resolve `workflow_op.impl.module` before invoking the runtime.
- Update `WorkflowRegistry` so it can load the implementation module but invoke on behalf of an op.
- Update `PureRegistry` similarly for pure ops.
- Update workflow context fields:
  - replace or supplement `workflow` with workflow op identity
  - replace pure context `module` with pure op identity
- Update tests and fixtures that construct `RoutingEvent { module: ... }`.
- Update built-in workspace routing to use the workspace workflow op.

## Compatibility Decision

Do not maintain `routing.module` as accepted authoring syntax unless it is very cheap and temporary. This branch can break old manifests.

## Main Touch Points

- `crates/aos-kernel/src/world/manifest_runtime.rs`
- `crates/aos-kernel/src/world/event_flow.rs`
- `crates/aos-kernel/src/workflow.rs`
- `crates/aos-kernel/src/pure.rs`
- `crates/aos-wasm-abi/src/lib.rs`
- `crates/aos-wasm-sdk`
- `crates/aos-sys`
- `crates/aos-kernel/tests*`
- `crates/aos-node/tests`
- `crates/aos-smoke/fixtures`

## Invariants

- State identity follows the workflow op, not the bundle/module.
- A module can contain many workflow ops.
- Replacing one op should not imply replacing every op in the same module.
- Existing replay determinism rules still apply for WASM workflow ops.

## Done When

- Domain events route to workflow ops.
- Workflow state snapshots and cell indexes use op identity.
- Built-in workspace e2e tests pass with op routing.
- Pure invocation works through pure ops.

