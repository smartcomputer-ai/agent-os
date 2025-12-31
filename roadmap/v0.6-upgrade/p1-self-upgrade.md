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

## What still needs to be done
- **Plan-driven governance effects**: add the in-kernel effect adapter that handles `governance.*` intents, returns typed receipts, and replays from recorded receipts.
- **Governance cap type + default policy stub**: embed `sys/governance@1` in builtins and provide a default-deny policy template.
- **Patch build surface for plans**: add a compile/build path so plans can submit patch docs/CBOR (see options below).
- **In-world upgrade requests**: add a system intent schema + manifest trigger so reducers can request upgrades.
- **Tests/fixtures**: plan-driven loop, policy/cap denials, sequencing errors, hash mismatch, idempotency, and replay determinism.
- **CLI polish**: `gov list/show` are still stubs (optional for P1, but useful for operator parity).

## Proposed work (updated)
1) **Governance cap design + embed builtin**  
   - Add `sys/governance@1` defcap to `spec/defs/builtin-caps.air.json` and `aos-air-types` builtin list.  
   - Enforce constraints via a pure enforcer module (see design below); handler only normalizes/derives summary.

2) **Governance effect adapter + receipts**  
   - Route `governance.propose/shadow/approve/apply` intents through kernel governance APIs.  
   - Emit receipts that mirror governance journal records (Proposed/ShadowReport/Approved/Applied).  
   - Enforce `GovProposeParams.manifest_base == patch.base_manifest_hash` when provided.  
   - Use idempotency keys to fence duplicates; reject invalid sequencing.

3) **Patch build surface for plans**  
   - Lock-in: extend `governance.propose@1` params to accept a variant `patch` input:
     - `patch = { hash }` where `hash` is the canonical **ManifestPatch CBOR** hash (no JSON form here).
     - `patch = { patch_cbor }` for raw ManifestPatch CBOR bytes.
     - `patch = { patch_doc_json }` for PatchDocument JSON bytes.
     - `patch = { patch_blob_ref, format }` with `format = "manifest_patch_cbor" | "patch_doc_json"` for large payloads.
   - The handler compiles PatchDocument inputs to a canonical ManifestPatch, stores nodes, computes `patch_hash`, and returns it in the receipt.
   - No separate `patch.compile` step for P1; keep the single-step propose flow to mirror control.

4) **Plan surface + triggers**  
   - Introduce `sys/GovActionRequested@1` (or similar) so reducers can emit upgrade requests.  
   - Add manifest triggers to launch privileged upgrade plans.  
   - Document the pattern: reducer intent -> upgrade plan -> governance effects -> result event to reducer.

5) **Tests/fixtures**  
   - Integration test: plan-driven loop end-to-end with receipts and replay.  
   - Negative cases: policy deny, cap missing, sequencing errors, manifest_base mismatch, duplicate apply.

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
