# P1: Core Runtime & Batch Mode (path2 opinionated cut)

**Goal:** Land an async `aos-host` crate that wraps the deterministic kernel, exposes a small runtime API, and supports single-shot batch execution. Keep adapters in-process for now.

## Shape

```
crates/aos-host/
  src/
    lib.rs
    runtime.rs      // WorldRuntime: open, enqueue, drain, pending_effects, apply_receipt
    config.rs
    error.rs
    adapters/
      mod.rs
      traits.rs     // AsyncEffectAdapter (kind(), execute(intent) -> receipt)
      registry.rs
      timer.rs      // stub, immediate receipt
      http.rs       // stub receipt
      llm.rs        // stub receipt
    modes/
      batch.rs      // BatchRunner
```

## Runtime API (minimal)

- `WorldRuntime::open(store: Arc<S>, manifest_path, config) -> Self`
- `enqueue(EventOrReceipt)`
- `drain(fuel: Option<u64>) -> DrainOutcome { ticks, idle }`
- `pending_effects() -> Vec<EffectIntent>`
- `apply_receipt(EffectReceipt)`
- `snapshot()`
- `state_reader() -> &dyn StateReader`

Batch path (`BatchRunner::step`):
1) enqueue supplied events/receipts
2) drain
3) dispatch pending effects via adapters → collect receipts
4) enqueue receipts, drain again
5) snapshot

## Tasks

1) Scaffold `crates/aos-host`; add to workspace.
2) Define `HostError`, `RuntimeConfig`.
3) Define `AsyncEffectAdapter` and `AdapterRegistry`.
4) Implement stub adapters (timer/http/llm) returning OK receipts.
5) Implement `WorldRuntime` over `aos-kernel` + `aos-store`.
6) Implement `BatchRunner` and wire a CLI entry (`aos world step`).
7) Smoke-test with `examples/00-counter` using `--event`.

## Success Criteria

- `aos world step <path> --event demo/Increment@1 '{}'` drains and snapshots without panics.
- Pending effects are surfaced and fed stub receipts to reach quiescence.
