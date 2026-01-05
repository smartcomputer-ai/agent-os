# P8: Manifest Events

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (replay can be invalid without correct manifest)  
**Status**: In progress

## Goal

Track manifest changes in the journal so:
- Worlds boot without local manifest files (journal/snapshots are the source of truth).
- Replay uses the correct manifest at each journal height.
- Upgrades through governance or `aos push` are auditable and deterministic.

## Implemented so far

- Added `JournalKind::Manifest` and `ManifestRecord { manifest_hash }`.
- Kernel records a manifest when:
  - the journal is empty on first boot (after replay),
  - the manifest changes via `swap_manifest` (governance apply or `aos push`).
- Replay now applies manifest records in-order:
  - `JournalRecord::Manifest` loads the referenced manifest and swaps it in
    without emitting new records.
  - Snapshot boot loads the snapshot manifest first, then applies later
    manifest records as replay proceeds.
  - Manifest loads are skipped if the hash matches the active manifest.
- Added `apply_loaded_manifest` helper for replay-safe swaps.
- Manifest persistence now canonicalizes refs:
  - `persist_loaded_manifest` stores defs, updates manifest named refs to the
    actual stored hashes, and then writes the manifest/`AirNode::Manifest`.
  - Builtin modules can be overridden when a non-builtin hash is provided.
  - `canonicalize_patch` normalizes patch manifest refs against patch nodes and
    builtins to keep patch hashes stable.
- Tests updated/added:
  - Kernel replay tests cover manifest upgrades and snapshot + upgrade.
  - Governance tests use canonicalized patch hashes.
  - Snapshot tests updated for the TimerFired event schema variant.

## Remaining work

1) **Finish effect params normalization test**
   - Update the reducer ABI event schema in
     `crates/aos-host/tests/effect_params_normalization.rs` to use a variant
     schema that includes both `Start` and `sys/TimerFired@1`.
   - Add the variant schema definition to the test manifest so the replay
     validator accepts timer receipts.

2) **Diagnostics (optional)**
   - Expose manifest record info for debugging (e.g., journal scan or `ws`/`gov`
     inspection commands).

## Related changes in this workstream (context)

- Workspace sync is now `aos push`/`aos pull` with `aos.sync.json` as the map
  file (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).
- Workspace annotations can be strings or JSON, stored as UTF-8 text or
  canonical CBOR; `aos ws ann get` displays CBOR values as JSON.
