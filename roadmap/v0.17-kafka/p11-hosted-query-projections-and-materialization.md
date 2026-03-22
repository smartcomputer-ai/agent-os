# P11: Hosted Query Projections and Materialization

**Priority**: P11  
**Effort**: High  
**Risk if deferred**: Medium/High (hosted read APIs will either route too much traffic to owner
workers or reopen worlds on the read path)  
**Status**: Complete
It is okay to break things. I do not want you to  steps to ensure continuity or temporary solutions. We are on an expreimental branch and can move forward boldly.

## Progress Snapshot

The first hosted projection/control slice now exists, but the accepted steady-state design has
changed.

What exists today:

- hosted control reads current-state query surfaces from SQLite-backed projections rather than
  restoring worlds on the request path
- hosted control serves:
  - `manifest`
  - `defs`
  - `def-get`
  - `state-get`
  - `state-list`
  - `workspace-resolve`
  - `journal/head`
  - `journal`
  - workspace tree helper APIs
  - CAS blob APIs
- hosted materializer SQLite helpers exist under `crates/aos-node-hosted/src/materializer/`
- broker-backed materialization now consumes:
  - compacted `aos-projection` for current-state rows
  - committed `aos-journal` for retained journal tail
- projection rows and SQLite source offsets are persisted together before Kafka offsets are
  committed

Implemented in the revised design so far:

- hosted Kafka/config/dev-script support for a compacted `aos-projection` topic
- projection-topic protocol with stable `world/meta`, `workspace`, and `cell` key families
- `projection_token` carried on current-state projection values, with `world/meta` as the sink-side
  source of truth for the current token
- worker-side projection emission after authoritative journal commit
- materializer rewritten as:
  - projection-topic current-state sink
  - separate `aos-journal` retained-tail sink
- old replay-era materializer steady-state path removed from the active architecture
- kernel/runtime helper support for:
  - generic cell projection deltas
  - typed workspace projection deltas derived from `sys/Workspace@1`
- worker projection emission now behaves as:
  - full snapshot bootstrap once per active-world activation
  - incremental workspace updates from workspace deltas afterward
  - incremental cell upserts / tombstones from cell deltas afterward
- worker-local continuity now behaves as:
  - continuity state is kept in memory on registered worlds, not on active hosts
  - quiescent close/reopen in the same worker process may reuse `projection_token`
  - worker restart loses continuity and forces a new token plus full republish
  - failed projection publish invalidates continuity and forces the next attempt to republish under
    a fresh token
- materializer bootstrap now behaves as:
  - first projection-partition assignment with no stored offset triggers a retained partition scan
  - retained bootstrap establishes the latest retained `world/meta` rows before other projection
    rows
  - retained bootstrap keeps only the latest retained row per projection key
  - stale-token retained rows are ignored by normal token gating during bootstrap application
  - after bootstrap, the materializer returns to normal constant topic consumption from the stored
    source offset

Deferred follow-on:

- broader hosted test polish around token resets, continuity preservation, and cold rebuild may be
  added later, but is no longer required for P11 completion
- `world_epoch` bump handling is not part of P11; simple hosted mode keeps `world_epoch` stable
  and does not implement epoch-transition flows yet

What is no longer the target design:

- replaying/reopening worlds inside the materializer in order to derive current-state projections
  from `aos-journal`
- having the materializer externalize or re-externalize hot cell state as part of projection
  building
- treating journal replay inside the materializer as the normal steady-state way to obtain current
  manifest / workspace / cell projections

Accepted change in direction:

- workers should emit current-state projection updates directly from committed hot world state
- those projection updates should go to a compacted Kafka topic
- the materializer should consume that compacted projection topic and write SQLite
- the materializer should consume `aos-journal` only for retained journal-tail indexing
- control should continue serving from SQLite plus CAS/blobstore

So P11 is no longer "make the journal-replay materializer robust enough for hot worlds". It is now
"replace replay-based current-state materialization with worker-emitted compacted projections".

## Goal

Define the hosted read/query plane that lets the hosted control surface answer the main
current-state queries cheaply without routing every read to the worker that currently owns the
world shard.

The target shape for `v0.17` remains:

