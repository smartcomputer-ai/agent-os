## Recently Completed

1. **Raise Event Schema Alignment**
   - Kernel now infers reducer event/key schemas via `ReducerSchema`, canonicalizes plan literals, and validates before journaling. Specs, fixtures, and plan tests were updated to drop `$schema` payloads.

2. **Capability Grant Param Validation**
   - `CapabilityResolver::from_manifest` expands `defcap.schema` references through the shared `SchemaIndex` and type-checks each grant before CBOR encoding; helper tests cover both direct schemas and refs.

## Upcoming Kernel/AIR Tasks (ordered by priority)

1. **Plan Output Enforcement & Visibility (High)**
   - Gap: `PlanStep::End` only checks presence/absence; it doesn’t validate against `plan.output`, nor do we persist surfaced results.
   - Why: typed outputs are part of AIR’s contract (spec 03 §11). We need schema validation plus a persistence path (journal or governance shadow summary) so operators and downstream automation can rely on plan outputs.

2. **Strengthen Plan Validator & Await Semantics (High/Med)**
   - Gap: `validate_plan` currently ignores await_receipt linkage, await_event schemas/where predicates, and invariant expression typing.
   - Why: catching these statically keeps invalid AIR graphs from reaching runtime and aligns runtime semantics with the spec’s deterministic guarantees.

3. **Manifest/Test Fixtures Schema Coverage (Med)**
   - Gap: several helper manifests still synthesize reducers/events without registering their `defschema` nodes or reducer ABI metadata (recent journal fixes highlighted this).
   - Why: keeping fixtures aligned with runtime expectations prevents silent drift and ensures schema-indexed features (raise_event validation, capability refs) stay exercised by integration tests.

4. **Governance/Journaling Docs Refresh (Med)**
   - Gap: spec patches mention raise_event/cap validation, but `spec/03-air.md` and `spec/05-workflows.md` still need an explicit “runtime enforces schemas” callout in governance + journaling sections, and TODO should reference the new workflow.
   - Why: keeps implementers and reviewers aware of the enforced boundaries and the audit story when reviewing manifests or plan diffs.
