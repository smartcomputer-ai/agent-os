# p2-policy: Policy System (Current State + Work Remaining)

## TL;DR
Policies are wired as a kernel gate that can Allow/Deny effects at enqueue time, but the system is minimal: no approvals, no rate limits/budgets, and no journaling of decisions. Policies are selected via manifest defaults and apply globally. To become governance-grade, policies must be journaled, approval-capable, and deterministic.

---

## Diamond Invariants (Design Spine)

1) **One authoritative authorizer, one deterministic transaction**
   - Kernel authorization is canonical:
     - canonicalize params -> cap check -> policy check -> journal decisions -> enqueue or deny
   - Any mutation (counters, approvals, reservations) must be part of the same deterministic, journaled transaction.

2) **Caps and policy stay orthogonal in shape**
   - Caps answer: "can ever".
   - Policies answer: "should now".
   - The kernel composes them in fixed order: cap -> policy.

3) **Every decision is explainable and replayable**
   - Allow/deny/require-approval outcomes must be derivable from the journal.
   - Journal the matched rule index and rationale.

---

## What Policies Are For (Conceptual Model)
Policies are **dynamic, governance-controlled gates** that decide whether an otherwise-capable effect should be allowed _right now_. They are orthogonal to caps:

- **Caps**: static authority and constraints.
- **Policies**: dynamic allow/deny/approval and rate limits.

Policies are the natural home for:
- allow/deny by origin (plan/reducer), plan name, cap name, effect kind
- rate limits and counters
- approval workflows
- auditability of governance decisions

---

## Current Implementation (What Works Today)

### 1) Policy gate is wired into effect enqueue
- `EffectManager::enqueue_effect` calls the policy gate after cap resolution.
- If policy returns Deny, enqueue fails.

Relevant code:
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-kernel/src/policy.rs`

### 2) RulePolicy supports Allow/Deny rules only
- `DefPolicy` rules match on:
  - effect kind
  - cap name
  - origin kind (plan/reducer)
  - origin name
- First match wins; default is Deny.

Relevant code:
- `crates/aos-kernel/src/policy.rs`

### 3) Policies are selected via `manifest.defaults.policy`
- If set, kernel builds a `RulePolicy` gate from that defpolicy; otherwise it uses `AllowAllPolicy`.

Relevant code:
- `crates/aos-kernel/src/world.rs`

### 4) Tests exist for allow/deny paths
- Integration tests cover reducer/plan allow/deny cases.

Relevant code:
- `crates/aos-host/tests/policy_integration.rs`

---

## What Is Not Implemented (Gaps)

1) **No approval outcome (RequireApproval)**
   - Policies can only Allow or Deny.

2) **No policy decision journaling**
   - A `PolicyDecision` record exists but is never written.

3) **No rate limit / budget counters**
   - No deterministic counters or rate-limit mechanism.

4) **No per-plan policy override**
   - Only a global default policy exists.

5) **Policy context is minimal**
   - The gate sees only intent + origin; no deterministic time, plan_id, manifest hash, or correlation.

6) **Denial is fatal by default**
   - Deny results in enqueue failure rather than a controlled plan-level outcome.

---

## Governance-Grade Semantics (Target Behavior)

### 1) Journal every policy decision
For each attempted emission, record a policy decision with:
- `intent_hash`
- `policy_name`
- matched rule index (or "default")
- decision (allow/deny/require_approval)
- rationale / error details

This makes decisions explainable without re-running logic.

### 2) RequireApproval as a third outcome (with suspension)
Diamond semantics for RequireApproval:

- `emit_effect` does **not** fail the plan.
- The plan step transitions to a **blocked** state.
- The kernel journals an `ApprovalRequested` record keyed by a stable id (often `intent_hash`).
- An approval event/receipt arrives, is journaled, and unblocks the step.
- Only then does the kernel enqueue the effect (or deny definitively).

### 3) Rate limits and counters via a deterministic ledger
Policies should declare counters, but use shared deterministic ledger infrastructure:

- Cap budgets: monotone decreasing, topped up via governance.
- Policy counters: token buckets or windows, replenished via deterministic time/epoch.

---

## Deterministic Policy Context (Safe Inputs)

Expand the policy context only with deterministic values, e.g.:

- `origin_kind`, `origin_name`
- `plan_id`, `step_id`
- `manifest_hash`
- `journal_height` / `logical_time`
- `cap_name`, `cap_type`
- correlation id (if present)

Avoid wallclock or mutable host state inside the policy gate.

---

## Optional Ergonomic Upgrade: Denial as a Synthetic Receipt

Instead of making denial a hard error, treat denial/approval-required as synthetic receipts:

- `emit_effect` returns a receipt-like record `{status: denied | approval_required | ok | error}`
- `await_receipt` consumes the result and drives plan branching

This keeps governance outcomes explicit and makes flows resilient without weakening security.

---

## Minimal "Working Policy System" Requirements

1) **Policy decision journaling**
   - Always record allow/deny/approval-required decisions.

2) **RequireApproval outcome**
   - Suspend the plan step; unblock only after approval event/receipt.

3) **Rate limits / budgets**
   - Deterministic counters with clear replenishment semantics.

4) **Deterministic context**
   - Provide logical time/sequence and stable identifiers.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) **Plan allowlist**
   - Policy allows only a named plan for `http.request`.

2) **Rate limit**
   - Policy allows N HTTP requests per logical window per plan.

3) **Approval-gated effect**
   - `payment.charge` requires approval; plan blocks then resumes on approval.

---

## Where Policy Logic Should Live

Policies must be enforced **in the kernel** (authoritative, deterministic, journaled). Adapters should never decide policy outcomes; at most they provide operational safety checks.

---

## Build Order (Minimal to Meaningful)

1) Journal allow/deny policy decisions
2) Add RequireApproval + plan suspension model
3) Add deterministic counters / rate limits
4) Expand deterministic policy context
5) Optional: denial as synthetic receipts for better ergonomics

---

## Summary of Required Work

1) Journal policy decisions with rationale and rule index.
2) Implement RequireApproval with a blocked-step model.
3) Implement deterministic counters / rate limits.
4) Expand deterministic policy context.
5) Add tests/fixtures for the minimal use-cases.

Once these exist, policies become governance-grade and auditable rather than a simple filter.
