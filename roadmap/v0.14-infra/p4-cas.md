# P4: FoundationDB-Native CAS

**Priority**: P4  
**Effort**: High  
**Risk if deferred**: High (hosted worlds keep a narrow correctness-only CAS story and lack a coherent FDB-native path for larger immutable bodies)  
**Status**: Complete (Phases 1-3 implemented; partial-upload GC deferred until real CAS GC work)

## Goal

Ship a standalone, universe-scoped content-addressable storage (CAS) layer in `crates/aos-fdb` that stores immutable blobs directly in FoundationDB.

This item reframes the earlier "blob storage" discussion as a first-class CAS substrate:

1. CAS is the primary abstraction.
2. The implementation should be usable standalone, not only through `WorldPersistence`.
3. Existing hosted persistence code may later delegate `cas_put_verified` / `cas_get` to this module.
4. Alternative body backends such as S3-compatible object storage are deferred to a later phase behind the same CAS boundary.

This milestone is about correctness, layout, publish semantics, and operational shape for an FDB-native CAS. Worker-local caching remains a separate follow-on concern.

## Current Status Audit

The standalone CAS, the Phase 2 collapse onto a single hosted storage path, and the Phase 3 in-memory cache wrapper have all landed in `crates/aos-fdb`.

Implemented now:

- [x] Standalone `CasStore` trait with direct FDB-backed and in-memory implementations
- [x] Canonical `CasRootRecord`, `CasUploadMarker`, and `CasLayoutKind` types in `protocol.rs`
- [x] Direct `u/<u>/cas/meta|chunk|present|upload/...` key layout
- [x] Chunked writes with bounded transactions, presence pages, and final root publication
- [x] Full streaming reads and range reads over chunked CAS bodies
- [x] `WorldPersistence::{cas_put_verified, cas_get, cas_has}` reduced to thin adapters over the CAS module
- [x] Focused tests for round-trip behavior, range reads, size/hash validation, zero-byte blobs, reopen behavior, corruption detection, and universe isolation

Completed Phase 1 close-out:

- [x] Add an explicit concurrent same-hash writer test instead of relying only on the idempotence argument
- [x] Add explicit coverage for "not visible before publish" behavior
- [x] Add explicit coverage for retry / uncertain-commit behavior at the CAS publish boundary
- [x] Converge the FDB CAS implementation onto shared `keyspace.rs` helpers instead of duplicating key construction inside `cas.rs`

Deferred until real CAS GC work starts:

- Deferred: implement incomplete-upload GC that sweeps stale `cas/upload/*` markers and clears abandoned `chunk/*` and `present/*` prefixes once real CAS GC work starts

Completed in Phase 2:

- [x] Remove object-store-backed CAS support from `aos-fdb`
- [x] Remove blob-store env/config handling from `aos-fdb-worker`
- [x] Remove `CasMeta` / `BlobStorage` and collapse CAS onto canonical root records
- [x] Remove `CasConfig.inline_threshold_bytes`
- [x] Move segment bodies onto CAS by hash instead of external object keys

Conclusion:

- Phase 1 implementation work is complete.
- Phase 2 is implemented: hosted immutable bodies now go through CAS only, and cold segments are CAS-backed.
- Phase 3 is implemented with a process-local in-memory `CachingCasStore<T>`.
- The only deferred follow-up on this roadmap is incomplete-upload cleanup once real CAS GC work begins.

## Relationship To Existing Design

This spec must stay aligned with the key design and runtime/storage boundary already established in `v0.20-infra/p2-hosted-persistence-plane.md` and `crates/aos-fdb`:

- universe-scoped keys remain rooted under `u/<universe>/...`
- tuple/subspace helpers remain the authoritative encoding boundary
- hashes remain SHA-256
- CAS remains immutable and idempotent
- typed `PersistError` / `PersistConflict` / `PersistCorruption` remain the error vocabulary

The current implementation stores small CAS values inline in FDB metadata and larger values behind an object-store key. This milestone defines the replacement design for an FDB-native large-object CAS while preserving the higher-level invariants already relied upon by hosted restore, snapshot loading, and journal replay.

## Design Stance

