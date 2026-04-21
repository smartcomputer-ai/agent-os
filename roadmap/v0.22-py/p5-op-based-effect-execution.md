# P5: Op-Based Effect Execution

Status: planned.

## Goal

Remove `manifest.effect_bindings` and route effect execution from the effect op implementation metadata.

The node should dispatch an opened effect by looking at the pinned effect op and its implementation, not by looking up semantic kind in an adapter binding table.

## Work

- Replace `EffectRuntime.effect_routes: kind -> adapter_id` with op implementation resolution.
- Use `defop.effect.execution_class` instead of prefix-based `classify_effect_kind`.
- Use `defop.impl.calling_convention` to select runtime path:
  - `builtin_effect`
  - `async_effect`
  - later `python_async_effect`
  - later `wasm_effect`
- For builtin/existing Rust adapters, map op implementation entrypoint to the existing adapter registry internally.
- Remove `strict_effect_bindings`.
- Remove route diagnostics that refer to `manifest.effect_bindings`.
- Update adapter start context to carry effect op identity and semantic kind.
- Update receipt `adapter_id` semantics or replace it with executor identity in new records.
- Keep the durable append fence unchanged: opened async effects are published only after their containing frame is flushed.

## Main Touch Points

- `crates/aos-node/src/execution/effect_runtime.rs`
- `crates/aos-node/src/worker/runtime.rs`
- `crates/aos-node/src/worker/util.rs`
- `crates/aos-effect-adapters/src/adapters/registry.rs`
- `crates/aos-effect-adapters/src/adapters/traits.rs`
- `crates/aos-effect-adapters/src/lib.rs`
- `crates/aos-kernel/src/internal_effects`
- `crates/aos-kernel/src/trace.rs`
- node and adapter tests

## Execution Class Mapping

Effect execution class should be declared in the effect op:

```text
internal_deterministic -> kernel internal effect path
owner_local_async      -> owner-local runtime such as timer
external_async         -> async effect runtime
```

Do not infer execution class from semantic kind prefixes once this phase is complete.

## Done When

- External async effects start from effect op impl metadata.
- Timers use owner-local async classification from effect op metadata.
- Workspace, introspect, governance, and portal effects use internal deterministic classification from effect op metadata.
- `manifest.effect_bindings` is not read anywhere.

