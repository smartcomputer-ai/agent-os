# P3.3A: Plan Import Reuse via `aos.sync.json`

**Priority**: P3  
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
4. Add validation/CI gates for worlds that consume imported plans.

### Out of scope

1. In-plan subplan calls (`spawn_plan`, etc.) and trigger projection semantics (P3.3B).
2. Template macro language.
3. Runtime fetching of plans from workspaces.

## Decision Summary

1. Keep using `air.imports` from `aos.sync.json` as the only import mechanism.
2. Standardize SDK plan-pack exports under `air/exports/plan-packs/<pack>/`.
3. Treat imported plan cap slot names as stable API.
4. Keep world-local `cap_grants` and policy rules local to consuming world.
5. Keep hard-error policy for same-name, different-hash conflicts.
6. Start enforcing import lock pinning in CI (warning locally at first, hard-fail in CI).

## Plan-Pack Export Convention

In `crates/aos-agent-sdk`:

1. `air/exports/plan-packs/<pack>/defs.air.json`
2. `air/exports/plan-packs/<pack>/README.md`

`defs.air.json` should contain only defs (no manifest):

1. reusable `defplan` nodes,
2. required supporting `defschema` nodes for plan IO/results/events,
3. optional helper defs needed for validation completeness.

### Why this convention

1. Mirrors P3.1 export style, reducing tool complexity.
2. Gives explicit package boundaries for plan APIs.
3. Allows multiple packs with independent version cadence.

## Consumer Contract for Imported Plans

Every imported reusable plan must document in pack `README.md`:

1. **Input schema** and output/result schema.
2. **Emitted event schemas** (if any).
3. **Required cap slot names** and expected effect kinds.
4. **Assumptions** about routing/triggering pattern.

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
2. Import pack in one fixture world first.
3. Replace local duplicated plan definitions where IO contract already matches.
4. Keep temporary thin wrappers only where envelope mismatch remains.
5. Remove duplicated local plans once coverage is green.

## Validation Gates

For any world consuming imported plans:

1. `aos push --dry-run` must pass,
2. manifest/loader validation must pass,
3. replay parity check must pass,
4. at least one smoke/e2e path must execute imported plan logic.

## Risks and Considerations

1. **Interface drift**: plan packs without strict docs become implicit contracts.
2. **Cap slot instability**: renaming slots breaks consumers.
3. **Lock non-enforcement**: hidden import drift across developer machines.
4. **Over-shared plans**: if plan inputs are envelope-coupled, reuse remains low.

## Definition of Done

1. SDK ships at least one documented plan pack export.
2. At least one consumer world uses imported `defplan` from SDK export.
3. Conflict behavior and lock strategy are documented and exercised in CI.
4. Local duplicate plan logic is reduced in at least one existing fixture/app.
