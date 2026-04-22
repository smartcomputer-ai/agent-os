# P3: Repository Convergence And AIR v2 Fixtures

Status: in progress. Fixture/test-common convergence for kernel, node, authoring, smoke, and
agent-session paths is complete; repo-wide spec/CLI cleanup remains.

## Goal

Finish the AIR v2 cut across specs, fixtures, CLI surfaces, examples, and tests after P1 and P2 make
the model executable.

This phase should remove stale AIR v1 forms from the repository and make the default developer path
exercise the op-centered runtime.

## Non-Goals

- Do not start the Python workflow/effect implementation here.
- Do not keep historical AIR v1 examples as loadable fixtures.
- Do not add a migration layer for old manifests or journals.

## Work

- [x] Convert checked-in AIR manifests and fixtures to AIR v2:
  - `manifest.ops`
  - `manifest.secrets`
  - `routing.subscriptions[].op`
  - workflow `effects_emitted[]`
  - module runtime/artifact declarations
  - effect ops with `params`, `receipt`, and `impl`
- [x] Remove checked-in `defeffect` fixtures and examples from active smoke/agent fixture paths.
- [x] Remove `manifest.effects`, `manifest.effect_bindings`, `routing.module`, and `routing.inboxes`
  from active fixtures.
- [x] Convert built-in SDK support schemas:
  - effect intent request payloads name an effect op
  - receipt envelopes use origin workflow op and effect op identity
  - stream frames use origin workflow op and effect op identity
  - executor identity replaces adapter-only identity where durable audit records need it
- [x] Update workflow SDK helpers and examples to emit effect op names.
- [x] Update smoke fixtures:
  - workspace demos
  - timer demos
  - blob demos
  - HTTP demos
  - Fabric demos
  - agent demos
- [x] Update CLI and query rendering:
  - list/show `defop`
  - show workflow op and effect op counts
  - remove effect binding output
  - route summaries name target ops
  - governance summaries report `defop` changes
- [x] Update specs to match the canonical target:
  - `spec/03-air.md`
  - `spec/04-workflows.md`
  - `spec/05-effects.md`
  - schema and built-in reference shelves
- [ ] Remove roadmap files made stale by the three-phase cut.
- [ ] Add focused regression tests for:
  - schema validation
  - built-in AIR loading
  - patch v2 operations
  - routing exact-event delivery
  - routing variant-arm delivery
  - keyed workflow routing
  - workflow op invocation through named WASM entrypoints
  - effect op emission authorization
  - effect op intent hashing
  - receipt continuation routing by recorded origin identity
  - replay determinism under AIR v2 journals
- [ ] Keep a short explicit known-fail list only for deferred Python runtime execution.

## Progress

- [x] Kernel test fixtures now synthesize canonical AIR v2 `DefModule` + `DefOp` manifests while
  preserving ergonomic test helpers.
- [x] Node integration fixture loader now converts authored workflow fixtures into v2 modules and
  workflow ops, and node tests compile against `ops`/`routing.op`.
- [x] Smoke fixture AIR manifests now use `air_version = "2"`, `manifest.ops`, workflow op routing,
  and distinct runtime WASM module names such as `*_wasm@1`.
- [x] Smoke workflow sources emit canonical effect ops and consume `effect_op` receipt/stream
  envelope fields.
- [x] `aos-smoke` harnesses patch runtime module hashes through workflow op implementation metadata.
- [x] `ExampleHost` classifies external effects through configured `EffectRuntime` routes, so
  built-in effect ops such as `sys/llm.generate@1` dispatch through adapters instead of being
  misclassified by module identity alone.
- [x] `aos-authoring` system module resolution uses v2 runtime module identities
  `sys/workspace_wasm@1` and `sys/http_publish_wasm@1`.
- [x] `aos-agent` AIR and workflow helpers emit canonical effect ops, and local plus live agent
  smoke examples run end to end.
- [x] Active smoke/agent manifest fixtures no longer contain `manifest.effects`,
  `manifest.effect_bindings`, `routing.module`, or `routing.inboxes`.
- [x] CLI/query rendering cut completed for active world defs/state paths and agent inspect-world
  output:
  - `aos world defs` help now names `op`/`secret` instead of removed cap/effect/policy kinds.
  - `aos world state ls` enumerates workflow ops instead of workflow modules.
  - bundle upload sends `DefOp` nodes and resolves WASM hashes from v2 module runtime artifacts.
  - agent inspect-world reports `op_count`/`ops` and no longer emits `effects` or
    `effect_bindings`.
