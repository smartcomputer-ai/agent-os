# P4: Switchable Journal Backends

**Priority**: P4  
**Effort**: Large  
**Risk if deferred**: Medium-High (the runtime now has a real backend seam, but Kafka still leaks
through the shared cursor/commit model and remains the only practical journal backend)  
**Status**: Done  
**Depends on**:
- `roadmap/v0.19-unify/directive.md`
- `roadmap/v0.19-unify/p2-direct-http-ingress-and-explicit-ownership.md`
- `roadmap/v0.19-unify/p3-world-based-discovery-and-checkpoints.md`
- `roadmap/v0.18-execution/architecture/hosted-architecture.md`

## Goal

Define the fourth implementation phase for `v0.19` around one clear change:

1. keep the current worker slice/stage/flush model,
2. keep world-based checkpoints and replay from P3,
3. make the journal backend genuinely switchable,
4. support both Kafka and SQLite behind the same backend seam,
5. stop treating "broker Kafka vs embedded Kafka" as the backend architecture.

This phase is not about unifying product surfaces yet.
It is about finishing the journal abstraction so Phase 5 can collapse the node shape cleanly.

## Why This Exists

P2 and P3 already removed the larger architectural mistakes:

1. ingress is no longer Kafka-shaped,
2. ownership is no longer ingress-assignment-shaped,
3. discovery and checkpoints are now world-based,
4. replay/open already works from world checkpoints plus journal tail.

That leaves one remaining backend asymmetry:

1. the worker talks to a `JournalBackend`,
2. but the concrete implementation is still effectively Kafka-only,
3. and the shared cursor/commit model still assumes Kafka partition offsets.

So the seam exists, but it is not yet the right seam.

## Current State In Code

The runtime is already in a much better position than the directive assumed when Phase 4 was
written.

Implemented already:

1. the shared journal contract exists in
   [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:107)
2. hosted worker flush uses that contract in
   [worker/journal.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/journal.rs:57)
3. world open/replay uses that contract in
   [worker/worlds.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/worlds.rs:576) and
   [services/replay.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/services/replay.rs:222)
4. Kafka already implements the contract in
   [infra/kafka/mod.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/infra/kafka/mod.rs:185)
5. world checkpoints already carry journal cursor metadata for replay in
   [log.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/log.rs:134)
6. the shared cursor and commit model is now backend-aware:
   - `WorldJournalCursor` is an enum in
     [log.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/log.rs:134)
   - `JournalCommit` now reports per-world cursors in
     [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:82)
7. Kafka conforms to the new shared model and returns per-world Kafka cursors in
   [infra/kafka/mod.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/infra/kafka/mod.rs:218)
8. checkpoint post-commit no longer reconstructs Kafka metadata in the worker; it persists backend
   commit cursors directly in
   [worker/journal.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/journal.rs:297)
9. a fresh hosted SQLite journal backend now exists in
   [infra/sqlite/backend.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/infra/sqlite/backend.rs:1)
10. hosted journal infra now wraps a concrete backend enum rather than Kafka only in
    [worker/types.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/types.rs:276)
11. hosted runtime now has a SQLite-backed constructor in
    [worker/runtime.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/runtime.rs:336)
12. the hosted product surface now accepts explicit journal backend selection through
    [main.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/main.rs:136)
13. integration coverage now exists for:
    - Kafka journal backend e2e in
      [tests_e2e/kafka_broker_backend_e2e.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/tests_e2e/kafka_broker_backend_e2e.rs:1)
    - hosted SQLite reopen/replay in
      [tests/sqlite_runtime.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/tests/sqlite_runtime.rs:1)

What remains after P4 is intentionally narrow:

1. `JournalSourceAck` is still Kafka-specific in
   [backends.rs](/Users/lukas/dev/aos/crates/aos-node/src/model/backends.rs:47), because Kafka is
   still the only backend that performs durable source acknowledgement during flush
2. Kafka-specific debug/inspection remains intentionally separate and only works for Kafka, which
   is the desired boundary for `KafkaDebugService`

