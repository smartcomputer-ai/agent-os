# P1: Defer await_event to v1.1 (superseded)

**Update (decision)**: We are **not removing** `await_event`. Instead we clarified its v1 semantics, required correlation predicates when triggers set `correlate_by`, and added documentation + tests to make its role explicit. The feature stays, but is now better specified and guarded.

**Priority**: P1 (recommended for v1)
**Effort**: Small-Medium
**Risk if deferred**: Low (docs/spec guardrails already landed)

## Summary

The `await_event` plan step allows plans to wait for domain events. The original proposal was to defer/remove it. After review, we kept it and:
1. Documented exact semantics (future-only, first match per waiter, broadcast, predicate scope, correlation guard)
2. Required a `where` predicate when the plan was started with `correlate_by` (runtime guard + validation allowance for `correlation_id`)
3. Updated specs/workflows to state why `await_event` matters (keeps a single plan’s locals/invariants across multiple domain events)

So the “defer” recommendation is superseded by clarifying and tightening the feature in v1.

## Rationale

### Underspecified semantics (now resolved in docs/runtime)

- Future vs historical: clarified as future-only (wait registered at step activation).
- Multiple matches: first matching event by journal order resumes the waiter.
- Racing waiters: broadcast; events are not consumed.
- Correlation: `correlate_by` key is injected as `@var:correlation_id`; `await_event` now requires a `where` when correlated.

### Examples/tests coverage

Runtime + validator tests now cover `await_event`; we still lack an `examples/` sample, but kernel/testkit suites exercise it.

### Core pattern vs await_event

The trigger→plan→raise_event loop is sufficient for many flows, but `await_event` is valuable when a single plan needs to span multiple domain events while keeping its locals/invariants intact (e.g., approvals, chained orchestration). Removing it would force extra reducer state or plan hand-offs. We chose to keep it and clarify semantics instead of deferring.

### What remains for v1.1+

- Add an `examples/` scenario showcasing correlated `await_event`.
- Consider multi-wait/structured concurrency extensions once real workloads demand it.

## Implementation Notes (what we shipped)

- Kept schema/type/runtime support for `await_event`.
- Added runtime guard: correlated runs must supply `where` on `await_event`.
- Validator now whitelists `correlation_id` so predicates can reference it.
- Specs updated: `spec/02-architecture.md`, `spec/03-air.md`, `spec/05-workflows.md` (semantics, rationale, when-to-use, correlation guidance).
- New unit test ensures guard triggers when missing predicate in correlated runs.

## Acceptance Criteria (updated)

- [x] `await_event` semantics documented (future-only, first match, broadcast, predicate scope, correlation guard)
- [x] Docs explain why `await_event` is retained and when to use it vs multi-plan choreography
- [x] Runtime rejects correlated runs without an `await_event.where` predicate
- [x] Validator allows `@var:correlation_id` in predicates
- [x] Workflow doc example updated to correlation-aware `await_event`
- [x] Tests updated/added and passing
