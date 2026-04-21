# P6: Repo Sweep And Fixture Conversion

Status: planned.

## Goal

Remove old forms and convert the repository to the op model after the runtime path works.

This should be a broad cleanup phase. Doing it before P1-P5 will create noisy intermediate failures; doing it after keeps the implementation phases easier to verify.

## Work

- Delete `spec/schemas/defeffect.schema.json`.
- Remove `DEFEFFECT` from embedded schema docs.
- Remove `DefEffect`, `EffectBinding`, and `OriginScope::Plan/Both` legacy surface.
- Remove `manifest.effects` and `manifest.effect_bindings`.
- Remove all `defeffect` fixtures.
- Convert checked-in manifests to `ops`.
- Convert checked-in routing to `op`.
- Convert workflow SDK helpers and examples to emit effect op refs.
- Update smoke fixtures:
  - blob demos
  - timer demos
  - fabric demos
  - agent demos
- Update docs and roadmap notes to stop describing `defeffect`.
- Update CLI rendering and inspection tools that show `effect_count` or `effect_bindings`.
- Run formatting and targeted tests.

## Main Touch Points

- `spec/`
- `roadmap/v0.22-py/`
- `crates/aos-air-types`
- `crates/aos-kernel`
- `crates/aos-node`
- `crates/aos-cli`
- `crates/aos-authoring`
- `crates/aos-smoke/fixtures`
- `crates/aos-agent-eval/fixtures`
- `crates/aos-agent`
- `crates/aos-harness-py`

## Done When

- `rg "defeffect|effect_bindings|manifest.effects|routing.module"` returns only intentional historical notes or no matches.
- Checked-in fixtures build/load under the op model.
- The default examples no longer mention `adapter_id` in manifest AIR.
- Full workspace tests are either passing or have a short known-fail list unrelated to defop cleanup.

