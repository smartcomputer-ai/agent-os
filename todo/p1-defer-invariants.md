# P1: Finish Plan Invariants for v1

**Priority**: P1 (recommended for v1)
**Effort**: Small
**Risk if deferred**: Medium (runtime already enforces them; semantics need to be explicit)

## Summary

`defplan` includes `invariants?: [Expr]` and the kernel already evaluates them after each completed step and at plan completion. They work, but the semantics and docs are fuzzy. Instead of removing the field, we will keep invariants in v1 and finish the semantics/docs to match the v1.1 design.

## Rationale

### Current problems

The spec mentions invariants but leaves key questions unanswered:

1. **When are they checked?**
   - After each step?
   - Only at plan end?
   - Both?

2. **What happens on violation?**
   - Plan aborts immediately?
   - Plan continues but logs a warning?
   - Error status propagated to parent?

3. **What can they reference?**
   - Spec says "may only reference declared locals/steps/input (no `@event`)"
   - But the validator doesn't enforce this yet

### Why defer?

- **They are already implemented** in code and tests; removing would create churn.
- **We have a clear design in spec/12**; we should align the runtime/spec with it.
- **Minimal surface area**: keeping them with clarified semantics is safer than a breaking removal.

### Target semantics (from spec/12, to adopt in v1):

> **When invariants run:**
> - After every step completes (at tick boundary)
> - At plan end, before emitting `PlanEnded`
>
> **Failure semantics:**
> - On the **first failing invariant**:
>   1. Kernel immediately ends the plan instance
>   2. Appends `PlanEnded { status: "error", result_ref: <PlanError code="invariant_violation"> }`
>   3. No further steps execute

Today the kernel raises `KernelError::PlanInvariantFailed` (no `PlanEnded` record). We need to switch to the planned `PlanEnded` error outcome.

## Proposed Change: Keep and Finish

### Implementation Plan

1. **Runtime semantics**  
   - Change invariant failure to emit `PlanEnded { status:error, result_ref: PlanError(code="invariant_violation") }` instead of surfacing a kernel error.  
   - Ensure the tick loop stops scheduling further steps after the first failure.

2. **Docs**  
   - Update `spec/03-air.md` to state timing (after each completed step and at plan end) and failure semantics (PlanEnded error with code).  
   - Call out the reference rules (no `@event`, only declared locals/steps/plan input, `@var:correlation_id` allowed).

3. **Examples**  
   - Add one positive example (invariant stays true) and one negative example (fails) in `spec/12-plans-v1.1.md` or a short snippet in `spec/03-air.md`.

4. **Tests**  
   - Kernel plan tests: assert invariant failure yields `PlanEnded{status:error, ...code="invariant_violation"}` and no further steps run.  
   - Happy-path test that invariants are evaluated after each step (e.g., failing on second step).

5. **Telemetry/Errors**  
   - Map legacy `KernelError::PlanInvariantFailed` to the new PlanEnded error path or remove that error variant once nothing else throws it.

6. **Schema/Validator**  
   - Keep the field in `defplan.schema.json`.  
   - Validator rules are already present; verify they still pass after runtime change.

## Acceptance Criteria

- [x] Invariant failure ends plan with `PlanEnded { status:error, result_ref: PlanError(code="invariant_violation") }`
- [ ] Invariants are evaluated after every completed step and at plan end (documented)
- [ ] `spec/03-air.md` describes timing, reference scope, and failure semantics; examples added
- [ ] Tests cover success and failure cases with PlanEnded behavior
- [ ] No stray uses of `KernelError::PlanInvariantFailed` remain (or it is mapped to PlanEnded)
