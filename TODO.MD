## Recently Completed

1. **Raise Event Schema Alignment**
   - Kernel now infers reducer event/key schemas via `ReducerSchema`, canonicalizes plan literals, and validates before journaling. Specs, fixtures, and plan tests were updated to drop `$schema` payloads.

2. **Capability Grant Param Validation**
   - `CapabilityResolver::from_manifest` expands `defcap.schema` references through the shared `SchemaIndex` and type-checks each grant before CBOR encoding; helper tests cover both direct schemas and refs.

3. **Plan Output Enforcement & Visibility**
   - Plan `End` steps now canonicalize + validate outputs against `plan.output`, persist the canonical value (journal + snapshot), and expose recent outputs through the kernel/testkit for operators and replay integration tests.

4. **Strengthen Plan Validator & Await Semantics**
   - `validate_plan` now enforces await→emit linkage, rejects duplicate effect handles, type-checks await_event predicates, and ensures invariants only reference declared vars/steps, preventing a raft of runtime invariant/await errors from entering the manifest.

5. **Manifest/Test Fixtures Schema Coverage**
   - `fixtures::build_loaded_manifest` now auto-registers placeholder schemas for plan inputs/outputs, triggers, reducer ABIs, and routing events so every helper manifest is schema-complete by default. Tests still opt-in with richer schema shapes via `insert_test_schemas`, but nothing ships without the references wired into the manifest catalog.

6. **Governance/Journaling Docs Refresh**
   - `spec/03-air.md` and `spec/05-workflows.md` now document the enforced schema boundaries, `PlanResult` journal entries, and how plan outputs become visible to governance/shadow tooling, closing the loop between runtime behavior and operator documentation.

## Upcoming Kernel/AIR Tasks (ordered by priority)

_(Nothing queued — add the next kernel/AIR task above as it emerges.)_
