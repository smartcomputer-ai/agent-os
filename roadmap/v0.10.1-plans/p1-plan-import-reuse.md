# P1: Plan Import Reuse via `aos.sync.json`

**Priority**: P1  
**Status**: Proposed (implementation-ready)  
**Depends on**: P3.1 defs sync (`air.imports`)  
**Does not require**: kernel language changes

## Context

We already solved schema/module/cap/policy duplication with defs sync (P3.1). The same import transport and merge model can distribute reusable SDK plans now.

This is the fastest way to reduce duplicated plan files while preserving deterministic manifest loading and strict conflict behavior.

## Problem

Without shared plan import conventions, every app world copies plan bodies and edits them locally. Even when logic is identical, local copies diverge over time and become difficult to audit.

Current pain patterns:

1. identical effect orchestration copied into fixture/app-local plans,
2. bug fixes needing N backports,
3. unclear ownership of "source of truth" for core agent orchestration.

## Why this approach fits AOS

AOS emphasizes deterministic replay, auditable control plane state, and explicit governance boundaries.

Import-based reuse fits these goals because:

1. imports are resolved at build/authoring time,
2. merged defs are content-addressed and hash-verified,
3. duplicate name conflicts are explicit hard errors,
4. no runtime dynamic loading is introduced.

## Scope

### In scope

1. Reuse `defplan` from upstream import roots.
2. Define SDK export layout for reusable plan packs.
3. Define interface contract for imported plans (IO schemas, cap slot naming, emitted events).
4. Support two consumption modes:
   - turnkey import (minimal world-local glue, no wrapper plan when contracts match),
   - composable-core import (app-local wrapper/adapters are expected and first-class).
5. Add validation/CI gates for worlds that consume imported plans.

### Out of scope

1. In-plan subplan calls (`spawn_plan`, etc.) and trigger projection semantics (P2).
2. Template macro language.
3. Runtime fetching of plans from workspaces.

## Decision Summary

1. Keep using `air.imports` from `aos.sync.json` as the only import mechanism.
2. Standardize SDK plan-pack exports under `air/exports/plan-packs/<pack>/`.
3. Treat imported plan cap slot names as stable API.
4. Keep world-local `cap_grants` and policy rules local to consuming world.
5. Keep hard-error policy for same-name, different-hash conflicts.
6. Start enforcing import lock pinning in CI (warning locally at first, hard-fail in CI).
7. Standardize two plan-pack profiles:
   - `turnkey`: import as near-standalone app flow when schema/envelope contracts already align,
   - `composable-core`: import core orchestration and compose via wrapper/adapters where needed.
8. Treat wrapper plans as a first-class extension mechanism, not a migration smell.

## Plan-Pack Export Convention

In `crates/aos-agent-sdk`:

1. `air/exports/plan-packs/<pack>/defs.air.json`
2. optional: `air/exports/plan-packs/<pack>/README.md` (human guidance only)

`defs.air.json` should contain only defs (no manifest):

1. reusable `defplan` nodes,
2. required supporting `defschema` nodes for plan IO/results/events,
3. optional helper defs needed for validation completeness.

Plan-pack contract is inferred from `defs.air.json` + naming conventions (no extra metadata file):

1. profile capability inferred from exported plan roles:
   - has `entry_*` plans => turnkey-capable,
   - has `core_*` plans => composable-core-capable,
   - may support both.
2. plan roles inferred by naming conventions.
3. required cap slots/effect kinds inferred from `required_caps`/`allowed_effects` (or derived from `emit_effect`).
4. turnkey trigger contract inferred as: trigger event schema equals the entry plan input schema.

### Why this convention

1. Mirrors P3.1 export style, reducing tool complexity.
2. Gives explicit package boundaries for plan APIs.
3. Allows multiple packs with independent version cadence.
4. Avoids introducing another repository-level config file while still enabling lint/scaffold tooling.
5. Keeps contract machine-checkable even when README is absent.

## Where Classification Lives (and Why)

Plan classification should live in conventions over `defs.air.json`, not inside `defplan`.

Rationale:

1. `defplan` is AIR runtime language and schema-hashed content; adding profile/classification fields there is a language change and hash churn beyond P1 scope.
2. `turnkey` vs `composable-core` is a distribution/integration concern, not a kernel execution concern.
3. naming conventions can classify plan roles (`entrypoint`, `core`, `adapter`, `internal`) without introducing new config files.
4. tooling can evolve lint/scaffold behavior independently of AIR grammar changes.

Conventions used for machine-checkable role inference:

1. entrypoint plans: `.../entry_<flow>@<v>`
2. core plans: `.../core_<flow>@<v>`
3. adapter plans: `.../adapter_<flow>@<v>`
4. internal plans: `.../_internal_<flow>@<v>`

Future option:

1. If runtime-visible plan annotations become necessary later, add them as explicit AIR versioned semantics in a follow-on roadmap item.

## Plan-Pack Profiles (Both Are Required)

### Why both profiles are needed

One profile cannot optimize for both fast adoption and deep app customization:

1. teams often want to import an SDK flow as-is and run it immediately,
2. other teams need app-specific event envelopes, routing, and post-processing.

Supporting both keeps reuse high without forcing every consumer into the same coupling level.

### `turnkey` profile (import as near-standalone flow)

Use when:

1. world event contracts already match the pack’s trigger/input contracts,
2. app can accept pack event outputs directly,
3. primary goal is fastest adoption with minimal local plan authoring.

