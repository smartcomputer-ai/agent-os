# Example 09 — WorldFS Lab (Notes + Catalog)

A keyed notebook reducer plus ObjectCatalog and a small plan. Finalized notes trigger a plan that writes a report blob and registers it in the catalog, giving `aos world fs` something real to explore under `/sys`, `/obj`, and `/blob`.

## What it does
- Keyed reducer `notes/NotebookSM@1` owns one note per key.
- `NoteFinalized` emits `SnapshotRequested`; plan `notes/SnapshotPlan@1` does:
  1. `blob.put` the report (namespace = note_id).
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

## Play with WorldFS CLI
After the run, in `examples/09-worldfs-lab` try:
```
aos world fs ls /sys/reducers/NotebookSM@1

aos world fs cat /sys/reducers/NotebookSM@1/alpha

aos world fs ls /obj --long

aos world fs cat /obj/notes/alpha/report/data

aos world fs stat /obj/notes/alpha/report

aos world fs tree /obj
```
These exercise `introspect.*`, `list_cells`, `blob.get`, and catalog reads with provenance metadata.

## Layout
```
examples/09-worldfs-lab/
  air/           # schemas, manifest, plan, caps, policies
  reducer/       # Notebook reducer (wasm built via aos-wasm-build)
  README.md
```
