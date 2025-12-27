# p2-policy: Policy System (Constraints-Only, v0.5)

## TL;DR
Policies are a kernel gate that can Allow/Deny effects at enqueue time. Decisions are journaled. Policies are selected via manifest defaults and apply globally; if no default is set, the kernel allows all. v0.5 policy scope is deliberately small: **no approvals, no counters/limits, no external policy engines**. The only remaining scope item is adding `cap_type` matching to reduce rule explosion.

---

## Diamond Invariants (Design Spine)

1) **One authoritative authorizer, one deterministic transaction**
   - Kernel authorization is canonical:
     - canonicalize params -> cap check -> policy check -> journal decisions -> enqueue or deny
   - Any future mutation (approvals, counters) must be part of the same deterministic, journaled transaction.

2) **Caps and policy stay orthogonal in shape**
   - Caps answer: "can ever".
   - Policies answer: "should now".
   - The kernel composes them in fixed order: cap -> policy.

3) **Every decision is explainable and replayable**
   - Allow/deny outcomes must be derivable from the journal.
   - Journal the matched rule index.

---

## What Policies Are For (Conceptual Model)
Policies are **dynamic, governance-controlled gates** that decide whether an otherwise-capable effect should be allowed _right now_. They are orthogonal to caps:

- **Caps**: static authority and constraints.
- **Policies**: dynamic allow/deny.

Policies are the natural home for:
- allow/deny by origin (plan/reducer), plan name, cap name, effect kind
- auditability of governance decisions

For v0.5, policy is constraints-only: allow/deny only, no approvals, no counters.

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

### 4) Policy decisions are journaled
- Each decision records `{intent_hash, policy_name, rule_index, decision}`.

Relevant code:
- `crates/aos-kernel/src/effects.rs`
- `crates/aos-kernel/src/journal/mod.rs`

### 5) Tests exist for allow/deny paths
- Integration tests cover reducer/plan allow/deny cases.

Relevant code:
- `crates/aos-host/tests/policy_integration.rs`

---

## What Is Not Implemented (Gaps)

1) **No per-plan policy override**
   - Only a global default policy exists.

2) **Policy context is minimal**
   - The gate sees only intent + origin; no deterministic time, plan_id, manifest hash, or correlation.

3) **Denial is fatal by default**
   - Deny results in enqueue failure rather than a controlled plan-level outcome.

---

## Deterministic Policy Context (Safe Inputs)

v0.5 keeps the context minimal (origin + effect + cap). If we expand it later, only add deterministic values such as:

- `origin_kind`, `origin_name`
- `plan_id`, `step_id`
- `manifest_hash`
- `journal_height` / `logical_now_ns` (see `roadmap/v0.5-caps-policy/p3-time.md`)
- `cap_name`, `cap_type`
- correlation id (if present)

Avoid wallclock or mutable host state inside the policy gate.

---

## Minimal "Working Policy System" Requirements

1) **Policy decision journaling**
   - Always record allow/deny decisions.

2) **Minimal deterministic context**
   - Keep context to origin + effect + cap unless new policy features require more.

---

## Proposed Minimal Use-Cases (Tests/Examples)

1) **Plan allowlist**
   - Policy allows only a named plan for `http.request`.

## Where Policy Logic Should Live

Policies must be enforced **in the kernel** (authoritative, deterministic, journaled). Adapters should never decide policy outcomes; at most they provide operational safety checks.

---

## Build Order (Minimal to Meaningful)

1) Add `cap_type` matching to policy rules
2) Keep policy context minimal until approvals/limits exist

---

## Required Spec/Schema Updates (Scope-Only)

1) **defpolicy**: add optional `cap_type` match field.
2) **Rule matching**: implement `cap_type` match in RulePolicy.

---

## Summary of Required Work

1) Add `cap_type` match to `defpolicy` rules.
2) Add/adjust tests for `cap_type` matching as needed.

---

## FAQ (Current Questions)

### If we have cap enforcers, do we still need policy?
Yes. Caps constrain **delegated authority** (“can ever”). Policy is a **governance gate** (“should now”). Collapsing them would reintroduce cap semantics inside policy and weaken least-privilege reasoning.

### What about performance?
Rule matching is fast (a few string compares). The incremental cost is tiny compared to canonicalization/journaling and any real external I/O. Optimize later if profiling shows hot paths.
