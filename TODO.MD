## Upcoming Kernel/AIR Tasks (ordered by priority)

1. **Raise Event Schema Alignment (High)**
   - Gap: runtime still requires `$schema` inside `raise_event` payloads and never validates the event or key against a referenced schema.
   - Why: spec §11 expects reducers to declare the event schema, the plan step to reference it, and the kernel to validate before emitting `DomainEvent`s. This keeps governance reviewers confident that raised events match their declared shapes.

2. **Validate Capability Grant Params (High)**
   - Gap: `capability.rs` CBOR-encodes `CapGrant.params` without checking them against the referenced `defcap.schema`.
   - Why: malformed grants currently slip through despite spec/JSON-schema guarantees; enforcing their schemas keeps effect gating deterministic and secure.

3. **Plan Output Enforcement & Visibility (High)**
   - Gap: `PlanStep::End` only checks presence/absence; it doesn’t validate against `plan.output`, nor do we persist surfaced results.
   - Why: typed outputs are part of AIR’s contract (spec 03 §11). We need schema validation plus a persistence path (journal or governance shadow summary) so operators and downstream automation can rely on plan outputs.

4. **Strengthen Plan Validator & Await Semantics (High/Med)**
   - Gap: `validate_plan` currently ignores await_receipt linkage, await_event schemas/where predicates, and invariant expression typing.
   - Why: catching these statically keeps invalid AIR graphs from reaching runtime and aligns runtime semantics with the spec’s deterministic guarantees.
