# P7: Hosted World Upload and Create-From-Manifest

**Priority**: P7  
**Effort**: High  
**Risk if deferred**: High (hosted workers can run seeded worlds, but there is still no first-class path equivalent to local `aos init` + `aos push` for uploading a real world from AIR plus compiled modules)  
**Status**: Complete

## Goal

Add the missing hosted upload/bootstrap path so a client can upload a real world built from:

- AIR assets,
- compiled workflow modules,

without first constructing a full baseline snapshot locally.

This milestone should make normal hosted world creation feel like:

- local authoring/build happens on the client,
- immutable artifacts are uploaded to hosted CAS,
- the hosted node constructs the first authoritative baseline snapshot,
- hosted workers can then restore and run the world normally.

This is an extension of the existing hosted world create API, not a separate endpoint.

## Status Snapshot

Implemented:

- `POST /v1/universes/{universe_id}/worlds` now accepts a tagged `CreateWorldRequest` with:
  - `source.kind = "seed"` for exact baseline restore/import,
  - `source.kind = "manifest"` for hosted bootstrap from uploaded manifest artifacts.
- The seed-based persistence primitive remains available internally as a lower-level `CreateWorldSeedRequest`.
- The hosted node manifest path now:
  - validates the uploaded manifest hash,
  - creates a temporary hosted world metadata shell,
  - opens the world from manifest through the normal hosted runtime path,
  - lets the hosted runtime establish and promote the first authoritative baseline snapshot,
  - returns a normal `WorldCreateResult`.
- Bootstrap failure cleanup is wired so a manifest-create world that fails before baseline promotion is removed from the normal hosted catalog/state.
- Workspace bootstrap remains out of scope for create and continues to be a follow-on step.

Coverage shipped with this milestone:

- memory-backed control API coverage for `source.kind = "manifest"`,
- existing hosted/FDB coverage for restore/open/fork paths and seeded create compatibility.

Remaining gap:

- there is not yet a dedicated real-FoundationDB integration test that exercises the manifest create path through the node control API.

## Current State

Today the hosted system already has the core low-level pieces:

- universe-scoped CAS upload and read,
- hosted world creation from a promoted baseline snapshot seed,
- hosted world restore from active baseline,
- governance control mailbox for manifest-changing commands,
- direct workspace tree construction APIs by root hash.

That means the substrate is mostly present, but it is still too low-level for a normal “upload this world” workflow.

The main missing product/API layer is:

1. create a world directly from a manifest hash and uploaded artifacts,
2. synthesize and promote the first baseline snapshot on the server side,
3. expose an end-to-end hosted upload flow that a future hosted CLI can use directly through the normal world create endpoint.

## Problem Statement

The existing hosted `CreateWorldRequest` is deliberately low-level and expects a `WorldSeed` whose baseline is already a valid promoted `SnapshotRecord`.

That is the right primitive for:

- import/export,
- exact replication,
- migrations,
- restore from known immutable roots.

It is the wrong user-facing primitive for the common case:

- author AIR locally,
- compile workflow locally,
- upload artifacts,
- create a fresh hosted world from those artifacts.

Requiring the client to construct the first kernel snapshot locally for that common case adds unnecessary complexity and creates a poor UX boundary.

## Design Stance

- Keep the current seeded-baseline create semantics as the authoritative low-level primitive.
- Reuse the existing world create endpoint instead of adding a second create route.
- Extend the create request with an explicit tagged source contract.
- Build initial runtime state on the hosted node, not on the client, for this path.
- Keep immutable artifacts in CAS and keep the server-created baseline snapshot content-addressed and auditable.
- Preserve the distinction:
  - `source.kind = "seed"` is for exact restore/import,
  - `source.kind = "manifest"` is for normal hosted upload/bootstrap.
- Keep workspace creation/sync out of create.
- Use the already-existing workspace APIs and `sys/WorkspaceCommit@1` flow as a separate second step after world creation.

## API Model

Retain the current world create route:

```text
POST /v1/universes/{universe_id}/worlds
```

but change the request shape from a single `seed` contract to a tagged source union.

Suggested request shape:

```text
CreateWorldRequest {
  world_id?: WorldId,
  placement_pin?: String,
  created_at_ns: u64,
  source: CreateWorldSource,
}
 
CreateWorldSource =
  | Seed { seed: WorldSeed }
  | Manifest { manifest_hash: String }
```

Rust shape:

```rust
pub struct CreateWorldRequest {
    pub world_id: Option<WorldId>,
    pub placement_pin: Option<String>,
    pub created_at_ns: u64,
    pub source: CreateWorldSource,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateWorldSource {
    Seed { seed: WorldSeed },
    Manifest { manifest_hash: String },
}
```

Validation rules:

- `world_id`, when absent, is allocated by the server.
- `placement_pin`, when present, must be non-empty.
- `source.kind = "seed"` must pass existing baseline seed validation unchanged.
- `source.kind = "manifest"` must provide a non-empty `manifest_hash` that already exists in universe CAS.

Response shape should remain:

```text
WorldCreateResult
```

The response should always include the actual created `world_id`, whether it was caller-provided or server-allocated.

The resulting world should be indistinguishable from a world created by seeded baseline once creation has completed.

## Server-Side Create Flow

### `source.kind = "seed"`

This path preserves current semantics:

1. Validate the provided `WorldSeed`.
2. Allocate or validate `world_id`.
3. Create the world from the supplied promoted baseline snapshot.

This path remains the import/replication/restore primitive.

### `source.kind = "manifest"`

The hosted node should execute the following sequence:

1. Validate request fields and CAS existence for the manifest hash.
2. Allocate or validate `world_id`.
3. Create the hosted world metadata shell in the universe catalog in a pre-restore state.
4. Open a hosted world from `manifest_hash`.
5. Start from otherwise empty runtime state.
6. Drain the world until quiescent.
7. Create the first authoritative snapshot.
8. Promote that snapshot as the active baseline.
9. Publish normal head/query projections.
10. Return the created world record.

Important semantic rule:

- for `source.kind = "manifest"`, the initial baseline snapshot is created by the hosted node, not supplied by the caller.

That means the server, not the client, becomes responsible for materializing the first snapshot bytes and indexing the resulting `SnapshotRecord`.

## Workspace Is A Second Step

P7 should not include workspace bootstrap or workspace sync in the create API.

That work already has an existing semantic path and API surface:

- build workspace roots,
- upload their blobs,
- submit `sys/WorkspaceCommit@1` through domain ingress,
- observe workspace projections afterward.

Those steps should happen after world creation, not inside it.

This keeps P7 sharply scoped:

- create the world from manifest,
- then sync/create workspaces separately.

## Hosted Upload Workflow

The intended future hosted upload client flow should be:

1. Load AIR from the local project.
2. Compile workflow modules locally.
3. Resolve manifest module hashes locally.
4. Build workspace roots locally from filesystem contents.
5. Upload manifest bytes, module blobs, and workspace/file blobs to hosted CAS.
6. Call `POST /v1/universes/{universe_id}/worlds` with:
   - `source.kind = "manifest"`
   - `manifest_hash`
   - optional `placement_pin`
7. After world creation succeeds, use the existing workspace APIs to create/sync workspace state.
8. Poll world readiness or inspect runtime/head after creation.

This creates the hosted equivalent of:

- local `aos init` for first world creation,
- only the manifest/bootstrap portion of the initial `aos push`.

Workspace population remains a separate follow-on step.

## Relationship To Hosted Push

P7 only covers first-world bootstrap from uploaded manifest artifacts.

It does not fully solve subsequent hosted updates for existing worlds.

Follow-on hosted push/update behavior should use:

- governance apply for manifest changes,
- `sys/WorkspaceCommit@1` domain ingress for new workspace roots,
- existing CAS upload and workspace root construction helpers.

In other words:

- `POST /worlds` with `source.kind = "manifest"` is for world birth,
- hosted push is for later evolution of an existing world.

## Implementation Shape

Suggested implementation additions:

### `aos-fdb`

Change the hosted request type from the current seed-only shape to a tagged source shape:

```rust
pub struct CreateWorldRequest {
    pub world_id: Option<WorldId>,
    pub placement_pin: Option<String>,
    pub created_at_ns: u64,
    pub source: CreateWorldSource,
}

pub enum CreateWorldSource {
    Seed { seed: WorldSeed },
    Manifest { manifest_hash: String },
}
```

This type should live in the existing hosted protocol surface, and the actual manifest-based world materialization should remain implemented through the hosted world engine path rather than direct persistence shortcuts.

### `aos-fdb-node`

Add:

- keep the existing `POST /v1/universes/{universe_id}/worlds` route
- update the facade method to dispatch by `source.kind`
- bootstrap helper that:
  - opens hosted world from manifest hash,
  - drains to quiescence,
  - snapshots,
  - finalizes world metadata/projections

### `aos-world`

No new alternate execution model should be introduced.

Reuse:

- open hosted from manifest hash,
- normal domain event submission,
- normal drain,
- normal snapshot creation.

If a small helper is useful, add a narrow bootstrap helper around those existing operations rather than duplicating world-init logic in the node crate.

## Failure Semantics

Creation must be all-or-nothing from the caller’s perspective.

Required behavior:

- if validation fails before world materialization starts, return error and create nothing,
- if bootstrap fails before first baseline promotion completes, the world must not be left appearing active and runnable,
- partially-created metadata should be cleaned up or marked failed and hidden from normal list/read surfaces until explicitly recovered,
- a successful response means:
  - active baseline exists,
  - manifest head projection exists,
  - hosted workers can restore the world.

Idempotency follow-up:

- explicit client-supplied idempotency for create-from-manifest is desirable,
- but if deferred, the API must still avoid ambiguous success semantics.

## Non-Goals

- Replacing baseline-seeded world create.
- Full hosted push/update workflow for existing worlds.
- Server-side compilation of workflows from source trees.
- Workspace bootstrap during world create.
- Turning world bootstrap into a generic async operation framework beyond what is required here.

## Acceptance Criteria

P7 is complete when:

1. A client can upload AIR and compiled module artifacts to hosted CAS and create a world without supplying a caller-built baseline snapshot.
2. `POST /v1/universes/{universe_id}/worlds` supports both seeded and manifest-based creation through an explicit tagged request shape.
3. The hosted node constructs and promotes the first authoritative baseline snapshot on the server side for manifest-based creation.
4. Hosted workers can restore and run a world created through the manifest-based path without any extra manifest bootstrap steps.
5. Baseline-seeded create remains supported for import/replication flows.
6. Workspace create/sync remains a separate post-create step using existing APIs.

## Recommended Follow-On

Once P7 lands, the next practical follow-on should be a hosted CLI or client workflow that wraps:

- artifact upload,
- `POST /worlds` with `source.kind = "manifest"`,
- workspace root build and sync as a second step,
- later hosted push for existing worlds.

That is the layer that should answer the user-facing question:

- “can I upload a real world and have it start working in hosted workers?”

P7 provides the missing server-side primitive needed for that answer to become yes.