These are not considered open Phase 4 blockers.

## Design Stance

### 1) Preserve the worker model from P2/P3

P4 should not redesign hosted execution.

Keep:

1. per-world serialized service,
2. speculative `CompletedSlice` staging,
3. durable flush as the publication fence,
4. rollback/reopen on flush failure,
5. replay from world checkpoint plus journal tail,
6. optional wait-for-flush on direct acceptance.

The change is strictly the journal backend and its metadata model.

### 2) Journal metadata must be backend-aware, not Kafka-shaped

The shared model should say:

1. a world checkpoint may carry an opaque or typed journal cursor,
2. a successful journal commit returns backend-specific durable cursor information,
3. replay asks the backend for tail frames after a checkpoint height and optional cursor,
4. the worker never reconstructs Kafka metadata itself.

P4 should remove the assumption that "durable cursor" means "topic partition offset."

### 3) SQLite should be rewritten as a dedicated journal backend

Do not adapt the current local SQLite runtime store in place.

That code mixes:

1. journal frames,
2. runtime counters,
3. world directory,
4. checkpoint heads,
5. command projection.

That is the wrong center for Phase 4.

The SQLite journal backend should be a fresh, single-purpose storage engine:

1. append-only world frame log,
2. durable per-world head metadata,
3. durable disposition storage if needed,
4. backend cursor generation for replay,
5. nothing about command projection, world inventory, or checkpoint persistence.

### 4) Kafka should remain a first-class journal backend, but only as a backend

Kafka is still valid as a journal backend after P2/P3.

Its remaining responsibilities should be:

1. append frames/dispositions transactionally,
2. recover journal partitions,
3. expose per-world frames and cursor-aware tails,
4. produce backend-specific commit metadata,
5. support Kafka-specific debug inspection.

It should no longer define the shared model shape.

## Required Shared Model Changes

### 1) Generalize `WorldJournalCursor`

This is complete.

Replace the current Kafka-only cursor:

```rust
pub struct WorldJournalCursor {
    pub journal_topic: String,
    pub partition: u32,
    pub journal_offset: u64,
}
```

with a backend-aware cursor, for example:

```rust
pub enum WorldJournalCursor {
    Kafka {
        journal_topic: String,
        partition: u32,
        journal_offset: u64,
    },
    Sqlite {
        frame_offset: u64,
    },
}
```

The exact names are less important than the semantics:

1. the cursor is journal-backend metadata,
2. checkpoints persist it without interpreting it,
3. replay passes it back to the backend,
4. the worker never assumes Kafka fields exist.

### 2) Generalize `JournalCommit`

This is complete.

Replace:

```rust
pub struct JournalCommit {
    pub partition_offsets: BTreeMap<u32, u64>,
}
```

with a per-world durable result, for example:

```rust
pub struct JournalCommit {
    pub world_cursors: BTreeMap<WorldId, WorldJournalCursor>,
}
```

This matches how checkpoints are published now:

1. checkpoints are per world,
2. replay resumes per world,
3. the worker needs the durable cursor for each checkpointed world,
4. it does not need raw partition offsets as the shared result.

### 3) Keep source acknowledgements separate from journal durability

This is partially complete.

`JournalSourceAck` still belongs to flush semantics because the runtime may acknowledge external
sources only after durable commit.

But it should be treated as:

1. ingress-facing metadata,
2. optional per backend,
3. not the shape of the journal commit result.

If Kafka remains the only source that uses durable source acknowledgement for now, that is fine.
Do not let it keep journal commit metadata Kafka-shaped.

## Proposed SQLite Journal Design

### Overview

The SQLite backend should be intentionally small and single-purpose.

I would create a new backend module, likely outside `embedded/`, such as:

1. `crates/aos-node/src/journal/sqlite.rs`
2. or `crates/aos-node/src/model/sqlite_journal.rs`

The key point is that it is a shared journal backend, not a local-runtime helper.

### Responsibilities

The new SQLite journal backend should own only:

1. durable append of `WorldLogFrame`
2. durable append of flush dispositions
3. querying all known world IDs
4. querying durable head per world
5. querying full world history
6. querying cursor-aware world tail
7. returning per-world commit cursor metadata

It should not own:

1. checkpoint storage
2. world inventory beyond worlds observed in the journal
3. command projection
4. runtime submission counters
5. world directory / initial manifest metadata

### Storage Model

I would use one append-only node journal database with a single global frame offset.

Proposed tables:

```sql
create table journal_frames (
    frame_offset integer primary key,
    world_id text not null,
    universe_id text not null,
    world_epoch integer not null,
    world_seq_start integer not null,
    world_seq_end integer not null,
    frame blob not null
);

create index journal_frames_world_seq_idx
    on journal_frames(world_id, world_seq_start);

create table journal_world_heads (
    world_id text primary key,
    next_world_seq integer not null,
    last_frame_offset integer not null
);

create table journal_dispositions (
    disposition_offset integer primary key,
    world_id text not null,
    disposition blob not null
);
```

I would also keep a tiny metadata table for schema version and migration state:

```sql
create table journal_meta (
    singleton integer primary key check (singleton = 1),
    schema_version integer not null
);
```

### Why this design

This gives the backend the exact things Phase 4 needs:

1. a single durable append order via `frame_offset`
2. efficient per-world replay via `(world_id, world_seq_start)`
3. cheap `durable_head(world_id)` from `journal_world_heads`
4. easy cursor generation via `WorldJournalCursor::Sqlite { frame_offset }`
5. no coupling to local runtime counters or projection state

### Transaction model

`commit_flush(...)` should run in one SQLite transaction:

1. validate each frame is contiguous with `journal_world_heads.next_world_seq`
2. insert frames in flush order
3. update `journal_world_heads`
4. insert any durable dispositions
5. commit

On success, return the last committed `frame_offset` per touched world.

This gives the same invariant as Kafka:

1. frames become visible together at commit,
2. world durable head advances only on commit,
3. post-commit work can safely use returned cursors,
4. wait-for-flush semantics remain correct.

### Replay path

`world_frames(world_id)`:

1. select frames by `world_id`
2. order by `world_seq_start`

`world_tail_frames(world_id, after_world_seq, cursor)`:

1. if cursor is `Sqlite { frame_offset }`, select frames for the world with
   `frame_offset > ? and world_seq_end > ?`
2. otherwise select by `world_seq_end > ?`

That mirrors the Kafka cursor-aware optimization without embedding Kafka semantics.

### SQLite durability settings

I would use:

1. `journal_mode = WAL`
2. `synchronous = FULL`
3. a reasonable `busy_timeout`
4. one connection owned by the backend, because the worker already has a single logical writer

`FULL` is the right default here because the journal is the durable source of truth.

### What I would explicitly not do in P4

1. no attempt to merge checkpoint storage into SQLite
2. no attempt to preserve the current local runtime schema
3. no fancy multi-writer story
4. no backend-level compaction yet

P4 should make the backend correct and swappable first.

## Required Hosted Changes

### 1) Replace Kafka-only hosted journal infra

This is in progress.

`HostedJournalInfra` should stop being:

1. a thin wrapper around `HostedKafkaBackend`

and become:

1. an enum over supported concrete backends,
2. or a thin wrapper around a `Box<dyn JournalBackend>`

Given the existing Kafka-only debug surface, an enum is likely cleaner:

```rust
pub enum HostedJournalBackend {
    Kafka(HostedKafkaBackend),
    Sqlite(HostedSqliteJournalBackend),
}
```

`HostedJournalInfra` can then wrap that enum and continue implementing `JournalBackend`.

### 2) Move backend-specific cursor stamping out of the worker

This is complete.

`apply_checkpoint_post_commit()` in
[worker/journal.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/journal.rs:297)
should stop:

1. reading Kafka config,
2. computing partitions,
3. merging previous cursor offsets manually.

Instead it should:

1. read `JournalCommit.world_cursors`,
2. attach the returned cursor for the affected world if present,
3. persist it directly in the checkpoint record.

That is the cleanest separation point in the current codebase.

### 3) Make runtime/bootstrap config choose a journal backend explicitly

This is in progress.

Replace the current Kafka-first constructors in
[worker/runtime.rs](/Users/lukas/dev/aos/crates/aos-node-hosted/src/worker/runtime.rs:202)
with an explicit backend selection model:

1. Kafka journal backend config
2. SQLite journal backend config

`partition_count` should become Kafka-specific config, not general worker config.

### 4) Keep Kafka debug inspection separate

`KafkaDebugService` remains valid after P4.

It should stay:

1. Kafka-specific
2. test/debug-only
3. unavailable when SQLite is the selected journal backend

This is not a problem.
It is exactly the right place for Kafka semantics that do not belong in the shared model.

## Implementation Order

### P4.1 Generalize the shared journal metadata model

Implemented.

1. change `WorldJournalCursor`
2. change `JournalCommit`
3. update checkpoint records and replay call sites
4. keep Kafka behavior working with the new model

### P4.2 Make Kafka conform to the new model

Implemented.

1. return per-world cursors from Kafka commit
2. stop exposing partition offsets as the shared result
3. update post-commit checkpoint publication to consume backend-neutral cursors

### P4.3 Build the new SQLite journal backend from scratch

Implemented for hosted runtime.

1. create a new shared SQLite journal module
2. implement `JournalBackend`
3. add focused backend tests for append, replay, and non-contiguous world-seq rejection

### P4.4 Wire hosted runtime to select the backend

Implemented.

1. add hosted journal backend config
2. introduce hosted backend enum/wrapper
3. run hosted reopen/replay against both Kafka and SQLite backends
4. expose hosted CLI/runtime selection for `kafka|sqlite`

### P4.5 Trim backend-specific product assumptions

Implemented.

1. make `partition_count` Kafka-specific
2. keep `KafkaDebugService` as the only Kafka-semantic service
3. remove stale “embedded vs broker Kafka” language from hosted runtime constructors

Completed so far:

1. `HostedWorkerConfig` no longer carries `partition_count`; it is now a Kafka-runtime concern
   rather than a general worker setting
2. hosted bootstrap/CLI builder names are now Kafka-specific:
   - `build_worker_runtime_kafka(...)`
   - `require_kafka_journal_config(...)`
3. hosted runtime Kafka constructors are now named explicitly:
   - `new_kafka(...)`
   - `new_kafka_with_default_universe(...)`
   - `new_kafka_with_default_blobstore(...)`
   - `new_kafka_with_state_root(...)`
   - `new_kafka_with_state_root_and_universe(...)`
4. embedded Kafka constructors are now named explicitly:
   - `new_embedded_kafka(...)`
   - `new_embedded_kafka_with_state_root(...)`
   - `new_embedded_kafka_with_state_root_and_universe(...)`
5. SQLite configuration remains intentionally minimal:
   - external surface: `--journal-backend sqlite` plus `--state-root`
   - internal path: `{state_root}/journal.sqlite3`
   - internal timeout default: `busy_timeout_ms = 5000`
   - no dedicated SQLite CLI/env tuning knobs are exposed in P4

## Acceptance Criteria

Phase 4 is done when:

1. the shared journal cursor and commit model is no longer Kafka-shaped
2. Kafka still works as the journal backend
3. a new SQLite journal backend exists and passes the same backend conformance tests
4. hosted replay/open/checkpoint publication works unchanged against either backend
5. the worker core does not compute Kafka partition metadata during checkpoint publication
6. hosted runtime/backend selection is explicit and no longer described as “broker vs embedded”

## Non-Goals

P4 should not:

1. collapse `aos-node-local` and `aos-node-hosted`
2. move checkpoints into SQLite
3. redesign the worker scheduler
4. redesign the current blobstore checkpoint backend
5. preserve compatibility with the current local SQLite schema

Those belong to later cleanup or explicit future phases.