- Keep the first implementation in `crates/aos-fdb`.
- A single `cas.rs` module is acceptable if it stays readable; a `cas/` submodule is acceptable if the implementation becomes materially larger.
- Expose a CAS API that is usable directly and not entangled with world journal/inbox/snapshot logic, but do not introduce a second wrapper type layer just to make it feel "standalone."
- It is acceptable, and recommended, to add a matching in-memory CAS implementation for unit tests and conformance-style coverage.
- Use FoundationDB as the only authoritative persistence substrate for this milestone.
- Do not require a single transaction for a full blob upload.
- Do not require locks for correctness.
- Treat all upload coordination markers as advisory only.

Type and helper placement:

- Stable CAS record types should live in `protocol.rs` from the beginning.
- Stable CAS key constructors should live in `keyspace.rs` from the beginning.
- `cas.rs` should own algorithms and implementation behavior, not a parallel copy of protocol/keyspace types.
- The same principle applies to an optional in-memory CAS: reuse the same protocol types rather than wrapping or redefining them.

## Core Requirements

The CAS must:

1. Store blobs up to 1 GiB.
2. Address blobs by `SHA-256(blob_bytes)`.
3. Permit multiple writers concurrently uploading the same hash.
4. Avoid single large transactions.
5. Support streaming reads.
6. Provide atomic publish semantics.
7. Support efficient cleanup of incomplete uploads.
8. Support multiple isolated universes.
9. Remain idempotent under retries and `commit_unknown_result`.

Non-goals for this milestone:

- cross-blob chunk deduplication
- transparent compression
- erasure coding
- tiered hot/cold storage
- striped/hot-object layouts
- one-pass direct upload from an unknown stream when the final hash is not yet known

## Important Constraint: Direct Layout Requires Hash-Known Writes

The current hosted protocol surface is `cas_put_verified(universe, bytes)`, which computes the hash before persisting content. That maps cleanly onto a direct deterministic chunk layout.

However, a direct key layout under the final hash prefix cannot support a truly one-pass upload from an opaque stream unless the caller already knows the expected hash up front.

Accordingly, this milestone defines:

- v1 required write paths:
  - `put_verified(universe, bytes)`
  - `put_reader_known_hash(universe, expected_hash, size_bytes, reader)` or equivalent
- v1 required read paths:
  - full read to bytes
  - full read as a stream
  - chunk/range read as a stream
- optional follow-on write path:
  - staged upload for unknown-hash streaming sources

If a future caller wants "stream from unknown source and never buffer locally", that requires an explicit staging protocol and is out of scope for this milestone.

## FoundationDB Limits That Shape The Design

This CAS must respect FoundationDB operational guidance:

- max value size: 100 kB
- max transaction size: 10 MB
- recommended transaction size: comfortably below 1 MB
- transaction duration: must stay well below 5 s

Design implications:

- blobs must be chunked
- uploads must be split across many transactions
- reads must be paged across multiple transactions

## Scope (Now)

### 1) Standalone CAS boundary in `aos-fdb`

Add a standalone CAS module with an FDB-backed implementation and, ideally, an in-memory twin.

Suggested API shape:

```rust
pub trait CasStore {
    fn put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError>;
    fn put_reader_known_hash(
        &self,
        universe: UniverseId,
        expected: Hash,
        size_bytes: u64,
        reader: &mut dyn std::io::Read,
    ) -> Result<Hash, PersistError>;

    fn stat(&self, universe: UniverseId, hash: Hash) -> Result<CasRootRecord, PersistError>;
    fn get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError>;
    fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError>;

    fn read_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        writer: &mut dyn std::io::Write,
    ) -> Result<CasRootRecord, PersistError>;

    fn read_range_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        offset: u64,
        len: u64,
        writer: &mut dyn std::io::Write,
    ) -> Result<(), PersistError>;
}
```

Notes:

