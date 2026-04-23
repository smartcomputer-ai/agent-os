# P5: Cleanup

P4 owns the builtin workflow runtime migration:

- `roadmap/v0.22-dx/p4-builtin-workflow-runtime.md`

P5 should stay limited to residual cleanup after the P4 implementation.

## Follow-Up Cleanup

1. Remove any stale docs outside the v0.22 roadmap that still mention building system workflow
   WASM.
2. Delete obsolete `.aos/cache/sys-modules` migration notes or fixtures if they remain.
3. Re-check examples and smoke fixtures for old `sys/*_wasm@1` module names.
4. Remove any historical `aos-sys` references that are not intentionally documenting the P4
   migration.
