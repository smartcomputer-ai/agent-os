# P5: Op-Based Effect Execution

Status: planned.

## Goal

Remove `manifest.effect_bindings` and route effect execution from the effect op implementation metadata.

The node should dispatch an opened effect by looking at the pinned effect op and its implementation, not by looking up semantic kind in an adapter binding table.

## Work

- Replace `EffectRuntime.effect_routes: kind -> adapter_id` with op implementation resolution.
- Remove prefix-based `classify_effect_kind` dispatch.
- Select the runtime path from the effect op's referenced module `runtime.kind`, the op kind, and
  the runtime/builtin implementation registry.
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

## Runtime Dispatch Mapping

Effect dispatch class is not a public AIR field. Resolve it from op implementation metadata:

```text
builtin sys/workspace.* -> kernel internal effect path
builtin sys/timer.set   -> owner-local timer runtime
python effect op        -> async Python runner after durable flush
builtin adapter effects -> async effect runtime after durable flush
```

Do not infer dispatch class from semantic kind prefixes once this phase is complete. For builtins,
the internal registry keyed by `impl.module + impl.entrypoint` owns the mapping. For Python effects,
`runtime.kind = "python"` and `op_kind = "effect"` select the async Python runner.

## Done When

- External async effects start from effect op impl metadata.
- Timers use owner-local dispatch from builtin op implementation metadata.
- Workspace, introspect, governance, and portal effects use internal deterministic dispatch from builtin op implementation metadata.
- `manifest.effect_bindings` is not read anywhere.
