# Remove Caps And Policies From v0.22

Status: done for cap/policy removal. Python effect runner work remains as a separate follow-on.

## Context

AOS has no users depending on the current capability and policy model, and the model is not yet carrying real product weight. It adds ceremony to AIR authoring, manifest loading, examples, and Python runtime design before we have the use cases that would make that complexity pay rent.

The goal is not to pretend authority will never matter. The goal is to remove the current public API and schema burden now, while keeping enough internal structure that a future authority model can return without a rewrite.

## Goal

Make v0.22 easier to author and easier to explain:

```text
schemas + modules/ops + effects + routing + secrets + receipts
```

No public caps.
No public policies.
No module/op cap bindings.
No policy language in manifests.

For now, a workflow may emit an effect if the effect is declared and allowed by the workflow/op contract. The runtime then executes it through the configured effect implementation. This is a behavioral contract, not a security boundary.

## Remove From Public Surface

AIR root forms:

- `defcap`
- `defpolicy`

Manifest fields:

- `manifest.caps`
- `manifest.policies`
- `manifest.defaults.policy`
- `manifest.defaults.cap_grants`
- `manifest.module_bindings`
- future `manifest.op_bindings` authority slots, if present in drafts

Module/op interface fields:

- `defmodule.abi.workflow.cap_slots`
- `defop.workflow.cap_slots`
- `defop.effect.cap_type`
- `defeffect.cap_type`

Runtime and authoring APIs:

- `ctx.emit(..., cap=...)`
- policy/cap flags in CLI authoring flows
- examples that require cap grants before an effect can run

## Keep In Public Surface

These still matter without caps or policies:

- `defschema`
- `defmodule` / `defop`
- `defeffect` until `defop` fully replaces it
- `manifest.schemas`
- `manifest.modules`
- `manifest.ops` when introduced
- `manifest.effects` until `defop` fully replaces it
- `manifest.effect_bindings` until Python effect routing replaces `adapter_id`
- `manifest.secrets`
- `manifest.routing`
- workflow `effects_emitted` / future effect-op allowlist
- effect params schema
- effect receipt schema
- effect `origin_scope`
- effect `execution_class`

## Keep Internally For Now

Keep these as permissive internals so existing code does not need to be gutted in the same patch as the public simplification:

- `LoadedManifest` indexes may keep empty caps/policies maps internally.
- Kernel authorization function stays, but returns allow by default.
- Legacy kernel cap enforcer machinery may remain unreachable/dead-ended temporarily for compatibility tests, but public built-in defs and `aos-sys` handlers are removed.
- Policy evaluator can remain behind an empty/default policy shim.
- Receipt, open-work, and idempotency logic remain unchanged.
- Effect param and receipt schema validation remains strict.

## Phase 1: Schema And AIR Surface

Status: done. The public `defcap`/`defpolicy` schema files and built-in cap defs are removed.

Work:

- Remove `defcap.schema.json` and `defpolicy.schema.json` from the public schema index.
- Remove caps, policies, default policy, default cap grants, and `module_bindings` from `manifest.schema.json`.
- Remove `cap_slots` from `defmodule.schema.json`.
- Remove `cap_type` from `defeffect.schema.json`, or mark it legacy-only if a compatibility window is needed.
- Update `spec/03-air.md` to describe the v0.22 simplified authority model.
- Update built-in defs so effects no longer require cap definitions.

Done when:

- New manifests validate without caps or policies.
- Examples no longer need cap grants or module bindings.

## Phase 2: Rust Model And Loader

Status: done for the public surface. Legacy structs remain internally, but authored/imported AIR rejects `defcap` and `defpolicy`, and the active schema set excludes them.

Work:

- Make `Manifest` omit caps, policies, `defaults.policy`, `defaults.cap_grants`, and `module_bindings`.
- Remove `DefCap` and `DefPolicy` from the public `AirNode` path, or keep legacy deserialization only.
- Simplify `validate_manifest` so it checks schemas, modules, effects, and routing, but not cap grants or policies.
- Decide whether old manifests with caps/policies are rejected or accepted with warnings and ignored.

