# P1: Self-Upgrade via Governed Plans

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks agent-led upgrades; governance remains operator-only)

## Status snapshot (post v0.5-caps-policy)
- Kernel governance loop exists (propose/shadow/approve/apply) with journal records and shadow summaries.
- Patch schema + patch-doc compiler are in place; control channel accepts PatchDocument or ManifestPatch.
- Governance effect schemas and defeffects are defined (plan-only origin) in builtins.
- Control channel governance verbs are live; CLI can propose/shadow/approve/apply.
- Safe-upgrade example and control/kernel governance tests exist.
- Kernel internal adapter handles `governance.*` effects and emits typed receipts.
- `governance.propose@1` accepts patch variants; kernel preprocessor compiles patch docs/CBOR to canonical patches + summaries.
- `sys/governance@1` cap + `sys/CapEnforceGovernance@1` enforcer are in builtins (with GovPatchInput/GovPatchSummary schemas).
- Plan-driven governance loop is covered by new integration tests.
- `sys/GovActionRequested@1` schema added for reducer-driven upgrade requests.

## What still needs to be done
- **Default-deny governance policy stub**: provide a starter policy template for `sys/governance@1` (optional but helpful).
- **In-world upgrade requests**: wire up manifest triggers in the example manifests (schema is now in builtins).
- **Tests/fixtures**: policy/cap denials, sequencing errors, idempotency, replay determinism assertions for governance receipts.
- **CLI polish**: `gov list/show` are still stubs (optional for P1, but useful for operator parity).

## Proposed work (updated)
1) **Governance cap design + embed builtin** (done)  
   - `sys/governance@1` in builtins + pure enforcer module.  
   - Handler normalizes/derives summary before enforcement.

2) **Governance effect adapter + receipts** (done)  
   - `governance.propose/shadow/approve/apply` routed through kernel governance APIs.  
   - Typed receipts emitted; manifest_base check enforced.  
   - Sequencing errors surfaced via error receipts.

3) **Patch build surface for plans** (done)  
   - `governance.propose@1` accepts `patch` variant input (`hash`, `patch_cbor`, `patch_doc_json`, `patch_blob_ref`).  
   - PatchDocument inputs compile to canonical ManifestPatch + summary.  
   - No separate `patch.compile` step in P1.

4) **Plan surface + triggers** (partial)  
   - `sys/GovActionRequested@1` is now available for reducer-driven requests.  
   - Add manifest triggers to launch privileged upgrade plans.  
   - Pattern: reducer intent -> upgrade plan -> governance effects -> result event to reducer.

Example trigger pattern (manifest excerpt):
```json
{
  "triggers": [
    {
      "event": "sys/GovActionRequested@1",
      "plan": "com.acme/UpgradePlan@1"
    }
  ]
}
```

5) **Tests/fixtures** (partial)  
   - Plan-driven loop is covered.  
   - Remaining negative cases: policy/cap denials, sequencing/idempotency edges, replay determinism checks for governance receipts.

## Governance cap design (proposal)
Design the cap in terms of patch operations and manifest surfaces, since patches are the upgrade unit. Keep cap enforcement in pure modules (per v0.5 caps/policy) and give the enforcer a canonical, minimal patch summary rather than the full patch payload.

Proposed `sys/governance@1` schema (record, all fields optional):
- `ops?: set<text>`: allowed patch ops (`add_def`, `replace_def`, `remove_def`, `set_manifest_refs`, `set_defaults`, `set_routing_events`, `set_routing_inboxes`, `set_triggers`, `set_module_bindings`, `set_secrets`).
- `def_kinds?: set<text>`: allowed def kinds (`defschema`, `defmodule`, `defplan`, `defcap`, `defeffect`, `defpolicy`, `defsecret`).
- `name_prefixes?: set<text>`: allowed prefixes for def names and manifest refs (empty or missing = all).
- `manifest_sections?: set<text>`: allowed sections for set ops (`defaults`, `routing_events`, `routing_inboxes`, `triggers`, `module_bindings`, `secrets`, `manifest_refs`).

Enforcement flows:
- Governance handler compiles the patch and derives a **canonical patch summary** (see below).
- The effect params include that summary before intent hashing and the cap enforcer runs.
- A new pure enforcer module (e.g., `sys/CapEnforceGovernance@1`) checks cap constraints against the summary.
- Policy remains the coarse gate (default deny; allow only from specific plans/cap names).

Canonical patch summary fields (minimal; expand only if needed):
- `base_manifest_hash`
- `patch_hash`
- `ops` set
- `def_changes` list: `{ kind, name, action }`
- `manifest_sections` set

Rationale: the enforcer needs patch-aware context, but passing full patch payloads into params would bloat intent size, duplicate parsing, and decouple enforcement from the compiled (canonical) patch. A summary keeps the pure-enforcer model intact while staying deterministic and compact. Use receipt status for errors (no extra error fields).

Approver identity is optional; do not require it in policy. Use effect_kind + origin_name + cap_name rules for gating.

## Open questions
- Should `GovShadowReceipt@1` include `patch_hash` to match the journal record?  
- Do we need a minor policy extension to match on cap params or effect params, or is plan-name + cap-name matching sufficient?

## Out of scope
- Cross-world orchestration and multi-world policy delegation (leave to later roadmap).
