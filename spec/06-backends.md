# Backends

This document describes the persistence backends supported by the current unified node. It covers
the logical contracts, the supported backend combinations, how the node chooses and manages those
backends, and the recovery rules that keep replay deterministic.

The important split is:

- the **journal backend** stores ordered world frames,
- the **CAS/blob backend** stores content-addressed bytes,
- the **checkpoint/meta backend** stores restart hints such as latest checkpoint refs and command
  records.

The kernel does not know whether these are SQLite rows, Kafka records, local files, or object-store
objects. It sees canonical journal records and `hash -> bytes` CAS reads.

## 1) Scope

This spec covers:

- SQLite and Kafka journal backends,
- local filesystem CAS and object-store-backed CAS,
- object-store checkpoint metadata and command metadata,
- node configuration and supported backend combinations,
- checkpoint cursors, recovery, and world discovery.

It does not define:

- AIR forms or manifest validation; see [spec/03-air.md](03-air.md),
- workflow/effect semantics; see [spec/04-workflows.md](04-workflows.md) and
  [spec/05-effects.md](05-effects.md),
- future deletion GC; see [spec/20-gc.md](20-gc.md),
- provider-specific S3 bucket lifecycle, IAM, replication, or Kafka cluster operations.

## 2) Backend Contracts

`aos-node/src/model/backends.rs` defines the runtime seams.

### 2.1 JournalBackend

The journal backend stores and retrieves `WorldLogFrame` values. A frame contains:

- `format_version`,
- `universe_id`,
- `world_id`,
- `world_epoch`,
- `world_seq_start`,
- `world_seq_end`,
- canonical kernel `records`.

The backend must provide:

- `refresh_all()` and `refresh_world(world_id)` to make externally durable records visible,
- `world_ids()` for discovery,
- `durable_head(world_id)` returning the next durable world sequence,
- `world_frames(world_id)` for full replay,
- `world_tail_frames(world_id, after_world_seq, cursor)` for checkpoint-based replay,
- `commit_flush(JournalFlush)` for atomic publication of staged frames and durable dispositions.

`commit_flush` returns `JournalCommit { world_cursors }`. A cursor is an optimization used by
future checkpoint opens:

- SQLite cursor: `WorldJournalCursor::Sqlite { frame_offset }`
- Kafka cursor: `WorldJournalCursor::Kafka { journal_topic, partition, journal_offset }`

The cursor is not the source of truth. Tail reads also filter by
`world_seq_end > after_world_seq`, and the node can fall back to full `world_frames` replay when no
checkpoint cursor is available.

### 2.2 BlobBackend and Store

The blob/CAS contract is logical `hash -> bytes`. The hash is SHA-256 over the logical bytes. All
reads must verify the returned bytes against the requested hash.

`HostedCas` is the node's CAS implementation. It always has:

- a local `FsCas` cache under the universe state root,
- a remote `RemoteCasStore`.

When the selected blob backend is local, the remote store is an embedded in-process implementation
and the durable copy is the filesystem CAS. When the selected blob backend is object-store-backed,
the remote store is S3-compatible object storage and the filesystem CAS is a local cache.

The same `HostedCas` implements the kernel `Store` trait for canonical CBOR nodes and opaque blobs.
It is used for AIR nodes, manifests, WASM modules, snapshots, workspace tree/file blobs, and user
blob-effect payloads through the same hash-addressed interface.

### 2.3 CheckpointBackend and WorldInventoryBackend

Checkpoint metadata is separate from snapshot bytes. Snapshot bytes live in CAS and snapshot
records live in the journal. Checkpoint metadata stores the latest known baseline and backend cursor
for faster open:

- `commit_world_checkpoint(record)`,
- `latest_world_checkpoint(world_id)`,
- `list_world_checkpoints()`,
- `list_worlds()`.

In object-store mode this metadata is durable. In local blob mode the checkpoint metadata backend is
embedded in-process; after restart, the node can still discover worlds from the journal and reopen
from the latest snapshot record in journal frames.

