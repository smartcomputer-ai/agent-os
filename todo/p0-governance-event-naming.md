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

## Recommendation

Use the **shorter names** (`Proposed`, `ShadowReport`, `Approved`, `Applied`) everywhere:

1. They're more concise
2. They match the conceptual verbs (propose/shadow/approve/apply)
3. Journal enums should be short since they appear frequently

### Canonical Journal Entry Shapes

```
Proposed {
  patch_hash: Hash,
  author: text,
  manifest_base: Hash
}

ShadowReport {
  patch_hash: Hash,
  effects_predicted: [EffectKind],
  diffs: Hash  // reference to diff summary
}

Approved {
  patch_hash: Hash,
  approver: text,
  decision: "approve" | "reject"
}

Applied {
  manifest_hash: Hash
}
```

## Implementation Plan

### Step 1: Update `spec/02-architecture.md`

Replace the event kinds section:

```diff
- **ProposalSubmitted** {patches, proposer}
- **ShadowRunCompleted** {proposal_id, predicted_effects, diffs}
- **ApprovalRecorded** {proposal_id, approver, decision}
- **PlanApplied** {manifest_root, plan_id}
+ **Proposed** {patch_hash, author, manifest_base}
+ **ShadowReport** {patch_hash, effects_predicted, diffs}
+ **Approved** {patch_hash, approver, decision}
+ **Applied** {manifest_hash}
```

Add a note: "These names correspond to the conceptual governance steps: propose → shadow → approve → apply."

### Step 2: Verify `spec/03-air.md` matches

Ensure the Journal Entries section uses the same names and field shapes.

### Step 3: Update Rust code

Search for any enum variants or struct names that use the old longer names:

```bash
grep -r "ProposalSubmitted\|ShadowRunCompleted\|ApprovalRecorded\|PlanApplied" crates/
```

Update to match the canonical names.

### Step 4: Update any examples/tests

Search examples for governance event references and update.

## Acceptance Criteria

- [ ] `spec/02-architecture.md` uses `Proposed/ShadowReport/Approved/Applied`
- [ ] `spec/03-air.md` uses the same names with matching field shapes
- [ ] Rust enums in `aos-kernel` match the spec names
- [ ] No references to old names remain in codebase
- [ ] Example 06-safe-upgrade uses correct event names
