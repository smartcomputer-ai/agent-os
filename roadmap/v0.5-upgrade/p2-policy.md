# p2-policy: Policy System (Current State + Work Remaining)

## TL;DR
Policies exist as a kernel gate that can Allow/Deny effects at enqueue time. The gate is wired and tested, but it is minimal: no approval flow, no budgeting/rate limits, and no journaling of policy decisions. Policies are selected via manifest defaults and apply globally.

---

## What Policies Are For (Conceptual Model)
Policies are **dynamic, governance-controlled gates** that decide whether an otherwise-capable effect should be allowed _right now_. Policies are **orthogonal** to caps:

- **Caps** answer “_can this plan/reducer ever use this kind of effect?_”
- **Policies** answer “_should we allow this particular effect emission in this world state?_”

Policies are the natural place for:
- Allow/Deny by origin (plan vs reducer), plan name, cap name, or effect kind
- Rate limits / budgets
- Approval workflows (RequireApproval)
- Governance visibility (policy decisions should be journaled)

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
- First matching rule decides; default is Deny if no rule matches.

Relevant code:
- `crates/aos-kernel/src/policy.rs`

### 3) Policies are selected via `manifest.defaults.policy`
- If a policy name is set in defaults, the kernel builds a `RulePolicy` gate from it.
- Otherwise, the kernel uses `AllowAllPolicy`.

Relevant code:
- `crates/aos-kernel/src/world.rs`

### 4) Tests exist for allow/deny paths
- Integration tests cover reducer/plan allow/deny cases.

Relevant code:
- `crates/aos-host/tests/policy_integration.rs`

---

## What Is Not Implemented (Gaps)

1) **No approval path (RequireApproval) wired**
   - Policies can only Allow or Deny. There is no policy outcome that triggers governance approval.

2) **No policy decision journaling**
   - There is a `PolicyDecision` journal record type, but decisions are not recorded in the journal.
   - This makes audit and replayability of policy outcomes incomplete.

3) **No rate limit / budget counters**
   - Policies do not track per-cap/per-plan quotas or time-based limits.

4) **No per-plan policy override**
   - Policies are only global via defaults. There is no policy selection per plan or per cap grant.

5) **No policy evaluation context beyond intent + origin**
   - The policy gate currently sees intent, grant, and origin; no world state, no governance state, no time source.

6) **No policy enforcement for non-effect actions**
   - Policies only gate effect enqueue; they do not gate plan scheduling or other kernel actions.

---

## Interaction With Other Systems

### Caps
- Policies run **after** cap resolution but before enqueue.
- This ordering is good: caps establish “can ever do X,” policy says “allow this instance.”

### Secrets
- Secret ACLs are enforced separately in `effects.rs` before policy.
- Secret policy is a separate mechanism (allowed_caps/allowed_plans) and is not driven by DefPolicy.

### Governance (propose/shadow/approve/apply)
- Policies live in the manifest, so shadow runs already apply the candidate policy.
- There is no policy that gates governance actions themselves (separate TODO in governance caps).

---

## Minimal “Working Policy System” Requirements

To make policies meaningful beyond basic allow/deny:

1) **Journal policy decisions**
   - Record policy decisions in the journal when an effect is enqueued or denied.
   - This keeps replay deterministic and supports audits.

2) **Add a RequireApproval decision type**
   - Introduce `PolicyDecision::RequireApproval` (or similar) in both AIR and runtime.
   - The kernel should enqueue a governance approval request rather than deny.

3) **Budget / rate limit enforcement**
   - Policies should be able to decrement counters (per cap, per plan, per origin).
   - This may share infrastructure with cap budgets, but conceptually belongs in policy for dynamic limits.

4) **Expose deterministic time/sequence to policy rules**
   - Policies need a deterministic notion of time (sequence number, logical time window) for rate limits.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) **Plan allowlist**
   - Policy allows only `plan == "com.acme/Plan@1"` for `http.request`.
   - Another plan emitting the same effect is denied.

2) **Rate limit (plan-local)**
   - Policy allows only N HTTP effects per window per plan.
   - Exceeding N yields Deny or RequireApproval.

3) **Approval-gated effect**
   - Policy returns RequireApproval for `payment.charge`.
   - After approval, the plan resumes and effect is enqueued.

---

## Where Policy Logic Should Live

Policies should be enforced in the **kernel** to preserve determinism and auditability. The policy decision must be:
- deterministic
- journaled
- replayable

Adapters should not apply policy logic beyond safety/operational checks.

---

## Summary of Required Work

1) Add **policy decision journaling**.
2) Add **RequireApproval** outcome and governance flow integration.
3) Implement **policy budgets/rate limits** with deterministic counters.
4) Expand policy context (deterministic time/sequence). 
5) Add tests and fixtures for the minimal use-cases above.

Once these exist, policies will have clear, enforceable governance value beyond static allow/deny rules.