- [x] Final CLI/query rendering items are complete:
  - `/v1/worlds/{world}/manifest` includes an op-centered summary with total ops, workflow op
    count, effect op count, and route summaries keyed by target `op`.
  - `aos world patch` and `aos world gov propose --patch-file <json>` include a patch summary that
    reports `defop` changes and routing targets by `op`.
  - Focused tests cover manifest op counts, routing summaries, and patch `defop` change summaries.
- [x] Scoped active-surface sweep is clean for `effect_bindings`, `manifest.effects`,
  `routing.module`, `routing.inboxes`, `defeffect`, `effect_kind`, `effect kind`, and
  `strict_effect_bindings` across CLI/node/agent/smoke/authoring paths touched by this cut.
- [x] Spec cleanup completed:
  - `spec/03-air.md` now describes AIR v2 root forms, `defop`, `manifest.ops`, op-based routing,
    effect op identity, v2 patch operations, and op-centered journal records.
  - `spec/04-workflows.md` now describes workflow ops, `workflow.effects_emitted`, keyed workflow
    routing by `routing.subscriptions[].op`, and op-based receipt continuation routing.
  - `spec/05-effects.md` now describes effect ops, op-based intent hashing/admission, executor
    identity, and durable open-work semantics.
  - `spec/01-overview.md`, `spec/02-architecture.md`, and `spec/README.md` were aligned to AIR v2
    terminology.
  - Active JSON Schemas and built-in defs were validated as JSON; the built-in defs shelf is clean
    for `effect_kind`, `defeffect`, `manifest.effects`, `effect_bindings`, `routing.module`, and
    `routing.inboxes`.
- [ ] Repo-wide naming cleanup remains: older roadmap notes, compatibility crates, SDK-local
  pending-effect fields, and some durable receipt types/tests still use transitional terms or
  compatibility fields; durable receipt records still carry `adapter_id` where that is still part of
  the current effect receipt type.
- [x] Verification completed for this convergence slice:
  - `cargo build -p aos-sys --target wasm32-unknown-unknown`
  - `cargo run -p aos-smoke -- all`
  - `cargo run -p aos-smoke -- all-agent`
  - `cargo test -p aos-agent`
  - `cargo test -p aos-authoring`
  - `cargo test -p aos-kernel --tests`
  - `cargo test -p aos-node --tests --no-run`
  - `cargo test -p aos-smoke --no-run`
- [x] Verification completed for the CLI/query rendering cut:
  - `cargo test -p aos-cli`
  - `cargo test -p aos-kernel governance_effects::tests::patch_summary_reports_defop_changes_and_routing_subscription_section`
  - `cargo test -p aos-node manifest_summary_counts_workflow_and_effect_ops_and_routes_by_op`
  - `cargo test -p aos-node --tests --no-run`
  - `cargo test -p aos-agent`
  - `cargo run -p aos-smoke -- all`
  - `cargo run -p aos-smoke -- agent-tools`
- [x] Verification completed for the spec cleanup cut:
  - `jq empty` over `spec/schemas/*.json` and `spec/defs/*.json`
  - stale-term sweep over active specs and `spec/schemas`; remaining active-spec hits are explicit
    removed/no-compatibility notes in `spec/03-air.md`
  - stale-term sweep over `spec/defs`

## Repository Sweep Targets

After conversion, these searches should return no active AIR v2 implementation or fixture hits:

```text
rg "defeffect"
rg "effect_bindings"
rg "manifest.effects"
rg "routing.module"
rg "routing.inboxes"
rg "effect_kind"
rg "adapter_id"
```

Historical notes may remain only when they are clearly marked as AIR v1 or removed behavior.

## Main Touch Points

- `spec/`
- `spec/schemas/`
- `spec/defs/`
- `roadmap/v0.22-py/`
- `crates/aos-air-types`
- `crates/aos-kernel`
- `crates/aos-node`
- `crates/aos-cli`
- `crates/aos-authoring`
- `crates/aos-effects`
- `crates/aos-effect-adapters`
- `crates/aos-wasm-sdk`
- `crates/aos-sys`
- `crates/aos-smoke/fixtures`
- `crates/aos-agent`
- `crates/aos-agent-eval/fixtures`

## Done When

- The repository's active specs, schemas, fixtures, and CLI output describe AIR v2 ops as the default
  model.
- No loadable fixture depends on `defeffect`, `effect_bindings`, module-routed workflows, or
  effect-kind identity.
- Built-in and smoke fixtures build/load under AIR v2.
- Targeted crate tests pass for AIR types, authoring, kernel, node worker/runtime, effects, adapters,
  and smoke fixtures.
- Full workspace tests either pass or have a short known-fail list limited to intentionally deferred
  Python runtime execution.
