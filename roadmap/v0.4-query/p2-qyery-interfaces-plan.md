**Survey Findings**

- Kernel already exposes Kernel::reducer_state_bytes and list_cells, plus heights/journal_head, but returns raw bytes only—no consistency controls or metadata. State is always “head”; no notion of Exact/AtLeast.
- Snapshots/journal: SnapshotRecord stores only {snapshot_ref, height}; KernelSnapshot omits manifest hash/ref. Kernel tracks last_snapshot_height but not the snapshot hash or manifest hash, so we can’t return the (journal_height, snapshot_hash, manifest_hash) tuple the spec requires.
- Replay path exists (replay_existing_entries, load_snapshot, apply_snapshot) and uses suppress_journal for read-only replay, which we can reuse for building read views.
- Host/control layer: daemon ControlMsg::QueryState and CLI aos world state/list-cells just call WorldHost::state() (bytes) and don’t surface metadata or consistency options; no HTTP/REST read surface or capability gating for reads.
- Capability/policy system only gates effect adapters; there is no path yet for gating observational reads.

**Implementation Plan (p2-query-interfaces)**

1. **Add read-API types in kernel**
    
    - Define Consistency (Head | Exact(JournalSeq) | AtLeast(JournalSeq)), ReadMeta { journal_height, snapshot_hash: Option<Hash>, manifest_hash: Hash }, and response envelopes (StateRead<T> { meta, value: Option<T> }).
    - Introduce a StateReader trait (in aos-kernel, likely query module) with get_reducer_state(module: Name, key: Option<&[u8]>, consistency), get_manifest(consistency), and get_journal_head().
2. **Track manifest/snapshot hashes**
    
    - Compute and store manifest_hash on kernel load and after apply/shadow swaps; expose accessor.
    - Extend snapshot creation to record the snapshot hash in memory and persist it (add field to KernelSnapshot and SnapshotRecord), update load/replay to restore it, and adjust tests/decoders.
3. **Implement hot/warm/cold resolution**
    
    - Hot: serve from live Kernel state if it satisfies requested consistency (head or exact/at least within current height).
    - Warm: when caller requests a height behind/ahead of the in-memory view, load the latest snapshot (with ref) plus replay a bounded tail into a throwaway read-only kernel (suppress_journal=true, mem journal) to materialize the requested height.
    - Cold: if requested Exact height matches an older snapshot or the tail is missing, load that historical snapshot directly (no replay) and serve from it.
    - Return StateRead with the resolved height + hashes; include None if reducer/key absent; error on stale Exact/AtLeast that can’t be satisfied.
4. **Wire into host surfaces**
    
    - Add a WorldHost::state_reader() (or methods) delegating to kernel StateReader.
    - Replace ControlMsg::QueryState/CLI world state to take a consistency parameter and return {state_b64, meta}.
    - Add a minimal HTTP query endpoint (opt-in adapter feature flag) that maps to StateReader, returns JSON with metadata, and is off by default.
5. **Capability/policy gating for reads**
    
    - Introduce a lightweight “introspect” capability class and enforce it in the HTTP/control handlers (e.g., require a configured capability name/policy rule before serving). Align policy evaluation with existing PolicyGate where feasible.
6. **Tests + docs**
    
    - Unit tests for StateReader resolution (hot vs. replay) including keyed reducers, missing cells, and Exact/AtLeast failure cases; update snapshot serialization tests for new fields.
    - Integration tests through the control/HTTP surfaces validating metadata and consistency semantics.
    - Document the new API in spec/02-architecture.md/spec/05-workflows.md as needed and add CLI help notes.

Natural next step: start by adding the kernel types/metadata (steps 1–2), then build the read path/resolution logic (step 3) before touching host/HTTP surfaces.