- `aos-journal` is authoritative for execution and replay
- workers remain authoritative for world execution
- projections are derived and non-authoritative
- control/query services serve from materialized read models rather than worker RPC

The revised implementation path is:

1. worker commits authoritative world progress to `aos-journal`
2. worker emits derived current-state projection updates from the committed hot `WorldHost`
3. materializer consumes those derived projection updates into local SQLite
4. materializer consumes `aos-journal` only for retained journal-tail state
5. control serves `latest_durable` reads from SQLite plus CAS/blobstore

## Design Stance

The query plane remains subordinate to the log-first runtime.

Required stance:

- workers still own authoritative execution
- `aos-journal` is still the only authoritative replay log
- the projection topic is a derived distribution log, not a second source of truth
- the system must remain correct if projections are stale, rebuilding, or temporarily absent
- control-plane reads should prefer projected `latest_durable` answers, not `latest_live` answers
- the normal hosted control read path should avoid restoring/replaying worlds and should treat that
  path as exceptional fallback or unavailable behavior, not the default serving model
- the materializer must not replay worlds in steady state to build current-state projections
- the materializer must not become a second owner of hot cell-state CAS externalization

This means the hosted query plane is a read optimization and product surface, not part of the
correctness core.

## Why the Previous Design Changes

The earlier journal-replay materializer was good enough to prove:

- the hosted control API shape
- the SQLite serving index
- journal-tail indexing
- `latest_durable` query semantics

But it is the wrong steady-state design for hot worlds.

Main problems with replaying worlds in the materializer:

1. The materializer redoes work the worker already did.
2. Hot cell state may be current in worker memory before it has been externalized as an individual
   CAS blob.
3. The materializer then has to choose between:
   - writing the state again to CAS itself, or
   - waiting for some other checkpoint/snapshot path and serving stale reads
4. Restore/reopen behavior becomes awkward because current-state rebuild now depends on replay
   details rather than on the current hot head state already owned by the worker.

The better boundary is:

- worker owns current committed hot state
- worker emits derived projection updates from that state
- materializer is a dumb sink again

## Core Model

### 1) Worker emits current-state projections from committed hot state

The source of truth for current-state projections should be the committed hot `WorldHost` owned by
the authoritative worker.

This means:

- projection updates are produced only after the authoritative journal commit succeeds
- projection updates are never produced from speculative pre-commit state
- projection updates are derived from the worker's committed hot head state and delta tracking
- the worker does not need to reopen/replay worlds to emit steady-state projections

### 2) Use one compacted projection topic for current-state projections

The first distributed current-state projection channel should be a single compacted Kafka topic,
for example:

- `aos-projection`

This topic is for current-state projections only.

It is not:

- a second authoritative journal
- a request-time query engine
- the journal-tail/history surface

### 3) `projection_token` marks one coherent projected view

Each world has a worker-owned opaque `projection_token`.

Properties:

- it is not an ordered counter
- it is not compared numerically
- it exists only to separate one full projected view of a world from an older one

The worker keeps the current token for a world while steady-state incremental projection updates
remain valid.

The worker mints a new token when it crosses a full-rebuild boundary, such as:

- create-world
- world-epoch change
- restore/reopen where worker-local continuity can no longer be proven

The materializer never asks whether one token is "greater" than another. It only asks whether a
row's token matches the current world token.

### 4) `world/meta` on the compacted topic is the source of truth for the current token

The compacted projection topic must include one stable world-meta record per world.

That world-meta record carries:

- `universe_id`
- the current `projection_token`
- `world_epoch`
- `journal_head`
- `manifest_hash`
- `active_baseline`
- optional small runtime summary bits if helpful

The materializer learns the current token for a world from the latest retained `world/meta` record
on the compacted topic, not from S3-style current-world metadata and not from a worker RPC.

### 5) Materializer consumes projections, not world history, for current-state serving

The materializer's steady-state current-state responsibilities are:

1. consume compacted projection updates
2. apply them into SQLite
3. persist per-topic/per-partition source offsets in SQLite
4. serve as the local sink used by control

The materializer's steady-state current-state responsibilities are not:

1. reopen worlds
2. replay journal history
3. derive current head state by re-running world execution
4. write current hot state into CAS on behalf of the worker

### 6) Journal-tail indexing remains separate from current-state projections

The hosted product still needs:

- `journal/head`
- `journal`

Those are not current-state projections.

The materializer should continue to consume committed `aos-journal` for that retained-tail
surface, but that journal consumption is a separate responsibility from current-state projection
application.

### 7) Freshness remains explicitly `latest_durable`

The first hosted read plane continues to serve:

- `latest_durable`

not:

- `latest_live`

The read plane may lag the active worker slightly, but it must never invent state that was not
already committed to the authoritative system.

## Projection Topic Design

### Topic and Partitioning

Use one compacted topic for current-state projections.

Required rules:

- all projection records for a given `world_id` must be written to the same Kafka partition as that
  world's `aos-journal` partition
- producer code must choose the partition explicitly from `world_id`
- Kafka key bytes are used for compaction identity only, not for partition routing

This matters because a world emits multiple projection-key families and default Kafka key hashing
would otherwise scatter one world's records across partitions.

### Key Families

`world_id` is globally unique, so projection-topic keys do not need `universe_id`.

Readers that need to recover the world-to-universe association should get that from the
`WorldMetaProjection` value on `world/meta`, not from the compacted key.

Logical key families:

```text
ProjectionKey::WorldMeta {
  world_id
}

ProjectionKey::Workspace {
  world_id,
  workspace
}

ProjectionKey::Cell {
  world_id,
  workflow,
  key_hash
}
```

The exact byte encoding can be canonical CBOR or another deterministic stable encoding, but the
logical key family above is the required shape.

There is intentionally no separate `world_reset` key in the first cut. The reset boundary is
expressed by changing the `projection_token` carried by `world/meta`.

### Value Families

The projection topic should reuse the existing projection record shapes where practical, but it
needs a topic envelope that carries `projection_token`.

Suggested logical values:

```text
WorldMetaProjection {
  universe_id,
  projection_token,
  world_epoch,
  journal_head,
  manifest_hash,
  active_baseline,
  updated_at_ns,
  runtime_summary?
}

WorkspaceProjectionUpsert {
  projection_token,
  record: WorkspaceRegistryProjectionRecord
}

CellProjectionUpsert {
  projection_token,
  record: CellStateProjectionRecord,
  state_payload: CborPayload
}
```

Notes:

- `WorldMetaProjection` subsumes what SQLite today stores separately as head/world rows
- SQLite may still keep separate `head_projection` and `world_projection` tables if that remains
  convenient for serving code
- `WorkspaceRegistryProjectionRecord` and `CellStateProjectionRecord` remain the right served-row
  shapes

### Delete Semantics

Current-state deletes should use Kafka tombstones on the stable per-row key:

- workspace deletion -> tombstone on `ProjectionKey::Workspace`
- cell deletion -> tombstone on `ProjectionKey::Cell`

No tombstone is needed for "full rebuild" of a world. A new `projection_token` on `world/meta`
logically supersedes older cell/workspace rows for that world.

### Payload Semantics for Cell State

The projection topic must support both:

- `cbor_ref` / externalized payloads
- inline state bytes

Required rule:

- if the committed head cell state already has a CAS/blob reference, worker should emit a
  reference-backed `CborPayload`
- if the committed head cell state is still only hot worker state, worker may emit inline bytes

This is the key simplification versus the replay-based materializer:

- the worker can project current hot state without forcing it through a second CAS writer
- the materializer simply persists the payload it receives
- control already knows how to serve either inline bytes or a CAS/blob reference

## Projection Families

The first hosted query plane still serves the same functional families:

### 1) World / Head Projection

Purpose:

- cheap current-head manifest reads
- current defs / def-get without restoring the world
- current runtime summary / active-baseline summary for hosted world listings

Materialization source:

- `WorldMetaProjection` from the compacted projection topic

Served shape:

- existing `HeadProjectionRecord`
- existing `MaterializedWorldRow`

### 2) Workspace Registry Projection

Purpose:

- cheap `workspace/resolve` for current named workspace bindings

Scope:

- current workspace name/version/root binding
- not a duplicate of the entire workspace tree

Materialization source:

- `WorkspaceProjectionUpsert`
- tombstones for workspace deletes

### 3) Latest-Durable Cell State Projection

Purpose:

- `state-get` / `state-list` for durable current cell state without reopening the world

Required semantics:

- the answer is `latest_durable`
- control should fetch actual bytes from CAS/blob storage only when the projection payload is a
  reference
- projection rows may inline bytes when the worker is projecting still-hot state

Materialization source:

- `CellProjectionUpsert`
- tombstones for cell deletes

### 4) Retained Journal Tail

Purpose:

- `journal/head`
- `journal`

Materialization source:

- committed `aos-journal`

This remains a separate feature family from current-state projections above.

## Worker Responsibilities

### Steady-State Projection Emission

After a committed world update:

1. the worker commits authoritative progress to `aos-journal`
2. the worker promotes the speculative/hot world state that now reflects that commit
3. the worker emits:
   - updated `world/meta`
   - incremental workspace projection updates
   - incremental cell projection upserts / tombstones

Expected worker-side data sources:

- hot `WorldHost`
- existing kernel cell-delta tracking
- direct reads of current workspace state from the hot host

### Full Rebuild Boundaries

When the worker crosses a full rebuild boundary, it must mint a new `projection_token` and emit a
full current-state projection snapshot for the world.

Required full-rebuild boundaries:

- create-world
- world-epoch change
- restore/reopen where worker-local continuity cannot be proven

On a full rebuild, the worker should:

1. mint a new `projection_token`
2. emit `world/meta` first with the new token
3. emit the full current workspace projection set
4. emit the full current cell projection set

The worker should not read the projection topic to decide whether to do this.

### Worker-Local Continuity State

To avoid a projection-topic read-before-write dependency, continuity after restore/reopen should be
decided from worker-local in-memory metadata, not from sink state.

The worker should keep small in-memory continuity state keyed by `world_id`, attached to the
worker's registered-world state rather than the active host instance, containing at least:

- `projection_token`
- `world_epoch`
- last projected `journal_head`
- active-baseline identity sufficient to detect restore/reopen continuity mismatches

This continuity state is process-local only.

Required stance:

- if a world is closed for quiescence and later reopened in the same worker process, continuity may
  be preserved and the token may be reused
- if the worker process restarts, continuity state is lost and the first reopen should mint a new
  token and republish a full current-state snapshot

On reopen in the same worker process:

- if the in-memory continuity state proves continuity, keep the token and continue incremental
  emission
- if continuity state is missing or continuity no longer holds, mint a new token and emit a full
  projection snapshot

On worker restart:

- continuity is not persisted
- the next reopen should mint a new token and republish the full current-state projection set

### Transaction Boundary

The first cut does not need to make projection-topic writes part of the same Kafka transaction that
writes `aos-journal`.

Accepted first-cut behavior:

1. worker commits the authoritative journal batch
2. worker emits projection-topic updates afterward
3. if the worker dies after journal commit but before projection emit, recovery is by later
   full-rebuild emission with a new token

This keeps the projection plane derived and repairable.

A future optimization may place journal and projection-topic writes into one Kafka transaction, but
that is not required for P11.

## Materializer Responsibilities

### Current-State Projection Sink

The materializer should consume the compacted projection topic and write SQLite.

Required behavior:

- persist per-topic/per-partition source offsets in SQLite
- treat SQLite offsets as the authoritative materializer recovery cursor
- keep a local "current token by world" view derived from `world/meta`
- apply workspace/cell rows only when their `projection_token` matches the current token for that
  world

When `world/meta` changes token for a world:

1. update the current token for that world
2. clear that world's current-state rows in SQLite
3. materialize the new world/head row state from the new meta record
4. accept subsequent workspace/cell rows only if they match the new token

### Bootstrap from a Compacted Topic

Cold rebuild of an empty SQLite DB should not require worker RPC.

Required stance:

- the retained compacted projection topic must be sufficient to rebuild current-state projections
- bootstrap should be deterministic even if stale rows from older tokens remain in the compacted
  log

Recommended bootstrap approach:

1. scan retained projection-topic state to determine the latest `world/meta` record for each world
2. apply retained workspace/cell rows only if their token matches that world's current token

This two-phase bootstrap avoids trusting stale rows from superseded tokens.

### Journal-Tail Sink

The materializer should continue consuming committed `aos-journal` for retained journal-tail
indexing.

Required behavior:

- append journal rows in commit order
- persist journal-topic offsets in SQLite
- serve `journal/head` and `journal` from SQLite

When a world crosses a new projection-token boundary, the materializer may drop retained journal
tail for that world.

Accepted first-cut behavior:

- on a new token for a world, delete retained `journal_entries` for that world
- set `retained_from = journal_head + 1`
- resume appending new journal rows from `aos-journal`

This means journal-tail retention is operational and best-effort, not a complete current-state
rebuild requirement.

## Control / Query Service

The first hosted control/query process should:

1. run the materializer alongside the control API surface or connect to an external materializer
2. keep the latest materialized projection state in a local SQLite index
3. serve HTTP/API reads from that local index plus CAS/blob fetches when needed
4. avoid restoring/replaying worlds on the normal request path

Target read mapping:

- `manifest`, `defs`, `def-get`: served from world/head projection + CAS
- `workspace-resolve`: served from workspace projection
- `state-get`, `state-list`: served from latest-durable cell projection + payload/CAS
- `journal/head`, `journal`: served from retained journal-tail index

## Recovery and Rebuild

The projection plane must remain rebuildable from authoritative or retained derived sources.

Required rule:

- SQLite serving indexes are disposable caches

Current-state rebuild source:

- retained compacted projection topic

Journal-tail rebuild source:

- retained `aos-journal` rows seen by the materializer after the current retained boundary

This means:

- current-state SQLite loss must not require replaying worlds inside the materializer
- journal-tail history may be intentionally truncated at projection-token reset boundaries
- worker hot state remains authoritative for generating new derived projection snapshots

## Implementation Seams

This refactor should stay almost entirely inside `aos-node-hosted`.

Intentional scope boundary:

- nothing new should go into `aos-node` for this work; the projection topic, token handling,
  materializer sink, and continuity logic are hosted deployment concerns
- `services/` should remain a control/read facade rather than becoming a write-path or
  materialization layer
- `aos-runtime` / `aos-kernel` should change only if a small additive helper materially simplifies
  hosted worker projection extraction

Planned ownership split:

### `crates/aos-node-hosted/src/infra/kafka/`

This layer should own projection-topic transport and protocol, not projection derivation.

Planned additions:

- projection-topic config, for example a compacted `aos-projection` topic name in hosted Kafka
  config
- projection-topic key/value envelopes and deterministic encoding helpers
- explicit partition-routing helper so all records for one `world_id` land on that world's journal
  partition
- worker-side producer helper for projection records
- materializer-side consumer helper for projection records
- embedded-mode equivalents if hosted embedded tests should continue exercising the same behavior

This layer should not own:

- SQLite schema or apply logic
- worker continuity decisions
- hot-world projection extraction
- materializer rebuild policy beyond decoding topic records

### `crates/aos-node-hosted/src/worker/`

This layer should own projection derivation and projection emission because the worker owns the
authoritative committed hot world state.

Planned additions:

- a new worker-local `projections` module
- extraction helpers for:
  - `world/meta`
  - full workspace snapshot rows from hot host state
  - incremental cell upserts / deletes from drained projection deltas
  - `CborPayload` resolution as inline bytes vs `cbor_ref`
- emission helpers that publish projection-topic batches after authoritative journal commit and
  speculative-world promotion
- in-memory worker-local continuity state keyed by `world_id`, holding at least:
  - `projection_token`
  - `world_epoch`
  - last projected `journal_head`
  - active-baseline identity sufficient to detect restore/reopen continuity mismatch

Planned integration points:

- post-commit steady-state emission in the worker submission path
- restore/reopen decision logic in the worker lifecycle path
- full rebuild emission on:
  - create-world
  - world-epoch change
  - restore/reopen continuity break

This layer should remain the only place that decides whether to keep a token or mint a new one.

