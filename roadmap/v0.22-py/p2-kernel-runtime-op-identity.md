# P2: Kernel Runtime Op Identity Cut

Status: planned.

## Goal

Move kernel runtime identity to AIR v2 ops end to end.

After this phase, workflows, effect intents, open work, receipts, streams, continuations, snapshots,
and effect dispatch should be keyed by workflow op and effect op identity. The runtime should no
longer use module identity as workflow identity, or effect kind strings as effect identity.

## Non-Goals

- Do not implement Python workflow or Python effect execution.
- Do not preserve replay compatibility for old AIR v1 journals.
- Do not keep `manifest.effect_bindings` or semantic-kind dispatch as a fallback.

## Work

- Replace `LoadedManifest.effects` with an op-centered active manifest:
  - active schemas
  - active modules
  - active ops
  - active secrets
  - routing subscriptions
- Build explicit indexes:
  - op name/hash to `DefOp`
  - workflow op name to workflow contract
  - effect op name to effect contract
  - implementation module name/hash to runtime metadata
- Remove runtime lookups that ask `DefModule` for workflow ABI fields.
- Remove runtime lookups that ask `DefEffect` or effect kind for effect contract fields.
- Route domain events through `routing.subscriptions[].op`.
- Validate and deliver routing events against `DefOp.workflow.event`:
  - exact schema match delivers directly
  - variant-arm match wraps the event into the workflow event variant before invocation
- Store workflow state, cell indexes, in-flight metadata, continuation context, and trace identity by
  workflow op, not implementation module.
- Resolve `workflow_op.impl.module` and `workflow_op.impl.entrypoint` before invocation.
- Update the WASM runtime path so the invoked export comes from `impl.entrypoint`; `"step"` is not
  special.
- Update workflow context/ABI surfaces to carry origin workflow op identity and definition hash.
- Change workflow effect emission from effect kind to effect op name.
- Authorize emissions by checking the origin workflow op's `workflow.effects_emitted[]`.
- Resolve the effect op before params normalization.
- Canonicalize effect params using the effect op's `effect.params` schema.
- Validate receipts using the recorded effect op's `effect.receipt` schema.
- Redefine effect intent identity preimage to include:
  - origin workflow op name
  - origin workflow op definition hash
  - origin instance key
  - effect op name
  - effect op definition hash
  - canonical params
  - emission position
  - workflow-requested idempotency key
- Update open-work records, journal records, snapshots, replay restore, and quiescence checks to pin
  workflow op and effect op identity.
- Replace receipt and stream envelope schemas with v2 op-identity envelopes:
  - origin workflow op name/hash
  - effect op name/hash
  - executor module name/hash
  - executor entrypoint
  - receipt or stream payload
- Route receipt continuations by the recorded origin workflow op and pending intent identity, not by
  manifest routing.
- Dispatch effect execution from op implementation metadata:
  - `impl.module`
  - module `runtime.kind`
  - `impl.entrypoint`
  - active built-in/runtime registry
- Convert existing Rust/builtin adapters to internal registry entries keyed by builtin module plus
  entrypoint.
- Keep the durable append fence unchanged: opened async work is published only after the containing
  journal frame is durably flushed.
- Make Python runtime execution return a clear unsupported-runtime error if reached before the later
  Python implementation phase.

## Runtime Invariants

- Workflow identity is the workflow op, not the module.
- Effect identity is the effect op, not a semantic kind string.
- A single module may implement many workflow ops and many effect ops.
- Replacing one op does not imply replacing every op in the same module.
- Receipt binding and replay validation use the recorded effect op definition hash.
- Continuation routing is independent of domain routing subscriptions.
- Internal system paths may remain implementation details, but public AIR effect requests still enter
  through workflow op emissions.

## Main Touch Points

- `crates/aos-kernel/src/manifest.rs`
- `crates/aos-kernel/src/manifest_catalog.rs`
- `crates/aos-kernel/src/world/manifest_runtime.rs`
- `crates/aos-kernel/src/world/event_flow.rs`
- `crates/aos-kernel/src/world/runtime.rs`
- `crates/aos-kernel/src/world/snapshot_replay.rs`
- `crates/aos-kernel/src/workflow.rs`
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-kernel/src/receipts.rs`
- `crates/aos-kernel/src/journal.rs`
- `crates/aos-kernel/src/snapshot.rs`
- `crates/aos-effects/src/intent.rs`
- `crates/aos-wasm-abi/src/lib.rs`
- `crates/aos-wasm/src/lib.rs`
- `crates/aos-wasm-sdk`
- `crates/aos-node/src/worker`
- `crates/aos-node/src/execution/effect_runtime.rs`
- `crates/aos-effect-adapters/src/adapters`
- `crates/aos-sys`

## Done When

- Domain events route to workflow ops and invoke the export named by `impl.entrypoint`.
- Workflow snapshots, cell indexes, traces, and continuation records use workflow op identity.
- Workflows emit effect op names, and undeclared effect op emissions fail.
- Effect intents, open work, receipts, streams, and replay records pin effect op name/hash.
- Effect dispatch no longer reads `manifest.effect_bindings` or classifies semantic kind prefixes.
- Built-in workspace, timer, blob, HTTP, and governance/internal effect tests either pass under op
  identity or have a short known-fail list unrelated to old AIR v1 identity.
- New-journal replay from genesis remains byte-identical.
