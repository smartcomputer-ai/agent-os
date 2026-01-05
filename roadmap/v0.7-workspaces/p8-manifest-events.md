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
- CLI now finds the boot manifest by scanning the journal:
  - prefer the latest snapshot's `manifest_hash`,
  - otherwise use the latest `Manifest` record.
- Tests updated to account for the new manifest record occupying seq 0 in new journals.

## Current behavior (gap)

Replay treats `JournalRecord::Manifest` as a no-op. That means the kernel uses
the latest manifest for the entire replay, even if the journal contains
historical upgrades. This is incorrect because earlier events may require
schemas/modules from earlier manifests.

## Required changes

1) **Apply manifest records during replay**
   - When `JournalRecord::Manifest` is encountered, load that manifest and swap
     the kernel manifest in-place.
   - Do not append new journal entries during replay (avoid recursive manifest
     records). This likely needs a replay-safe swap path.
   - Recompute schema index, reducer schemas, cap bindings, and effect manager
     so subsequent events validate correctly.

2) **Snapshot interaction**
   - If a snapshot exists, start with the snapshot's `manifest_hash`.
   - While replaying entries after the snapshot, apply any later manifest
     records in order.

3) **Tests**
   - Replay with multiple manifest evolutions (event schemas change between
     upgrades) should pass and end on the latest manifest.
   - Snapshot + later manifest update should swap manifests during replay and
     continue processing events.

4) **Introspection/diagnostics (optional)**
   - Expose manifest record info for debugging (e.g., journal scan or `ws`/`gov`
     inspection commands).

## Related changes in this workstream (context)

- Workspace sync is now `aos push`/`aos pull` with `aos.sync.json` as the map
  file (see `roadmap/v0.7-workspaces/p7-fs-sync.md`).
- Workspace annotations can be strings or JSON, stored as UTF-8 text or
  canonical CBOR; `aos ws ann get` displays CBOR values as JSON.