- Exact API names may vary.
- `put_reader_known_hash` is illustrative; any equivalent "hash-known streaming upload" API is acceptable.
- `CasRootRecord` should be the canonical metadata type from `protocol.rs`, not a CAS-only wrapper copy.
- `put_verified` and `get` are convenience APIs for one-shot callers.
- `put_reader_known_hash`, `read_to_writer`, and `read_range_to_writer` are the primary streaming APIs.
- `put_verified` may internally delegate to the streaming upload path over an in-memory reader.
- `get` may internally delegate to the streaming read path into a `Vec<u8>`.
- Existing `WorldPersistence::{cas_put_verified, cas_get, cas_has}` should become thin adapters over this module rather than continuing to own the storage logic directly.
- "Standalone" here means callable without dragging in world-specific logic, not a requirement to duplicate stable types outside the existing protocol/keyspace boundary.

### 2) Canonical universe-scoped keyspace

Stay aligned with the existing `u/<u>/...` keyspace style used in `aos-fdb`.

Logical layout:

- `u/<u>/cas/meta/<hash> -> CasRootRecord`
- `u/<u>/cas/chunk/<hash>/<index> -> raw chunk bytes`
- `u/<u>/cas/present/<hash>/<page> -> presence bitmap page`
- `u/<u>/cas/upload/<hash> -> CasUploadMarker` (advisory)

Tuple examples:

- `("u", universe, "cas", "meta", hash)`
- `("u", universe, "cas", "chunk", hash, index)`
- `("u", universe, "cas", "present", hash, page)`
- `("u", universe, "cas", "upload", hash)`

Encoding notes:

- The exact tuple element representation for `hash` remains an implementation detail behind keyspace helpers.
- For continuity with the current codebase, it is acceptable for the first implementation to continue using the existing `Hash` helper and current tuple-packing conventions rather than forcing a raw-binary key migration immediately.
- The authoritative design requirement is the logical layout and immutability semantics, not a specific hash tuple representation.

### 3) Root metadata record

Authoritative publication record:

```text
u/<u>/cas/meta/<hash>
```

Suggested fields:

```text
CasRootRecord {
    version: u8,
    chunk_size: u32,
    chunk_count: u32,
    size_bytes: u64,
    layout_kind: u8,
}
```

Layout kinds:

- `0 = direct`
- `1 = staged` (reserved, not required in this milestone)
- `2 = striped` (reserved, future)

Rules:

- root record is immutable once published
- root record is the only authority that makes a blob visible
- chunks and presence pages without a root record do not make a blob readable

### 4) Chunk layout

Chunks are fixed-size by default:

- `chunk_size = 64 KiB`

Storage:

```text
u/<u>/cas/chunk/<hash>/<index>
```

Properties:

- chunks are stored as raw bytes
- final chunk may be shorter than `chunk_size`
- chunk indices are ordered and contiguous from `0..chunk_count-1`
- chunk index encoding must preserve lexicographic order for range scans

### 5) Presence bitmap

Presence pages record upload completeness without requiring a scan of every chunk key.

Storage:

```text
u/<u>/cas/present/<hash>/<page>
```

Bitmap shape:

- one page covers 1024 chunks
- one page stores 1024 bits = 128 bytes
- `page = chunk_index / 1024`
- `bit = chunk_index % 1024`

For a 1 GiB blob at 64 KiB chunking:

- `chunk_count ~= 16384`
- `presence_pages = 16`

Rules:

- presence pages are advisory until root publication
- final publish must verify completeness against the expected chunk count

### 6) Advisory upload marker

Store a lightweight upload marker under:

```text
u/<u>/cas/upload/<hash>
```

Suggested fields:

```text
CasUploadMarker {
    version: u8,
    writer_id?: text,
    started_at_ns: u64,
    last_touched_at_ns?: u64,
}
```

Purpose:

- reduce redundant uploads in the future
- give incomplete-upload GC something to age against

Rules:

- advisory only
- never required for correctness
- readers must ignore it
- writers must not rely on it for exclusivity
- stale markers are harmless

The earlier "lease" idea is acceptable in spirit, but `upload` is preferred here to avoid confusion with the hosted world lease protocol already defined in P3.

## Write Algorithm

### 1) Canonical chunking

All writers must use the same chunking policy for direct layout:

- SHA-256 over the full blob bytes
- `chunk_size = 64 KiB`
- chunk indices start at 0
- chunk count is exact

If two writers claim the same hash, they must be writing the same bytes and therefore the same chunk contents to the same keys.

### 2) Existence fast path

If the hash is already known before upload:

1. read `u/<u>/cas/meta/<hash>`
2. if present, return success immediately