## 3) Supported Combinations

The CLI exposes two independent-looking knobs:

```bash
aos node up --journal-backend sqlite --blob-backend local
aos node up --journal-backend sqlite --blob-backend object-store
aos node up --journal-backend kafka --blob-backend object-store
```

The supported combinations are:

| Journal | Blob/CAS | Status | Notes |
| --- | --- | --- | --- |
| SQLite | Local FS CAS | Default | Repo-local `.aos-node` state; simplest development mode. |
| SQLite | Object-store CAS | Supported | Local journal with shared/durable CAS and checkpoint metadata. |
| Kafka | Object-store CAS | Supported | Broker journal plus object-store CAS/meta for server deployments. |
| Kafka | Local FS CAS | Rejected by node binary | Kafka journal mode requires object-store CAS/meta. |

There is also an embedded Kafka implementation used by tests and harnesses. It is not exposed as a
node binary mode; `aos node up --journal-backend kafka` requires broker configuration.

## 4) SQLite Journal Backend

SQLite is the default journal backend.

Physical layout:

- database path: `<state_root>/journal.sqlite3`,
- WAL mode enabled,
- `synchronous = FULL`,
- schema version tracked in `journal_meta`.

Tables:

- `journal_frames`: one CBOR-encoded `WorldLogFrame` per row,
- `journal_world_heads`: `world_id -> next_world_seq` plus latest frame offset,
- `journal_dispositions`: durable rejected-submission or command-failure records,
- `journal_meta`: singleton schema version.

Commit behavior:

1. The worker stages completed slices as a `JournalFlush`.
2. SQLite opens one transaction for the flush.
3. For each frame, it checks the durable per-world head and rejects non-contiguous sequence starts.
4. It inserts the frame bytes and updates `journal_world_heads`.
5. It inserts durable dispositions.
6. It commits the transaction and returns SQLite frame-offset cursors for touched worlds.

`refresh_all()` and `refresh_world()` are no-ops for SQLite because the connection reads its own
database directly. Full replay loads `journal_frames` ordered by `world_seq_start`. Tail replay
uses either the stored SQLite `frame_offset` cursor or the world sequence filter.

## 5) Kafka Journal Backend

Kafka is an explicit journal backend, not the default architecture. Direct HTTP/control acceptance
still remains the default ingress path; Kafka here is the durable journal backend.

Physical/logical layout:

- topic: `AOS_KAFKA_JOURNAL_TOPIC` or `aos-journal`,
- partitions: `--partition-count` / `AOS_PARTITION_COUNT`,
- partitioning: `partition_for_world(world_id, partition_count)`,
- record key for frames: world id bytes,
- record value: canonical CBOR `HostedJournalRecord::Frame(WorldLogFrame)`,
- durable disposition values: canonical CBOR `HostedJournalRecord::Disposition(...)`.

The broker implementation keeps an in-memory index:

- `world_frames`: replay frames grouped by world,
- `partition_logs`: recovered records grouped by `(topic, partition)`,
- `recovered_journal_offsets`: last visible broker offset per partition.

Recovery consumes the configured journal topic with `read_committed` isolation and rebuilds those
indexes. `refresh_all()` recovers all topic partitions. `refresh_world(world_id)` recovers only the
partition assigned to that world.

Commit behavior:

1. The worker stages completed slices as a `JournalFlush`.
2. The Kafka backend wraps frames and dispositions in `HostedJournalRecord`.
3. The broker backend publishes the flush inside a Kafka transaction.
4. After transaction commit and delivery reports, it appends the delivered frames to its in-memory
   indexes with broker offsets.
5. It returns Kafka cursors for touched worlds using the latest visible partition offset.

Kafka cursors are valid only for the configured topic and partition count. If a cursor is absent or
does not match the active backend, replay falls back to the world sequence filter.

## 6) Filesystem CAS

`FsCas` is the local filesystem content-addressed store.

Physical layout:

```text
<universe_state_root>/cas/<first-two-digest-hex>/<remaining-digest-hex>
```

