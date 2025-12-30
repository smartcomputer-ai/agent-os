# Example 09 — WorldFS Lab (Notes + Catalog)

A keyed notebook reducer plus ObjectCatalog and a small plan. Finalized notes trigger a plan that writes a report blob and registers it in the catalog, originally meant to be explored via the `aos world fs` CLI.

## What it does
- Keyed reducer `notes/NotebookSM@1` owns one note per key.
- Runner sends `notes/NoteEvent@1` variants (Start/Append/Finalize) to seed notes.
- `NoteFinalized` emits `SnapshotRequested`; plan `notes/SnapshotPlan@1` does:
  1. `blob.put` the report.
  2. Raise `sys/ObjectRegistered@1` (object `notes/<id>/report`, kind `note.report`, tags `report,worldfs`).
  3. Raise `NoteArchived` to close the cell.
- Runner seeds two notes (alpha, beta), drives blob receipts, and verifies replay.

## Run it
```
cargo run -p aos-examples -- worldfs-lab
```

If you change schemas/manifests or rerun after a code edit, wipe any stale journal/store first (old entries won’t match new schemas):
```
rm -rf .aos
```

## Note on CLI
The experimental `aos world fs` CLI has been removed. The example still builds and runs, but there is currently no supported CLI wrapper to browse the catalog or blobs. You can inspect the journal/store directly or wait for the upcoming replacement commands (object and blob readers) to land.

## Layout
```
examples/09-worldfs-lab/
  air/           # schemas, manifest, plan, caps, policies
  reducer/       # Notebook reducer (wasm built via aos-wasm-build)
  README.md
```