For `put_verified(bytes)`, hash is computed from the supplied bytes before any writes.

For `put_reader_known_hash(...)`:

- the caller must provide the expected final hash up front
- the caller must provide the exact `size_bytes` up front
- the implementation must fail if the streamed byte count does not match `size_bytes`

### 3) Chunk write transactions

Write chunks in batches. Recommended initial batch size:

- `8` chunks per transaction

Each transaction should:

- `SET chunk/<hash>/<index> = bytes`
- update the corresponding presence page bits for those chunks

Implementation notes:

- if FoundationDB atomic bitwise OR is used for bitmap pages, that is preferred
- if the implementation instead performs read-modify-write on a 128-byte page inside the same transaction, that is also acceptable
- each chunk batch transaction should remain comfortably below normal FDB transaction-size guidance
- transactions should be write-only where practical

### 4) Publish transaction

After all chunk batches commit, publish visibility with a small final transaction:

1. read `meta/<hash>`
2. if already present, return success
3. read all expected presence pages
4. verify all required bits for `chunk_count` are set
5. write immutable `meta/<hash>`
6. optionally clear `upload/<hash>`

This publish transaction is the linearization point for blob visibility.

### 5) Why concurrent writers do not corrupt data

The correctness argument is:

1. chunk keys are deterministic under `(universe, hash, index)`
2. same hash implies same full bytes
3. same full bytes imply same chunk bytes
4. duplicate `SET` of identical chunk values is benign
5. duplicate presence-page updates converge
6. root publication happens last and is immutable

Therefore, concurrent same-hash writers are safe without locks.

The only writer-visible race is who wins the final publish. That is acceptable because:

- if a writer publishes first, later writers observe `meta` and return success
- if publish result is uncertain, the writer re-reads `meta`

## Read Algorithm

### 1) Full read

1. read `meta/<hash>`
2. derive `chunk_count` and `chunk_size`
3. range-read `chunk/<hash>/<index>` in pages across multiple transactions
4. concatenate bytes
5. verify total size matches `size_bytes`
6. for full reads, verify `SHA-256(bytes) == hash`

Rules:

- full reads should verify the hash by default
- missing chunk data for a published root is corruption
- extra chunk data beyond `chunk_count` is ignored by the canonical read path but should be considered cleanup debt
- `read_to_writer` is the primary full-read path; `get` is the convenience helper that materializes the entire blob in memory

### 2) Range read

For `(offset, len)`:

- `start_chunk = offset / chunk_size`
- `end_chunk = (offset + len - 1) / chunk_size`

Read only the required chunk range, then trim the prefix/suffix bytes in memory.

Rules:

- range reads do not need to hash-verify the entire blob
- range reads must still fail closed on missing published chunks
- `read_range_to_writer` is the primary range-read path

### 3) Streaming

The implementation should read chunks in bounded pages rather than loading a 1 GiB object in a single transaction.

Recommended initial read page:

- `128` chunks per transaction

The exact page size is operational and may be tuned later.

## Failure Semantics

### Incomplete upload

If a writer crashes before publish:

- some `chunk/*` and `present/*` keys may exist
- `meta/<hash>` does not exist
- the blob is considered nonexistent

### `commit_unknown_result`

All write paths must be idempotent:

- retrying a chunk batch is safe
- retrying the publish transaction is safe
- after any uncertain publish, read `meta/<hash>`
- if `meta` exists, treat the upload as successful

### Published corruption

If `meta/<hash>` exists but:

- a required chunk is missing
- full read hash mismatches
- stored size is inconsistent with reconstructed bytes

the implementation must return a corruption error, not a soft miss.

## Garbage Collection

Two distinct cleanup modes are required.

### 1) Incomplete-upload GC

Scan:

- `u/<u>/cas/upload/*`

For markers older than a configured threshold:

1. if `meta/<hash>` exists, remove stale upload marker only
2. if `meta/<hash>` is absent, clear:
   - `chunk/<hash>/*`
   - `present/<hash>/*`
   - `upload/<hash>`

Rationale:

- age-based cleanup is only practical if we persist an advisory upload marker
- scanning all chunk prefixes without such a marker is not operationally attractive

### 2) Reachability GC

