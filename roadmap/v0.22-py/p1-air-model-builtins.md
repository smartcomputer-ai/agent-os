# P1: AIR Model And Built-Ins

Status: planned.

## Goal

Change the public AIR model and built-in definitions so ops are first-class.

This is the structural phase. It should establish the new data model before the kernel runtime is migrated.

## Work

- Add `DefOp` to `aos-air-types`.
- Add `AirNode::Defop`.
- Replace canonical `DefEffect` with `DefOp { op_kind: "effect", ... }`.
- Redefine `DefModule` as runtime/artifact metadata.
- Move workflow ABI fields from `DefModule` to `DefOp.workflow`.
- Move effect contract fields from `DefEffect` to `DefOp.effect`.
- Keep `DefOp.impl.entrypoint` as an op-local selector so one module can implement many ops.
- Do not carry forward public pure ops in v0.22; reject `op_kind = "pure"` if it appears.
- Add JSON schema for `defop`.
- Update `defmodule.schema.json`.
- Update `manifest.schema.json`:
  - add required `ops`
  - remove `effects`
  - remove `effect_bindings`
  - keep `secrets` as `NamedRef[]` only; do not allow inline secret declarations
  - route subscriptions by `op`, not `module`
  - remove `routing.inboxes`
- Update `patch.schema.json` and common schema def-kind enums to understand `defop`.
  - use patch document version `"2"`
  - rename `set_routing_events` to `set_routing_subscriptions`
  - remove `set_routing_inboxes`
  - remove `set_secrets`; secret refs are handled through `set_manifest_refs`
- Convert `spec/defs/builtin-effects.air.json` into built-in effect ops.
- Convert built-in workflow module definitions into module-plus-workflow-op definitions.
- Update schema embedding in `crates/aos-air-types/src/schemas.rs`.
- Update `spec/03-air.md`, `spec/04-workflows.md`, and `spec/05-effects.md`.

## Suggested Shape

```json
{
  "$kind": "defmodule",
  "name": "sys/builtin_effects@1",
  "runtime": {
    "kind": "builtin"
  }
}
```

```json
{
  "$kind": "defop",
  "name": "sys/http.request@1",
  "op_kind": "effect",
  "effect": {
    "kind": "http.request",
    "params": "sys/HttpRequestParams@1",
    "receipt": "sys/HttpRequestReceipt@1"
  },
  "impl": {
    "module": "sys/builtin_effects@1",
    "entrypoint": "http.request"
  }
}
```

For WASM modules, `impl.entrypoint` names the export to invoke. It is not restricted to `"step"` in
the target model, and it should allow multiple workflow ops to share one content-addressed module.

Effect ops do not carry public `origin_scope` or `execution_class` fields. Workflows are the only
public effect emitters, gated by `workflow.effects_emitted[]`. Effect execution dispatch is resolved
from `impl.module`, `impl.entrypoint`, and the active runtime/builtin registry. Builtins use their
versioned module name as the registry identity; no separate `builtin_id` field is needed.

## Main Touch Points

- `crates/aos-air-types/src/model.rs`
- `crates/aos-air-types/src/validate.rs`
- `crates/aos-air-types/src/catalog.rs`
- `crates/aos-air-types/src/builtins.rs`
- `spec/schemas/*.schema.json`
- `spec/defs/*.air.json`
- `spec/03-air.md`
- `spec/04-workflows.md`
- `spec/05-effects.md`

## Done When

- New-style AIR with `defop` parses and serializes.
- Built-in schemas/modules/ops hash and load.
- Manifest validation no longer relies on `DefEffect`.
- `cargo test -p aos-air-types` passes.