Write behavior:

1. Hash the logical bytes.
2. If the blob already exists, return the hash.
3. Write a temporary file in the shard directory.
4. `sync_all` the file.
5. Rename into place.

Read behavior:

1. Read bytes from the digest path.
2. Hash the bytes.
3. Reject the read if the actual hash differs from the requested hash.

`FsCas` is used as the durable CAS in local blob mode and as the local hydration cache in
object-store mode.

## 7) Object-Store CAS

The object-store backend uses the Rust `object_store` S3-compatible client. It is selected when a
non-empty blobstore bucket is configured.

The node scopes all object-store keys by universe:

```text
<prefix>/universes/<universe_id>/...
```

Blob storage uses two physical layouts behind the same logical `hash -> bytes` contract.

Direct blobs:

```text
<prefix>/universes/<universe_id>/blobs/<sha256:...>
```

Packed blobs:

```text
<prefix>/universes/<universe_id>/packs/<pack_hash>.bin
<prefix>/universes/<universe_id>/cas/<logical_hash>.cbor
```

Small blobs at or below `pack_threshold_bytes` are written into immutable pack objects. The
`cas/<logical_hash>.cbor` root record stores:

- logical hash,
- size,
- direct object key or packed object key,
- packed byte range when applicable.

Large blobs are written directly and also get a CAS root record. Reads load the root record when
present, fetch either the direct object or the packed byte range, and verify the logical hash. If no
root record exists, the backend falls back to the direct blob key for compatibility.

## 8) Checkpoint and Command Metadata

Object-store-backed metadata lives under the universe-scoped prefix.

Latest checkpoint:

```text
<prefix>/universes/<universe_id>/checkpoints/worlds/<world_id>/latest.cbor
```

Checkpoint history:

```text
<prefix>/universes/<universe_id>/checkpoints/worlds/<world_id>/manifests/<checkpointed_at_ns>.cbor
```

Command records:

```text
<prefix>/universes/<universe_id>/commands/<world_id>/<command_id>.cbor
```

The latest checkpoint record includes:

- `universe_id`,
- `world_id`,
- `world_epoch`,
- checkpoint timestamp,
- promotable baseline snapshot ref and manifest hash,
- checkpointed world sequence,
- optional journal cursor.

Checkpoint history retention keeps only the newest
`retained_checkpoints_per_partition` history records per world. This retention only prunes old
checkpoint metadata snapshots. It does not delete CAS blobs or journal records; deletion GC is a
separate future protocol.

## 9) Node Configuration

`aos node up` starts the hidden `node-serve` entrypoint with the selected backend options. The
runtime config is represented by `NodeConfig`:

- `role`: worker, control, or both,
- `state_root`: local node state root,
- `default_universe_id`,
- `journal`: `NodeJournalBackend::Sqlite` or `NodeJournalBackend::Kafka`,
- `worker`: worker/scheduler config,
- `control`: HTTP bind config.

CLI/env selection:

- `--journal-backend sqlite|kafka` / `AOS_JOURNAL_BACKEND`,
- `--blob-backend local|object-store` / `AOS_BLOB_BACKEND`,
- `--partition-count` / `AOS_PARTITION_COUNT`,
- `--state-root` / `AOS_STATE_ROOT` for `node-serve`,
- `--default-universe-id` / `AOS_DEFAULT_UNIVERSE_ID`.

Kafka env:

- `AOS_KAFKA_BOOTSTRAP_SERVERS` (required for node binary Kafka mode),
- `AOS_KAFKA_JOURNAL_TOPIC`,
- `AOS_KAFKA_TRANSACTIONAL_ID`,
- `AOS_KAFKA_PRODUCER_MESSAGE_TIMEOUT_MS`,
- `AOS_KAFKA_PRODUCER_FLUSH_TIMEOUT_MS`,
- `AOS_KAFKA_TRANSACTION_TIMEOUT_MS`,
- `AOS_KAFKA_METADATA_TIMEOUT_MS`,
- `AOS_KAFKA_RECOVERY_FETCH_WAIT_MS`,
- `AOS_KAFKA_RECOVERY_POLL_INTERVAL_MS`,
- `AOS_KAFKA_RECOVERY_IDLE_TIMEOUT_MS`.