A higher layer determines which CAS hashes are still reachable from manifests, snapshots, journal refs, and other typed roots.

When a blob is unreachable, delete:

- `meta/<hash>`
- `chunk/<hash>/*`
- `present/<hash>/*`
- `upload/<hash>` if present

Because CAS content is immutable, deletion is prefix-oriented and does not require in-place mutation semantics.

## Performance Shape

At `1 GiB` with `64 KiB` chunks:

- `chunk_count ~= 16384`
- `presence_pages = 16`
- with `8` chunks/transaction:
  - about `2048` write transactions

This is acceptable for cold immutable blobs, snapshots, and other non-hot CAS material, but it is not free. Operators should expect:

- write amplification from many transactions
- storage amplification from FoundationDB replication
- meaningful benefit from later worker-local read caching

This CAS is appropriate as an early-system simplification and control-plane-friendly substrate. It is not a claim that FoundationDB is the final ideal large-object store for all future scales.

## Implementation Notes

Recommended first implementation shape:

- `crates/aos-fdb/src/cas.rs` or `crates/aos-fdb/src/cas/mod.rs`
- `FdbCasStore`
- optional `MemoryCasStore`
- CAS key helpers added directly to `keyspace.rs`
- typed metadata structs added directly to `protocol.rs` and CBOR-encoded with existing crate conventions
- streaming helpers built on ordinary `std::io::Read` / `std::io::Write` rather than introducing custom wrapper protocols unless implementation pressure proves that necessary

Suggested initial test matrix:

1. put/get round-trip for small and large values
2. duplicate put is idempotent
3. two concurrent writers of the same hash both succeed
4. incomplete upload is not visible before publish
5. published root with missing chunk fails as corruption
6. `commit_unknown_result` recovery via `meta` readback
7. range read returns exact slice
8. incomplete-upload GC removes abandoned prefixes
9. memory and FDB implementations obey the same high-level contract

## Implementation Phases

### Phase 1: Introduce FDB-native CAS

Deliver the standalone CAS described above:

- [x] chunked CAS bodies stored directly in FDB
- [x] immutable root publication records
- [x] presence pages and upload markers
- [x] `WorldPersistence` CAS methods delegated to the new module
- [x] optional in-memory CAS counterpart for tests
- [x] CAS key construction routed through shared `keyspace.rs` helpers
- [x] explicit concurrency / publish-boundary regression coverage

Deferred follow-up:

- Deferred: incomplete-upload GC for stale advisory upload markers

This phase can coexist briefly with the current object-store-backed implementation while the refactor lands, but that coexistence is transitional only.

### Phase 2: Collapse onto a single store and remove object-store support

**Status**: Complete

Once the FDB-native CAS exists, remove the old "FDB metadata + external body store" model entirely.

Required outcomes:

1. Delete S3 support and its configuration surface from `aos-fdb` and `aos-fdb-worker`.
2. Delete the filesystem/object-store backend abstraction used for hosted CAS bodies.
3. Remove CAS-internal `Inline | ObjectStore` storage branching.
4. Make CAS the only immutable body store in hosted FDB mode.
5. Move cold segment bodies onto CAS rather than storing them beside CAS.

Concretely, Phase 2 should:

- [x] remove `object_store.rs`
- [x] remove `BlobObjectStoreConfig`, `FilesystemObjectStoreConfig`, and `S3BlobObjectStoreConfig`
- [x] remove the `object_store` crate dependency and related S3/config libraries
- [x] remove blob-store env/config handling from `aos-fdb-worker`
- [x] replace the current `CasMeta { size, storage, object_key?, inline_bytes? }` shape with the canonical CAS root record defined in this document
- [x] remove `CasConfig.inline_threshold_bytes`
- [x] keep caller-level inline-vs-ref behavior for queue payloads and dispatch params where useful
- [x] change segment metadata to point at CAS content by hash rather than external `object_key`

Clarification:

- caller-level inline payload optimization is not the same thing as CAS-internal storage branching
- it is still reasonable for inbox/effect payloads to stay inline below a small threshold
- once bytes are stored in CAS, they should use one canonical CAS layout only

Net result:

- manifests, snapshots, large payloads, and cold segment bodies all share one immutable store
- FoundationDB is the only storage substrate in hosted mode for this phase
- the hosted FDB stack becomes materially smaller and easier to reason about

### Phase 3: Add worker-local CAS caching

**Status**: Complete

After the FDB-native CAS is the only immutable body store, add a worker-local cache for CAS reads and recent writes.

Goals:

1. Avoid immediate re-read cost for blobs the same worker just wrote.
2. Reduce repeated FDB chunk scans for hot immutable CAS objects.
3. Keep caching strictly as a performance layer; correctness must never depend on cache residency.

Recommended first implementation:

- [x] process-local in-memory cache only
- [x] cache key is `(universe_id, hash)`
- [x] cache value is the full reconstructed blob bytes plus small metadata such as `size_bytes`
- [x] size-bounded LRU or segmented-LRU eviction
- [x] per-item cache admission cap to keep oversized blobs from polluting the cache
- [x] read-through population on successful `get` / `read_to_writer`
- [x] write-through population on successful `put_verified` / `put_reader_known_hash`

Why this is the right first step:

- it captures the common "write then read soon after" path immediately
- it avoids a second persistence substrate while the hosted FDB stack is still early
- it keeps failure modes simple because cache loss is just a miss
- it avoids mixing operational local-disk concerns into the first CAS bring-up

Rules:

- cache entries are advisory only
- cache misses fall through to authoritative FDB CAS
- cache contents must never be treated as more authoritative than published CAS metadata
- only published CAS objects may be cached
- incomplete uploads must never populate the cache as visible objects

Recommended API/placement shape:

- keep the cache close to the CAS implementation boundary, not in kernel logic
- prefer a small `CachingCasStore<T: CasStore>` wrapper rather than baking cache state directly into `FdbCasStore`
- `FdbCasStore` should remain the authoritative implementation
- the caching wrapper should remain a pure performance layer and should not duplicate protocol/keyspace types

Recommended module shape:

- `protocol.rs`: canonical CAS record types
- `keyspace.rs`: canonical CAS key constructors
- `cas.rs` or `cas/mod.rs`: `CasStore` trait and `FdbCasStore`
- `cas/cache.rs` or similar: `CachingCasStore<T: CasStore>`

Why the wrapper is preferred:

- it keeps the authoritative FDB CAS implementation smaller and easier to reason about
- it makes cache-on vs cache-off behavior explicit in tests and bring-up
- it allows the same caching policy to wrap other CAS implementations such as an in-memory CAS
- it avoids leaking cache policy into world runtime code

Read behavior:

1. look up `(universe, hash)` in cache
2. on hit, serve from cached bytes
3. on miss, read from authoritative FDB CAS and populate cache after successful verification

Write behavior:

1. upload and publish through authoritative FDB CAS
2. only after successful publish, insert the blob into the local cache
3. if publish result is uncertain, confirm via `stat`/`has` before treating the blob as cacheable

Range-read behavior:

- simplest first implementation may satisfy range reads from a cached full blob when available
- on miss, stream the requested range from FDB CAS without requiring whole-blob cache population
- later optimization may add chunk-aware caching if profiling justifies the complexity

Deferred follow-on:

- local disk-backed cache
- shared node-level cache across worker restarts
- chunk-granular cache entries
- cache warming/prefetch based on restore paths
- operator-tunable persistence/eviction policy

Recommendation:

- implement only in-memory caching in Phase 3
- do not add local-disk caching yet

Rationale:

- a disk cache would reintroduce a second local storage subsystem just after removing the external object-store stack
- the main value for now is short-horizon locality on recent writes and hot reads within a live worker process
- in-memory caching is enough to validate access patterns before committing to a disk format and invalidation policy

## Summary

This milestone defines an FDB-native standalone CAS with:

- immutable hash-addressed blobs
- deterministic chunk layout
- small atomic publish step
- safe concurrent same-hash writers
- paged reads across transactions
- advisory upload markers for cleanup
- universe-scoped key isolation

The key design decision is to keep visibility anchored on a small immutable root record under the existing `u/<u>/cas/...` key hierarchy. Everything else is preparatory state. That preserves correctness under retries, concurrent writers, and partial failure, and it gives the rest of AgentOS a clean standalone CAS to build on.
