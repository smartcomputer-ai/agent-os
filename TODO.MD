## Upcoming Kernel/AIR Tasks (ordered by priority)

1. **Validate Capability Grant Params (High)**
   - Gap: `capability.rs` CBOR-encodes `CapGrant.params` without checking them against the referenced `defcap.schema`.
   - Why: malformed grants currently slip through despite spec/JSON schema guarantees; enforcing schemas keeps effect gating deterministic and secure.

2. **Plan Result Schema & Journaling (High)**
   - Gap: `PlanStep::End` enforces presence/absence only; values aren’t checked against `plan.output`, nor are results surfaced beyond `PlanTickOutcome`.
   - Why: typed outputs are part of AIR’s contract (spec 03 §11). We need schema validation plus a persistence path (journal or governance shadow summary) so consumers can rely on plan outputs.

3. **Raise Event Schema Alignment (High)**
   - Gap: runtime still requires `$schema` inside the raise_event payload and doesn’t validate against actual schemas.
   - Why: spec expects schema selection via step metadata; we should accept a SchemaRef, validate the payload (including key) before emitting `DomainEvent`, and drop the ad-hoc `$schema` requirement.

4. **Strengthen Plan Validator (Medium)**
   - Gap: `validate_plan` only checks DAG shape/allowed effects. It should verify await_receipt references, await_event schemas/where predicates, invariant expression types, and raise_event bindings.
   - Why: catching these statically keeps bad AIR trees from hitting runtime and aligns with the spec’s semantic-check requirements.

5. **Typed Event & Receipt Validation (Medium)**
   - Gap: awaited events/receipts are deserialized without schema checks; predicates can run on malformed data.
   - Why: validating against declared schemas ensures replay safety and lets policy/budget logic trust the payloads.

6. **Invariants in Governance & Testkit (Medium)**
   - Gap: kernel enforces invariants, but governance shadow runs and testkit scenarios don’t report/assert on them.
   - Why: reviewers need clear invariant failure diagnostics; tests should ensure invariants remain satisfied on replay.

7. **Keyed Routing Pipeline (Medium)**
   - Gap: manifest `routing.events[].key_field` isn’t used, so keyed reducers can’t receive events despite `DomainEvent.key` support.
   - Why: spec 05 (Cells) requires routing to `(reducer,key)` before we can ship cell-mode or per-key observability.

8. **Expose Plan Results to Callers (Medium)**
   - Gap: even after validation, results must be inspectable (CLI, governance shadow summary, testkit assertions).
   - Why: operators and downstream automation need the final value for audit and follow-up reducers/plans.

9. **Keyed Reducer Storage & Replay (Medium-Low)**
   - Gap: no CAS-backed per-cell state layout/snapshot story yet.
   - Why: once keyed routing lands, we’ll need storage/replay to make cells real; not urgent until routing + schema enforcement stabilize.

10. **AIR Versioning Plan (Low)**
   - Gap: code still assumes spec v1.0 while v1.1 design notes exist.
   - Why: before landing cell-mode or other v1.1 features, define how `AIR_SPEC_VERSION` increments and keep docs + code in sync. Not blocking current runtime fixes but should happen before large feature drops.