Done when:

- A minimal manifest only needs schemas, modules, effects, routing, and secrets.
- Manifest validation errors no longer mention missing caps, cap grants, or policy defaults.

## Phase 3: Kernel Runtime Shim

Status: done for workflow-origin effect enqueue. New workflow effects use an internal sentinel grant while legacy intent structs still carry `cap_name`.

Work:

- Replace cap+policy admission with a single permissive `authorize_effect` hook.
- Keep workflow `effects_emitted` enforcement.
- Keep `origin_scope` enforcement.
- Keep effect params canonicalization before intent hashing.
- Remove cap name from new effect intent construction, or set an internal sentinel while legacy code remains.
- Keep receipts bound to `intent_hash` and recorded origin identity.

Done when:

- Workflow-origin effects can be opened without a cap grant.
- Undeclared/disallowed effects are still rejected.
- Replay snapshots remain byte-identical.

## Phase 4: Authoring, CLI, Examples

Status: done for checked-in authored AIR fixtures and public authoring output. Cap/policy fixture files were removed, and patch docs no longer expose defaults or module bindings.

Work:

- Remove cap/policy prompts and generated manifest blocks from `aos` authoring flows.
- Simplify fixture manifests.
- Update smoke demos and docs.
- Update Python roadmap examples so `ctx.emit` uses effect/op identity only.

Done when:

- A new user can define a workflow and effect without learning caps/policies.
- All checked-in fixtures use the simplified manifest shape.

## Phase 5: Python Effects On Simplified Model

Status: follow-on.

Work:

- Python `@effect` declares name, kind, params, receipt, `origin_scope`, `execution_class`, and implementation.
- Python effect runner receives canonical params, intent identity, op identity, secret context, and tracing context.
- Secrets are granted by coarse runner/world config, not by AIR caps.
- Receipt payload validation remains schema-authoritative.

Done when:

- WASM workflow -> Python effect works without caps or policies.
- Python effect errors produce normal terminal receipts.

## Future Authority Reintroduction

When real use cases demand authority again, add it back as a smaller, sharper model. Do not resurrect the old cap/policy split by default.

Likely future shape:

```json
{
  "defop.effect.authority": {
    "requires": [],
    "secrets": [],
    "network": [],
    "budget": {}
  }
}
```

Or a world/runner-level authority profile:

```json
{
  "manifest.authority_profiles": {
    "dev": "allow configured runner access",
    "hosted": "restricted by tenant identity and reviewed grants"
  }
}
```

The future model should be driven by concrete hosted/runtime needs: tenancy, secret access, network egress, budget controls, audit, and marketplace trust.

## Non-Goals

- Do not solve multi-tenant hosted security in v0.22.
- Do not design a new policy language now.
- Do not block Python effects on a future authority model.
- Do not remove effect declaration/allowlist validation.
- Do not weaken receipt, open-work, or replay invariants.

## Open Decisions

1. Reject or ignore old cap/policy fields?

   Decision: for v0.22, reject them in new schemas. No backward compatibility.

2. What replaces cap on effect intents?

   Decision: new intent identity should not include cap. During transition, use an internal sentinel value only where older structs still require the field. But goal is to remove it.

3. How do secrets work without caps?

   Recommendation: use explicit world/runner secret configuration. Python effects receive only configured handles. Workflows still cannot read ambient secrets.

4. Should `origin_scope` remain?

   Recommendation: yes. It is not a policy system; it is a simple structural rule preventing workflow/system/governance origin confusion.

## Implementation Order

1. Schema/spec simplification.
2. Rust model and validation simplification.
3. Kernel permissive authorization shim.
4. Fixture/example cleanup.
5. Python effect runner on the simplified model.
6. `defop` migration after the simplified surface is stable.
