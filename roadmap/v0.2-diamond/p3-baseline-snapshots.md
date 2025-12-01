# P3: Baseline Snapshots & GC Groundwork

## Context / Rationale
- Today snapshots are a speed optimization only: the kernel records `{snapshot_ref, height}` and restores by loading the latest snapshot then replaying the full journal tail. Manifest alignment is implicit, CAS reachability is implicit, and the journal is a single append-only file.
- To enable single-world GC we need snapshots to become **semantic baselines** (“new genesis”) so a world can be reconstructed from `baseline snapshot + journal tail >= baseline_height`, and we need explicit roots for later CAS mark/sweep.
- No backward compat required (pre-alpha, no dev worlds). We can bump formats and layout now to avoid painful migrations later.

## Goals for this pass (groundwork, not full GC)
1) Make snapshots self-describing roots (control-plane + runtime) so restore is deterministic without older history.  
2) Add baseline metadata so the runtime can “start from here” and future tooling can truncate/archival safely.  
3) Segment the journal to permit future truncation/archiving.  
4) Expose a root-set API for future CAS GC.  
5) Add invariants/tests that enforce the new semantics (even if deletion is not wired yet).

## Proposed design

### A. Snapshot envelope v2 (self-describing)
- New struct stored as a CAS **node**, not just a blob:  
  `SnapshotEnvelope { version: u16, height, manifest_hash, snapshot_ref, pinned_blob_roots: Vec<Hash>, created_at, reducer_state_ref: Option<Hash> }`
- `KernelSnapshot` (runtime payload) gains `manifest_hash` and optionally `reducer_state_ref` (keep reducer state inline for now; field readied for future dedup).
- `journal::SnapshotRecord` remains `{snapshot_ref, height}` but the referenced blob now encodes the envelope; the envelope points to the runtime snapshot bytes (`snapshot_ref`).
- Restore path: load envelope → ensure manifest hash is present in store (load if needed) → apply snapshot → replay tail >= `height`.
- Rationale for a separate envelope: keeps the journal format stable and tiny (height + ref) while letting snapshot metadata evolve (roots, provenance, dedup pointers) without forcing journal rewrites; GC can parse roots from the envelope node without peeking into journal internals.

### B. Baseline metadata (per world)
- File: `.aos/world/baseline.json` with `{ baseline_snapshot_ref, baseline_height, manifest_hash, set_at }`.
- Promotion flow: choose an existing snapshot, write metadata, rotate journal segment (see C). No deletion yet.
- Restore contract: fail if journal contains entries `< baseline_height`; require manifest hash match or reload from CAS.

### C. Segmented journal
- Replace single `journal.log` with numbered segments (`journal/00000.log`, `00001.log`, …) and track `start_seq` per segment.
- On promotion, open a fresh segment starting at `baseline_height + 1`. Future truncation = delete/arch segments `< baseline_height`.
- `MemJournal` API unchanged; introduce `SegmentedFsJournal` impl behind the `Journal` trait.

### D. Root-set extractor (stub for CAS GC)
- New helper (likely in `aos-store` or `aos-kernel::gc`) that returns `RootSet { nodes: HashSet<Hash>, blobs: HashSet<Hash> }` for a world:
  - `manifest_hash` from baseline metadata and any newer Applied governance records in the tail.
  - `baseline_snapshot_ref` (+ optionally a small ring of retained snapshots).
  - All hash fields in journal tail records (snapshot refs, patch hashes, manifest_hash_new, effect payload refs if we add them later).
  - Optional operator-specified “pinned” hashes (modules, compliance artefacts).
- This will feed a later mark/sweep; for now it just computes and logs/returns.

### E. Invariants & tests
- Snapshot creation must include manifest hash and reference existence checks.
- Restore must: (1) assert tail starts >= snapshot height; (2) enforce manifest hash alignment (reload manifest if absent); (3) error if baseline metadata exists and tail violates it.
- Tests:
  - Governance patch → snapshot → promotion → restore → state matches without replaying pre-baseline governance events.
  - Reject tail that starts before baseline height.
  - Journal rotates on promotion.

### F. CLI/ops affordances (thin)
- `aos world promote-snapshot <ref>` → writes baseline metadata + rotates journal segment.
- `aos world roots` → prints root counts/hashes (uses root-set extractor).
- `aos world check-baseline` → runs invariants, non-zero on violation.

## Work items (ordered)
1) Bump snapshot format: add `manifest_hash` field to `KernelSnapshot`; create `SnapshotEnvelope` node type; update encode/decode + tests.  
2) Baseline metadata struct + read/write helpers; enforce restore contract in `replay_existing_entries`.  
3) Implement `SegmentedFsJournal` (write/read length-prefixed CBOR per segment); wire into `KernelBuilder::with_fs_journal`.  
4) Add “promote snapshot” hook in kernel/testkit (no deletion).  
5) Root-set extractor returning `RootSet` (walk baseline + tail records, plus optional pins).  
6) Tests for restore invariants, promotion rotation, and governance-aligned snapshot restore.  
7) CLI commands (thin wrappers) to call promote/roots/check-baseline.

## Non-goals (future passes)
- Actual journal truncation/archival policy.  
- CAS mark/sweep and deletion.  
- Per-adapter TTL fencing for late receipts (add once GC deletes history).  
- Snapshot dedup of reducer state blobs.  
- Cross-world CAS accounting.