Expected characteristics:

1. pack exports reducer + plan defs + schemas needed for that flow,
2. consumer manifest mainly wires refs/triggers and world-local authority (`cap_grants`, policy),
3. no wrapper plan is required when contracts align.

### `composable-core` profile (import core + compose locally)

Use when:

1. app has custom envelope/routing boundaries,
2. app needs custom pre/post steps around shared orchestration,
3. app wants to preserve local domain event contracts while reusing core effect DAGs.

Expected characteristics:

1. pack exports stable core plans with explicit typed IO contracts,
2. app-local wrappers/adapters project local contracts into core plan inputs and wrap outputs back,
3. wrapper plans are expected and documented.

## Consumer Contract for Imported Plans

Consumer contract must be derivable from defs alone:

1. **Input/output contract**: from `defplan.input` and optional `defplan.output`.
2. **Emitted events**: from each `raise_event.event` used by exported plans.
3. **Capability contract**: from `required_caps`/`allowed_effects` (or `emit_effect.{cap,kind}` derivation).
4. **Profile capability**:
   - turnkey-capable if pack exports one or more `entry_*` plans,
   - composable-core-capable if pack exports one or more `core_*` plans.
5. **Turnkey trigger rule**: consumer trigger event schema should match entry plan input schema.

Optional README may explain intent and migration guidance, but tooling must not require it.

### Important clarification about caps (agreed)

Importing cap defs is not enough by itself.

Consumer world still must provide:

1. concrete `defaults.cap_grants` bindings for the slot names used by imported plan,
2. policy rules that allow those effects for the plan origin (`origin_kind=plan`, `origin_name=<plan>`).

This is intentional: authority remains world-local and auditable.

## `aos.sync.json` Example

```json
{
  "air": {
    "dir": "air",
    "imports": [
      {
        "cargo": {
          "package": "aos-agent-sdk",
          "air_dir": "air/exports/plan-packs/session-core",
          "manifest_path": "../../Cargo.toml"
        }
      }
    ]
  }
}
```

## Merge and Conflict Behavior

Use existing P3.1 semantics unchanged:

1. same def name + same content hash -> dedupe,
2. same def name + different content hash -> hard error,
3. import roots cannot define manifests.

### Why strict conflict behavior remains best

1. deterministic provenance,
2. no hidden shadowing/override behavior,
3. easier debugging and governance review.

## Locking and Reproducibility

`air.imports[].lock` is currently parsed but not enforced. For plan reuse we should prioritize lock enforcement to avoid accidental drift.

Recommended rollout:

1. Phase 1: local warning when lock absent/mismatch.
2. Phase 2: CI hard-fail for lock mismatch.
3. Phase 3: optional local hard-fail mode.

Lock payload should include:

1. resolved package identity,
2. selected version/source,
3. content hash of imported AIR defs.

## Migration Guidance

1. Start with one high-value shared plan pack (for example session/workspace orchestration primitives).
2. Classify it as `turnkey` or `composable-core`.
3. Import pack in one fixture world first.
4. If contracts align, migrate to turnkey consumption (no wrapper plan).
5. If contracts differ, keep or add a thin wrapper plan and delegate to imported core logic.
6. Remove duplicated full local plans once coverage is green.

## Implementation Plan (Best Path in P1 Scope)

### Phase A1: Contract and profile standardization

1. Add role/profile naming conventions to every exported pack.
2. Ensure exported defs are sufficient for machine-derived contract checks (no README dependency).
3. Keep this as tooling/documentation only; no kernel changes.

### Phase A2: Import/lint enforcement

1. Add a CLI lint/check that validates:
   - plan names match role conventions (`entry_`, `core_`, `adapter_`, `_internal_`),
   - profile capability inference is unambiguous (turnkey/composable/both),
   - required cap slots are bound in consumer manifest defaults,
   - turnkey consumers do not carry redundant copied plan bodies.
2. Enforce lock pinning policy rollout described in this doc.

### Phase A3: Consumer scaffolding

1. Add CLI scaffold for pack consumption:
   - turnkey: generate minimal manifest refs/triggers/policy skeleton,
   - composable-core: generate thin wrapper template + trigger skeleton.
2. Keep generated policy/grants world-local for authority control.

## Validation Gates

For any world consuming imported plans:

1. `aos push --dry-run` must pass,
2. manifest/loader validation must pass,
3. replay parity check must pass,
4. at least one smoke/e2e path must execute imported plan logic,
5. at least one turnkey consumer path runs without wrapper plans,
6. at least one composable-core consumer path runs with thin wrapper composition.

## Risks and Considerations

1. **Interface drift**: weak naming conventions make contracts implicit.
2. **Cap slot instability**: renaming slots breaks consumers.
3. **Lock non-enforcement**: hidden import drift across developer machines.
4. **Over-shared plans**: if plan inputs are envelope-coupled, reuse remains low.
5. **Profile ambiguity**: unclear turnkey vs composable intent causes incorrect integration assumptions.

## Definition of Done

1. SDK ships at least one convention-compliant plan pack export.
2. At least one consumer world uses imported `defplan` from SDK export.
3. Conflict behavior and lock strategy are documented and exercised in CI.
4. Local duplicate plan logic is reduced in at least one existing fixture/app.
5. SDK ships at least:
   - one `turnkey` pack consumed without wrapper plans,
   - one `composable-core` pack consumed with thin wrapper composition.
6. Pack conventions (role naming + defs-derived contract checks) are validated by tooling in CI.