### `crates/aos-node-hosted/src/materializer/`

This layer should become a sink/apply role only.

Planned shape:

- keep SQLite schema and served-row types here
- add projection-topic apply logic that:
  - tracks current `projection_token` by world from `world/meta`
  - clears current-state rows for a world when the token changes
  - applies workspace/cell upserts and tombstones only when their token matches the current world
    token
- keep journal-tail indexing here as a separate responsibility from current-state projection apply
- keep source-offset persistence here for both:
  - projection-topic partitions
  - `aos-journal` partitions

Explicit de-scope:

- replaying/reopening worlds inside the materializer should be removed or demoted out of the
  steady-state path
- the materializer should not write current hot cell state into CAS on behalf of the worker

### `crates/aos-node-hosted/src/services/`

This layer should stay thin and mostly unchanged.

Expected role:

- `services/projections.rs` remains a read-only facade over materialized SQLite state for control
- `services/journal.rs`, `services/meta.rs`, and `services/cas.rs` stay focused on their current
  service surfaces

Not planned here:

- projection-topic emission
- projection-topic consumption
- rebuild/continuity policy
- SQLite write logic

### `aos-runtime` / `aos-kernel`

The first cut should require little or no core-runtime change.

Already available and useful:

- hot-world cell projection deltas via `WorldHost::drain_cell_projection_deltas()`
- current head reads via `state()`, `list_cells()`, and `heights()`

Optional later helpers, only if needed:

- a small helper that exposes decoded workspace projection rows directly from hot host state
- richer projection-delta metadata if it reduces repeated hosted-only extraction work

### `aos-node`

No new protocol or abstraction should be added here for this milestone.

Reason:

- this is a hosted query-plane concern, not a generic node/runtime contract
- local/non-hosted paths should not need to understand projection topics or projection tokens

## Suggested Rollout

Completed rollout slices:

1. Added projection-topic protocol/config/producer-consumer helpers under `infra/kafka/`.
2. Added worker-side projection extraction/emission under `worker/`.
3. Reworked `materializer/` into:
   - projection-topic current-state sink
   - separate journal-tail sink
4. Left control/service reads on top of SQLite with the schema/query adjustments required for the
   new projection model.

No remaining rollout slices are required for P11.

## Non-Goals

1. A generic secondary-index framework.
2. A read path that depends on owner-worker RPC for normal current-state queries.
3. Direct point-read serving from Kafka scans.
4. `latest_live` semantics in the first hosted gateway read plane.
5. Treating projections as correctness-critical state.
6. Keeping full retained journal history across every projection reset boundary.

## DoD

P11 is complete when:

1. The hosted roadmap explicitly defines the query plane as derived and non-authoritative.
2. The current-state projection source is the committed hot worker state, not materializer replay.
3. Current-state projection transport is a compacted Kafka topic, not direct worker-memory serving
   and not journal replay inside the materializer.
4. `world/meta` on the compacted topic is defined as the source of truth for the current
   `projection_token`.
5. Workspace/cell rows carry `projection_token` and the materializer applies only rows whose token
   matches the current world token.
6. Workers do not need to read the projection topic in order to decide whether to continue
   incrementally or emit a full rebuild.
7. The materializer no longer replays worlds in steady state to derive current manifest /
   workspace / cell projections.
8. The materializer continues to index retained journal tail separately from current-state
   projections.
9. Hosted control serves the main read APIs from SQLite plus CAS/blobstore and preserves
   `latest_durable` semantics.
10. SQLite source offsets are authoritative recovery cursors for the projection topic and for the
    journal-tail consumer path.
11. Cold rebuild of SQLite from retained compacted projection state is deterministic and does not
    require worker RPC.

## Could Be Added Later

Follow-on work may later add:

- broader hosted projection/materializer test polish around quiescent continuity preservation,
  token resets, and rebuild coverage
- one Kafka transaction covering both authoritative journal and derived projection writes
- separate projection topics by family if a single compacted topic proves insufficient
- richer command projections
- richer gateway freshness reporting
- optional `latest_live` owner-worker query paths
- alternative serving-index backends
- optional admin/recovery tooling for compacted-topic bootstrap and projection repair
