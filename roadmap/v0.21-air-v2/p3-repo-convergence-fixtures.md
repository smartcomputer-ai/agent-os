# P3: Repository Convergence And AIR v2 Fixtures

Status: planned.

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

- Convert checked-in AIR manifests and fixtures to AIR v2:
  - `manifest.ops`
  - `manifest.secrets`
  - `routing.subscriptions[].op`
  - workflow `effects_emitted[]`
  - module runtime/artifact declarations
  - effect ops with `params`, `receipt`, and `impl`
- Remove checked-in `defeffect` fixtures and examples.
- Remove `manifest.effects`, `manifest.effect_bindings`, `routing.module`, and `routing.inboxes`
  from active fixtures.
- Convert built-in SDK support schemas:
  - effect intent request payloads name an effect op
  - receipt envelopes use origin workflow op and effect op identity
  - stream frames use origin workflow op and effect op identity
  - executor identity replaces adapter-only identity where durable audit records need it
- Update workflow SDK helpers and examples to emit effect op names.
- Update smoke fixtures:
  - workspace demos
  - timer demos
  - blob demos
  - HTTP demos
  - Fabric demos
  - agent demos
- Update CLI and query rendering:
  - list/show `defop`
  - show workflow op and effect op counts
  - remove effect binding output
  - route summaries name target ops
  - governance summaries report `defop` changes
- Update specs to match the canonical target:
  - `spec/03-air.md`
  - `spec/04-workflows.md`
  - `spec/05-effects.md`
  - schema and built-in reference shelves
- Remove roadmap files made stale by the three-phase cut.
- Add focused regression tests for:
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
- Keep a short explicit known-fail list only for deferred Python runtime execution.

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
