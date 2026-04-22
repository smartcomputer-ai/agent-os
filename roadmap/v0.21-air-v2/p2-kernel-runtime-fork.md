# P2: Forked Workflow/Effect Runtime Cut

Status: implemented.

## Goal

Move kernel runtime, durable records, node/control surfaces, fixtures, and downstream crates from the
temporary op-centered model to the forked workflow/effect model introduced in P1.

After this phase, runtime semantics should use workflows and effects as distinct concepts wherever
they differ:

```text
workflow identity -> workflow definition name/hash
effect identity   -> effect definition name/hash
```

Shared implementation machinery may remain private and generic, but public and durable terminology
should not expose `defop` or `op_kind`.

## Starting Point

P1 should have already updated:

- active JSON schemas
- built-in definition shelf
- `aos-air-types`
- `aos-authoring`
- kernel manifest/governance loading and patching
- ambient `sys/*` definition resolution

P2 assumes those control-plane definitions exist and focuses on making the runtime executable again.

## Non-Goals

- Do not bring back `effect_kind`, `manifest.effect_bindings`, `routing.module`, `routing.inboxes`,
  public caps, or public policies.
- Do not add a migration layer for old AIR v1 or temporary `defop` journals.
- Do not implement Python workflow/effect execution unless the runtime already reaches an explicit
  unsupported-runtime branch.
- Do not broaden authority beyond `defworkflow.effects_emitted[]` and node-local runtime policy.

## Runtime Model

The kernel should stop treating workflows and effects as variants of one public op. Internally,
differentiate them by default:

```text
DefWorkflow -> workflow state, routing, keyed cells, continuations
DefEffect   -> effect params, execution dispatch, receipts, streams
```

Small shared helpers are still useful:

```text
Impl
ResolvedImpl
RuntimeSupport
DefinitionHash
```

Those helpers should not reintroduce a public or durable generic op vocabulary.

## Kernel World Runtime

- [x] Replace world runtime indexes:
  - `workflow_defs: HashMap<Name, DefWorkflow>`
  - `effect_defs: HashMap<Name, DefEffect>`
  - module/runtime indexes for shared `Impl` resolution
- [x] Update bootstrap/open/replay code to load workflow/effect maps from `LoadedManifest`.
- [x] Update domain routing:
  - route through `routing.subscriptions[].workflow`
  - validate delivery against `DefWorkflow.event`
  - keyed routing uses `DefWorkflow.key_schema`
  - variant-arm wrapping remains unchanged semantically
- [x] Update workflow invocation:
  - resolve `DefWorkflow.impl.module`
  - call `DefWorkflow.impl.entrypoint`
  - workflow context carries workflow definition identity and hash
  - no special `"step"` or `"workflow"` entrypoint value
- [x] Update workflow state storage:
  - instance identity is workflow name/hash
  - keyed cell indexes are keyed by workflow identity
  - trace/audit identity says workflow, not op
- [x] Keep strict quiescence semantics unchanged.

## Effects And Continuations

- [x] Update workflow effect emission:
  - emitted request names an effect definition
  - admission checks `DefWorkflow.effects_emitted[]`
  - resolve params schema from `DefEffect.params`
- [x] Update effect intent identity preimage:
  - origin workflow name
  - origin workflow definition hash
  - origin instance key
  - effect name
  - effect definition hash
  - canonical params
  - emission position
  - workflow-requested idempotency key
- [x] Update open-work records:
  - store origin workflow identity/hash
  - store effect identity/hash
  - store executor module/hash/entrypoint
- [x] Update receipt and stream admission:
  - validate payloads through `DefEffect.receipt`
  - route continuations by recorded origin workflow and pending intent identity
  - keep continuation routing independent of domain routing subscriptions
- [x] Update async effect dispatch:
  - resolve `DefEffect.impl`
  - dispatch from module runtime plus entrypoint
  - keep publication after durable flush
  - keep internal deterministic effects on the same intent/receipt path

## Durable Records And Replay

- [x] Rename durable public fields where practical:
  - `workflow_op` -> `workflow`
  - `workflow_op_hash` -> `workflow_hash`
  - `effect_op` -> `effect`
  - `effect_op_hash` -> `effect_hash`
- [x] Update journal records for new journals.
- [x] Update snapshots and replay restore for new journals.
- [x] Update receipt and stream envelope built-in schemas.
- [x] Update audit/provenance records to use workflow/effect terminology.
- [x] Preserve replay-or-die for new forked AIR v2 journals.
- [x] Do not preserve replay compatibility for temporary `defop` journals unless a later decision
  explicitly requires it.

## Node, CLI, And Query Surfaces

- [x] Update node manifest summaries:
  - workflow count
  - effect count
  - route summaries target workflows
- [x] Update hot world/control read models that expose op-centered fields.
- [x] Update CLI rendering:
  - list/show `defworkflow`
  - list/show `defeffect`
  - remove generic op counts from user-facing output
  - route summaries say workflow
- [x] Update governance summaries:
  - report workflow definition changes
  - report effect definition changes
  - report routing changes by workflow target
- [x] Keep any temporary compatibility aliases clearly marked and remove them before P2 is done.

## Fixtures And Downstream Crates

- [x] Convert active fixtures and examples:
  - smoke fixtures
  - kernel tests
  - node tests
  - authoring integration fixtures
  - agent AIR fixtures
  - eval fixtures
- [x] remove `sys/` references in manifests of fixtures, since sys is always included, where they are not helpful for clarity
- [x] Update SDK/helper crates that construct effect requests or consume receipt envelopes.
- [x] Update effect adapters only where start context or audit fields changed.
- [x] Update agent/session code only after kernel/node surfaces settle.
- [x] Sweep active fixture paths for:
  - `defop`
  - `op_kind`
  - `manifest.ops`
  - `routing.subscriptions[].op`
  - `workflow_op`
  - `effect_op`

Historical roadmap notes may retain these terms if they are clearly marked as temporary or removed
behavior.

## Verification

Recommended staged verification:

```text
cargo test -p aos-air-types
cargo test -p aos-kernel --lib
cargo test -p aos-kernel --tests
cargo test -p aos-authoring
cargo test -p aos-node --tests --no-run
cargo test -p aos-cli
cargo test -p aos-agent
cargo run -p aos-smoke -- all
```

Full workspace testing can wait until the active fixture migration is complete.

## Done When

- Kernel world runtime executes forked `defworkflow` definitions.
- Workflows emit forked `defeffect` definitions and admission checks `effects_emitted[]`.
- Effect intents, open work, receipts, streams, snapshots, and replay records use workflow/effect
  identity and hashes.
- Node/control/CLI user-facing surfaces no longer expose generic op terminology.
- Active fixtures no longer depend on `defop`, `op_kind`, `manifest.ops`, or route target `op`.
- New forked AIR v2 replay from genesis is byte-identical.
