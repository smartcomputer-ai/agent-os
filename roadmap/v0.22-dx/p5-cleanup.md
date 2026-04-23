# P5: Cleanup

P4 now owns the builtin workflow runtime design:

- `roadmap/v0.22-dx/p4-builtin-workflow-runtime.md`

P5 should stay limited to residual cleanup after P4 lands.

## Follow-Up Cleanup

1. Remove any stale docs that still mention building system workflow WASM.
2. Remove any leftover `aos-sys` references if the crate removal is not completed in P4.
3. Delete obsolete `.aos/cache/sys-modules` test fixtures or migration notes if they remain.
4. Re-check examples and smoke fixtures for old `sys/*_wasm@1` module names.
