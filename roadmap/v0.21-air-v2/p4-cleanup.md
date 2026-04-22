# P4: AIR v2 Cleanup

Status: planned.

## Goal

Finish correctness and cleanup work intentionally deferred while P1-P3 move the core model,
runtime identity, and repo fixtures to AIR v2.

## Work

- [ ] Validate secret references in effect params against active AIR v2 declarations:
  - Inspect each active effect op's params schema for admitted secret-reference positions.
  - Ensure secret refs used in effect params resolve through active `manifest.secrets` `defsecret`
    declarations.
  - Keep public AIR free of `defsecret.allowed_ops`; any stronger per-secret policy remains
    node-local/runtime policy.
  - Add focused tests for valid secret refs, missing secret declarations, and refs in schema
    positions that do not admit secrets.

## Notes

- This was originally listed in P1 semantic validation, but is cleanup-sized compared with the
  core schema/catalog/model cut and should not block the fixture/runtime migration phases.