Blobstore env:

- `AOS_BLOBSTORE_BUCKET` or legacy `AOS_S3_BUCKET`,
- `AOS_BLOBSTORE_ENDPOINT` or legacy `AOS_S3_ENDPOINT`,
- `AOS_BLOBSTORE_REGION` or legacy `AOS_S3_REGION`,
- `AOS_BLOBSTORE_PREFIX` or legacy `AOS_S3_PREFIX`,
- `AOS_BLOBSTORE_FORCE_PATH_STYLE` or legacy `AOS_S3_FORCE_PATH_STYLE`,
- `AOS_BLOBSTORE_PACK_THRESHOLD_BYTES` or legacy `AOS_S3_PACK_THRESHOLD_BYTES`,
- `AOS_BLOBSTORE_PACK_TARGET_BYTES` or legacy `AOS_S3_PACK_TARGET_BYTES`,
- `AOS_BLOBSTORE_RETAINED_CHECKPOINTS` or legacy `AOS_S3_RETAINED_CHECKPOINTS`.

Kafka journal mode requires object-store blob mode. The node binary rejects `kafka + local` and
rejects Kafka mode without `AOS_KAFKA_BOOTSTRAP_SERVERS`.

## 10) Control, Worker, and Backend Management

The unified node can run worker and control roles together. Backend management differs by journal
mode.

In non-broker mode, control services reuse the colocated worker runtime:

- journal reads are callbacks into the runtime's selected journal backend,
- CAS reads go through the runtime's per-universe `HostedCas`,
- checkpoint/meta reads go through the runtime's checkpoint backend,
- hot state reads come directly from active worlds.

In broker Kafka mode, control services also construct standalone services:

- `HostedJournalService` recovers journal frames from Kafka,
- `HostedCasService` opens per-universe CAS stores from local cache plus object store,
- `HostedMetaService` reads object-store checkpoint and command metadata,
- `HostedReplayService` can open kernels from checkpoint plus journal tail.

Submissions still go through the colocated runtime. Reads do not require projection/materializer
state; they are hot active-world reads or replay reads over the selected backend services.

## 11) World Discovery and Recovery

Startup recovery follows the backend contracts:

1. Refresh the journal source.
2. List worlds from checkpoint metadata.
3. Add worlds visible in the journal backend.
4. Register worlds by manifest hash from journal snapshots or checkpoint metadata.
5. Activate a world by opening from the latest checkpoint when available, otherwise from full
   journal frames.
6. Replay only frames after the chosen active baseline.
7. Rehydrate runtime work from pending workflow receipts after the world is active.

Checkpoint publication follows the durable flush fence:

1. The worker creates a kernel snapshot and stages a checkpoint slice.
2. Any snapshot journal records are committed through the selected journal backend.
3. Only after durable journal commit does the worker commit checkpoint metadata with the returned
   journal cursor.
4. The active kernel may compact its in-memory journal through the checkpoint height.

External async effects are also gated by this same durable append boundary. The worker starts
timers and external adapters only after the frame that opened the work has durably flushed.

## 12) Invariants

Backends must preserve these invariants:

1. Backend choice must not change canonical kernel records.
2. Per-world journal sequence numbers are contiguous and monotonic.
3. A committed frame is replayable by `(world_id, world_seq_start..world_seq_end)`.
4. Checkpoint cursors are restart accelerators, not authority.
5. CAS reads verify content hashes before returning bytes to the kernel.
6. Object-store packing is invisible above the CAS layer.
7. Async effect publication happens only after durable journal append.
8. No adapter or backend mutates world state directly; all state changes re-enter as journaled
   world input.
9. Journal partitioning is backend metadata and must not leak into public acceptance semantics.
10. Deletion of journal frames or CAS objects is not part of the active backend contract.
