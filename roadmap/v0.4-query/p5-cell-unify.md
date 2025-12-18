# p5: Unify keyed and non-keyed reducer state into CAS-backed cells

## Goal
- Treat every reducer state as a cell stored in CAS so both keyed and non-keyed reducers can be offloaded/evicted uniformly under memory pressure.

## Status
- ✅ Implemented in kernel/host: non-keyed reducers now use sentinel cell `MONO_KEY` via `CellIndex`/`CellCache`; monolithic fields removed; snapshots rely on index roots only; tests updated and passing with `--features test-fixtures`.

## Current behavior (pain)
- Keyed reducers: per-key state lives in CAS, indexed by `CellIndex`, cached in `CellCache` LRU.
- Non-keyed reducers: state kept in-memory (`ReducerState.monolithic`); we still `put_blob` but never retain the hash/root, so eviction/offloading is impossible and snapshots inline full bytes.

## Design: single-cell model for non-keyed reducers
- Represent non-keyed state as a single cell addressed by a sentinel key (recommend `MONO_KEY = b""` or a short constant) and keyed by `Hash::of_bytes(MONO_KEY)` in `CellIndex`.
- Give every reducer an index root (`reducer_index_roots` map) regardless of keyedness.
- Use the existing `CellIndex` + `CellCache` paths for all reducers; drop special monolithic fields.

## Implementation plan
1) **Data structures**
   - `ReducerState`: remove `monolithic`/`monolithic_hash`; keep only `cell_cache`.
   - `reducer_index_roots`: populate for all reducers at first write (create empty root lazily as today for keyed reducers).
   - Define `const MONO_KEY: &[u8] = b"";` (or similar) in `world.rs` to mark non-keyed cell.

2) **Write path (`handle_reducer_output`)**
   - For non-keyed reducers, wrap state as the sentinel cell:
     - compute `state_hash = store.put_blob(state)`.
     - `CellMeta { key_hash: Hash::of_bytes(MONO_KEY), key_bytes: MONO_KEY.to_vec(), state_hash, size, last_active_ns }`.
     - `index.upsert(root, meta)` and `cell_cache.insert(MONO_KEY, CellEntry { state, state_hash, last_active_ns })`.
   - Deletions (`state == None`) call `index.delete(root, hash(MONO_KEY))` and evict from cache.

3) **Read path**
   - `reducer_state_bytes` uses cache→CAS lookup via `CellIndex` for both keyed and non-keyed; map `key=None` to `MONO_KEY`.
   - `StateReader::get_reducer_state` benefits automatically (Head/AtLeast/Exact).

4) **Cache/offload semantics**
   - With monolithic field removed, eviction of `CellCache` naturally offloads non-keyed state to CAS; reload uses `CellIndex`.
   - `CELL_CACHE_SIZE` continues to bound in-memory cells across reducers.

5) **Snapshots**
   - Record only `reducer_index_roots` for all reducers; do not inline monolithic bytes.
   - Snapshot apply relies on `CellIndex` (no backward compat needed per request). ✅

6) **Helpers/validation**
   - Ensure router still rejects provided key_field for non-keyed reducers; keyed enforcement unchanged.
   - Unit tests: ✅
     - non-keyed state written → persisted in CAS, retrievable after cache eviction.
     - snapshot round-trip with non-keyed reducer using index root only.
     - cache eviction path keeps keyed and non-keyed working.

## Nice-to-haves (optional follow-ups)
- Configurable sentinel key constant exposed for diagnostics.
- CLI introspection: list-cells should return the sentinel cell for non-keyed reducers (size/last_active useful).

## Scope clarification
- No backward-compat migration needed; assume worlds will be replayed after this change.
