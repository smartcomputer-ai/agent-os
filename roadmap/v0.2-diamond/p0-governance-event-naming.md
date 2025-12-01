# P0: Standardize Governance Event Naming

**Priority**: P0 (must fix before v1 ships)
**Effort**: Small
**Risk if deferred**: Medium - permanent "which one was which?" confusion in code/docs

## Summary

The governance events that drive the constitutional loop (`propose → shadow → approve → apply`) have inconsistent names between spec documents. This must be standardized before v1 ships.

## Rationale

Naming inconsistency causes:
- Confusion when reading code vs docs
- Bugs when searching for event handling
- Drift between what docs say and what code does
- Friction for new contributors

## Current State

### In `spec/02-architecture.md` (Event Kinds section):

```
ProposalSubmitted {patches, proposer}
ShadowRunCompleted {proposal_id, predicted_effects, diffs}
ApprovalRecorded {proposal_id, approver, decision}
PlanApplied {manifest_root, plan_id}
```

### In `spec/03-air.md` (Journal Entries section):

```
Proposed {patch_hash, author, manifest_base}
ShadowReport {patch_hash, effects_predicted, diffs}
Approved {patch_hash, approver}
Applied {manifest_hash_new}
```

### Differences:

| Concept | Architecture doc | AIR doc |
|---------|-----------------|---------|
| Proposal | `ProposalSubmitted` | `Proposed` |
| Shadow | `ShadowRunCompleted` | `ShadowReport` |
| Approval | `ApprovalRecorded` | `Approved` |
| Apply | `PlanApplied` | `Applied` |

Also note field name differences (`proposal_id` vs `patch_hash`, etc.)

## Recommendation (updated)

Use the **shorter names** (`Proposed`, `ShadowReport`, `Approved`, `Applied`) everywhere **and carry both IDs**:

- `proposal_id`: primary correlation key (u64, monotonic per world) — used by the kernel/governance manager.
- `patch_hash`: content-addressed key — enables cross-world tooling and audit; not guaranteed unique (retries can reuse a patch).

### Canonical Journal Entry Shapes (dual-key)

```
Proposed {
  proposal_id: u64,
  patch_hash: Hash,
  author: text,
  manifest_base: Hash,
  description?: text
}

ShadowReport {
  proposal_id: u64,
  patch_hash: Hash,
  manifest_hash: Hash,          // candidate manifest root
  effects_predicted: [EffectKind],
  pending_receipts?: [PendingPlanReceipt],
  plan_results?: [PlanResultPreview],
  ledger_deltas?: [LedgerDelta]
}

Approved {
  proposal_id: u64,
  patch_hash: Hash,
  approver: text,
  decision: "approve" | "reject"
}

Applied {
  proposal_id: u64,
  patch_hash: Hash,
  manifest_hash_new: Hash  // actual manifest root after apply (not the patch hash)
}
```

Notes:
- Dual-key keeps current code paths intact while enabling hash-based audit/search.
- `ShadowReport` is flattened (no `summary_cbor`) and includes the candidate manifest root.
- `Approved` adds `decision` to support rejection.
- `Applied` must record the new manifest root (not the patch hash).

## Implementation Plan

### Step 1: Update `spec/02-architecture.md`

- Rename to `Proposed/ShadowReport/Approved/Applied`.
- Include both `proposal_id` and `patch_hash` fields; state that `proposal_id` is the correlation key, `patch_hash` is content key and may repeat.
- Clarify that `Applied.manifest_hash_new` is the manifest root after apply.

### Step 2: Align `spec/03-air.md`

- Mirror the dual-key shapes and naming.
- Document flattened `ShadowReport` fields (no `summary_cbor`), including manifest_hash and optional lists.
- Add `decision` to `Approved` (or explicitly defer and note approve-only).

### Step 3: Update Rust code

- Rename enums/records to the shorter names.
- Add `patch_hash` to Shadow/Approved/Applied records; ensure governance manager carries both keys.
- Fix `Applied` to record `manifest_hash_new` (actual manifest root) alongside ids.

### Step 4: Update examples/tests

- Example 06-safe-upgrade and governance integration tests: assert dual-key presence and correct manifest hash on apply.
- Add regression: two proposals with identical `patch_hash` must not collide (distinct `proposal_id`).

## Acceptance Criteria

- [x] `spec/02-architecture.md` uses `Proposed/ShadowReport/Approved/Applied` with dual-key fields and manifest root in `Applied`.
- [x] `spec/03-air.md` matches naming and fields; `ShadowReport` shape documented; `Approved` decision field resolved (implemented or explicitly deferred).
- [x] Rust enums/records in `aos-kernel` renamed and carry both `proposal_id` and `patch_hash`; `Applied` stores the new manifest root.
- [x] No references to old names remain in codebase (only this todo retains the before/after table for context).
- [x] Governance examples/tests cover dual-key semantics and manifest root correctness.
