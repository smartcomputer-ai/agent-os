# P5: Governance: Patch Schema + Control Surface

**Priority**: P5  
**Effort**: Medium  
**Risk if deferred**: Medium (governance ergonomics + correctness)

## What’s missing
- Patch format (`base_manifest_hash`, operations like `add_def`, `replace_def`, `remove_def`, `set_manifest_refs`, `set_defaults`, etc.) is only specified in prose (spec/03-air.md §15). There is no JSON Schema to validate proposals before they hit the kernel/shadow runner.
- Control/CLI path for governance verbs is underspecified. Propose/shadow/approve/apply should be first-class calls, not generic “enqueue event” plumbing.

## Why it matters
- Completes the “everything in the control plane has a schema” story; proposals become structurally validated before execution.
- Reduces kernel/shadow churn and improves diagnostics for malformed proposals.
- Keeps governance deterministic and auditable by routing through explicit kernel APIs (not generic domain events).

## Proposed work
1) **Author patch schema**: add `spec/schemas/patch.schema.json` covering the patch envelope and all op variants. Use existing schema patterns (Name format, hash refs, discriminated unions) and keep CBOR-canonical hashes.
2) **Spec update**: update `spec/03-air.md §15` to reference the schema, document invariants (single `base_manifest_hash`, op shapes, no duplicate ops per target).
3) **Kernel/tooling validation**: wire schema validation into manifest/patch load paths (store/shadow/kernels) so proposals are rejected early if they fail structure.
4) **Control channel/CLI**: add governance-specific verbs (`propose`, `shadow`, `approve`, `apply`) to the control protocol and CLI. These should:
   - validate the patch against the new schema,
   - enforce sequencing (only approve existing proposal_ids, etc.),
   - return typed results (proposal_id, manifest hashes, shadow report).
   Avoid generic “enqueue domain event” for governance.
5) **Fixtures/tests**: add round-trip fixtures for add/replace/remove/set_defaults ops and negative cases. Integration test the propose → shadow → approve → apply loop through the control channel.

## Design notes
- No semantic changes to patch ops; schema is structural and matches existing prose.
- Governance verbs remain explicit kernel calls; do not treat them as generic events in the control plane.
- If richer error info is needed, extend the schema with optional fields rather than inventing alternate payload shapes.
