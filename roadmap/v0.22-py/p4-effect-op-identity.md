# P4: Effect Op Identity

Status: planned.

## Goal

Make effect intent, open-work, journal, receipt, and continuation routing identity op-based instead of semantic-kind-based.

This is the most important semantic phase. It should happen before Python effects.

## Work

- Change workflow output from emitting `kind` to emitting `effect_op`.
- Change `workflow.effects_emitted[]` allowlists to effect op refs.
- Resolve the effect op before parameter normalization.
- Use the effect op params schema for canonicalization.
- Use the effect op receipt schema for receipt payload validation.
- Update `EffectIntent` to carry:
  - `effect_op`
  - `effect_op_hash`
  - semantic `kind`
  - canonical `params_cbor`
  - idempotency key
  - intent hash
- Change intent hash preimage to include:
  - origin workflow op identity/hash
  - origin instance key
  - effect op identity/hash
  - params
  - emission position
  - workflow-requested idempotency key
- Update `EffectIntentRecord` and replay restore.
- Update `WorkflowEffectContext` to pin effect op identity and receipt schema identity.
- Update stream frame and receipt envelopes to include effect op identity while retaining semantic kind for diagnostics.
- Update trace, snapshot, and quiescence summaries.

## Main Touch Points

- `crates/aos-wasm-abi/src/lib.rs`
- `crates/aos-effects/src/intent.rs`
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-kernel/src/journal.rs`
- `crates/aos-kernel/src/receipts.rs`
- `crates/aos-kernel/src/snapshot.rs`
- `crates/aos-kernel/src/world/event_flow.rs`
- `crates/aos-kernel/src/world/runtime.rs`
- `crates/aos-kernel/src/world/snapshot_replay.rs`
- `crates/aos-node/src/worker`
- `crates/aos-harness-py`
- `crates/aos-agent`

## Migration Stance

Do not try to replay old journals after this cut unless a test explicitly needs a converter. This is an experimental branch and the refactor should stay simple.

## Done When

- A workflow emitting `sys/timer.set@1` opens an effect whose canonical identity is the effect op.
- Undeclared effect op emissions fail.
- Receipt payload validation is driven by the recorded effect op, not by searching effect definitions by semantic kind.
- Replay from genesis produces byte-identical snapshots for new journals.

