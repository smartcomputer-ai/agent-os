# P3: Add JSON Schema for Patches

**Priority**: P3 (later)
**Effort**: Medium
**Risk if deferred**: Low (only tooling ergonomics)

## What’s missing
- Patch format (`base_manifest_hash`, operations like `add_def`, `replace_def`, `remove_def`, `set_manifest_refs`, `set_defaults`, etc.) is only specified in prose (spec/03-air.md §15).
- No JSON Schema to validate patch documents before hitting the kernel/shadow runner.

## Why it matters (later)
- Completes the “everything in control plane has a schema” story.
- Gives agents/tooling early structural validation and better error messages.
- Reduces kernel/shadow churn on malformed proposals.

## Proposed work
1) Author `spec/schemas/patch.schema.json` covering the patch document and all patch op variants.
2) Update spec/03-air.md §15 to reference the schema.
3) Wire schema validation into store/shadow tooling (similar to defplan/manifest validation path).
4) Add fixtures/tests that round-trip a few patch examples (add_def, replace_def, remove_def, set_defaults).

## Notes
- No semantic changes to patch ops; schema is structural.
- Keep hashes CBOR-canonical; follow existing schema patterns (common defs where possible).
- Defer until we prioritize polish for the governance loop UX.
