# P5: Governance: Patch Schema + Control Surface

**Priority**: P5  
**Effort**: Medium  
**Risk if deferred**: Medium (governance ergonomics + correctness)

**Status snapshot**: Patch schema authored and embedded (`spec/schemas/patch.schema.json`); governance param/receipt schemas and effect kinds drafted in built-ins for v0.4. Control verbs/CLI validation path wired; patch-doc compiler implemented in kernel. Remaining: control-path patch-doc submission, policy/cap for governance, adapter (deferred to p1).

## What’s missing
- Patch format (`base_manifest_hash`, operations like `add_def`, `replace_def`, `remove_def`, `set_manifest_refs`, `set_defaults`, etc.) is only specified in prose (spec/03-air.md §15). There is no JSON Schema to validate proposals before they hit the kernel/shadow runner.
- Control/CLI path for governance verbs is underspecified. Propose/shadow/approve/apply should be first-class calls, not generic “enqueue event” plumbing.

## Why it matters
- Completes the “everything in the control plane has a schema” story; proposals become structurally validated before execution.
- Reduces kernel/shadow churn and improves diagnostics for malformed proposals.
- Keeps governance deterministic and auditable by routing through explicit kernel APIs (not generic domain events).

## Proposed work
 1) **Author patch schema** *(done)*: added `spec/schemas/patch.schema.json`.
 2) **Spec update**: update `spec/03-air.md §15` to reference the schema, document invariants (single `base_manifest_hash`, op shapes, no duplicate ops per target).
 3) **Kernel/tooling validation** *(partially done)*: patch-doc compiler added in kernel; schema validation still to be wired into submission paths.
 4) **Control channel/CLI** *(partially done)*: verbs added; CLI validates patch JSON but still needs patch-doc submission wiring and schema check on control path.
 5) **Fixtures/tests** *(partially done)*: patch_doc unit tests added; control governance integration test added. Still need negative fixtures for invalid patches via control once validation is wired.

## Design notes
- No semantic changes to patch ops; schema is structural and matches existing prose.
- Governance verbs remain explicit kernel calls; do not treat them as generic events in the control plane.
- If richer error info is needed, extend the schema with optional fields rather than inventing alternate payload shapes.

## Forward prep for self-upgrade (v0.4)
- Reserve governance effect schemas and give them concrete shapes now (to avoid hash churn later); these will move into `spec/defs/builtin-schemas.air.json` and the effect catalog when self-upgrade lands:
  - **Params**
    - `sys/GovProposeParams@1`: `{ patch_hash:hash, manifest_base?:hash, description?:text }`
    - `sys/GovShadowParams@1`: `{ proposal_id:nat }`
    - `sys/GovApproveParams@1`: `{ proposal_id:nat, decision:"approve"|"reject", approver:text }`
    - `sys/GovApplyParams@1`: `{ proposal_id:nat }`
  - **Receipts**
    - `sys/GovProposeReceipt@1`: `{ proposal_id:nat, patch_hash:hash, manifest_base?:hash }`
    - `sys/GovShadowReceipt@1`: `{ proposal_id:nat, manifest_hash:hash, effects_predicted:[EffectKind], pending_receipts?:[PendingPlanReceipt], plan_results?:[PlanResultPreview], ledger_deltas?:[LedgerDelta] }` (mirrors `ShadowReport` fields)
    - `sys/GovApproveReceipt@1`: `{ proposal_id:nat, decision:"approve"|"reject", patch_hash:hash, approver:text }`
    - `sys/GovApplyReceipt@1`: `{ proposal_id:nat, manifest_hash_new:hash, patch_hash:hash }`
- Define a new cap type `governance` and a built-in `defcap` (to ship in v0.4) with schema:
  ```json
  {
    "$kind":"defcap",
    "name":"sys/governance@1",
    "cap_type":"governance",
    "schema":{
      "record":{
        "modes":{ "set":{ "text":{} } },          // which verbs: propose/shadow/approve/apply
        "namespaces":{ "set":{ "text":{} } },     // allowed AIR namespaces to touch
        "max_patches":{ "nat":{} }                // optional ceiling for proposals
      }
    }
  }
  ```
- Keep control-channel verbs typed and reusable by both operators and in-world plans; avoid CLI-only payloads that would block effect parity later.
- Ensure patch schema validation is factored so it can be invoked from both control verbs and future governance effect handlers (no CLI-only validation path).
- Receipts emitted by governance effects must mirror the canonical governance journal entries (Proposed/ShadowReport/Approved/Applied) so replay remains deterministic; journal stays the source of truth.
- **TODO (authoring ergonomics)**: keep “hashless” authoring like `examples/06-safe-upgrade`:
  - Accept sugar patches with ZERO_HASH wasm placeholders and missing manifest ref hashes.
  - CLI/control path should load nodes, write them to the store, fill hashes, patch manifest refs, then canonicalize and hash the patch before submission.
  - Validate only the patch envelope/ops via `patch.schema.json`; structural/node validation and canonicalization happen in the submit path.
  - Add a CLI convenience (`aos world gov propose --patch-dir <air dir>`) that builds the patch from an asset bundle, computes hashes, validates, then submits, so authors don’t need to hand-edit hashes.
- **Deferred to p1 self-upgrade**: governance effect adapter (`governance.*` intents handled in-kernel with same propose/shadow/approve/apply pathway) so in-world plans can self-upgrade. Keep control verbs and validation ready; adapter work will land with v0.4 self-upgrade milestone.

## Future: init via patch path (sketch)
**Background**: World init currently loads AIR assets directly and installs the manifest in a privileged path. Governance proposals use the patch pipeline. Unifying would reduce drift and make the first manifest follow the same determinism/validation rules.

**Why it matters**
- Single codepath for manifest construction and validation.
- Genesis becomes reproducible: “apply this patch doc to empty manifest” is auditable.
- Fewer chances for init-only bugs or unchecked manifests.

**Pros**
- Consistency with governance/shadow/canonicalization.
- Clear provenance: genesis is just proposal #0 applying a patch.
- Easier testing: patch compiler gets exercised from day 0.

**Cons / wrinkles**
- Need a documented “empty manifest hash” genesis constant.
- Init often runs before daemon/control; need an auto-approve/apply path for the first proposal.
- Slightly more steps vs. current direct load; must avoid user friction.

**Possible approach**
- Define `EMPTY_MANIFEST_HASH` (hash of canonical empty manifest) and allow `base_manifest_hash` to be this value for genesis.
- Add an optional `--init-via-patch` mode: load AIR bundle → build patch doc → compile → auto-approve/apply (record genesis governance entries) → write manifest/ledger.
- Keep existing fast init for now; after baking, consider making patch-based init the default and reserve the direct path for recovery tooling.
