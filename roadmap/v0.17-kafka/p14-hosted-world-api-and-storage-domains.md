# P14: Hosted World API and Universe-Scoped Storage

**Priority**: P14  
**Effort**: Medium  
**Risk if deferred**: High (hosted keeps too much old control-plane shape and remains harder than
necessary to reason about, operate, and extend)  
**Status**: Superseded (keep the world-scoped transport simplification, but do not keep
`universe` as the long-term semantic model)

## Goal

Reframe hosted around a lower-level world API while keeping `universe` as the first-class
semantic and storage boundary.

This is a simplification pass, not a feature-expansion pass.

The intended end state is:

- hosted transport is world-scoped
- world execution identity stays world-scoped
- `universe` remains the user/system concept above those worlds
- a universe is also the storage scope for CAS/blob retention, GC, cleanup, and ref validity
- hosted does not introduce a second top-level writable noun such as `universe` unless there
  is a hard semantic need for shared writable storage across multiple universes
- readonly cross-universe dedupe/common artifacts can be handled later via optional substrate
  layers

## Problem Statement

The earlier P14 framing correctly pushed hosted toward a lower-level world API, but it went too far
in separating storage from universe.

The real semantically loaded things in hosted are:

- `world`, as the unit of execution and ownership
- the retained roots that determine what storage must stay alive
- `universe`, as the live namespace within which worlds communicate and exchange refs

Shared CAS does not by itself force a separate top-level noun. Shared backing storage is already
valid as long as GC walks the union of retained roots before sweep. That means storage sharing is
real, but it does not require a separate user-facing semantic model in the common case.

The problem with `universe` is not that it is impossible. It is that it becomes a second
top-level concept that usually means "the same boundary as universe, but with different words."

That creates avoidable complexity:

- users have to learn both `universe` and `universe`
- control/API/docs have to explain why the two almost always coincide
- ref validity becomes harder to reason about
- lifecycle and cleanup semantics get murkier than they need to be

## Primary Stance

P14 should keep the world-scoped transport simplification, but it should not split storage away
from universe by default.

Required stance:

- keep one ingress topic and one journal topic per hosted deployment
- treat the topic set as a bag of worlds for transport purposes
- make `world_id` the transport and execution identity
- keep `universe` as the first-class semantic namespace above those worlds
- make universe the storage scope too
- avoid introducing `universe` unless we explicitly want multiple universes to share one
  writable GC domain
- treat readonly shared backing CAS as a later substrate concern, not as a new semantic layer in
  the base model

This means:

- the base hosted runtime can still look like a lower-level node/world API
- the product/system story remains "a universe contains worlds"
- storage, GC, retention, and ref validity stay aligned with that universe boundary
- later routing/platform work can add richer universe management without also carrying a separate
  storage noun

## New Model

## 1) Topics And Routing

Hosted should still use shared topics that contain many worlds.

Key idea:

- transport is a bag of worlds
- semantics are still universe-scoped

Required simplification:

- hosted ingress/journal keys should be world-scoped rather than `(universe_id, world_id)`-scoped
- hosted routing tables and local broker state should be keyed by `world_id`
- partitioning should be driven by `world_id`
- `universe_id` should not be a primitive transport key for hosted runtime internals

This keeps the hot path simple without demoting universe out of the user/system model.

## 2) Universe As Semantic And Storage Boundary

Hosted should treat universe as the live semantic namespace and storage scope.

Required semantics:

- a universe contains worlds
- a universe owns the writable CAS/blob namespace used by those worlds
- GC, retention, cleanup, snapshot roots, and ref validity are universe-scoped
- communication between worlds also happens within that same universe
- worlds remain the unit of computation/ownership inside that universe

This is internally coherent:

- worlds are where computation happens
- universes are where reachability and shared durable storage are interpreted

## 3) Universe CAS And Optional Substrates

The clean split is:

- **Universe CAS**: writable, GC-owned, cleanup-owned
- **Substrate CAS**: optional, readonly, externally managed backing storage

If we need cross-universe dedupe, seeding, or common artifacts later, add substrates rather than a
shared writable universe concept.

Required substrate semantics:

- a universe owns its writable CAS namespace and GC policy
- a universe may mount one or more readonly substrate CASes beneath it
- lookup resolves universe-local CAS first, then substrate layers
- new writes always land in the universe-local CAS
- GC only collects the universe-local CAS
- substrates are managed independently
- cross-universe sharing happens because multiple universes read from the same substrate, not
  because they share one live writable GC domain

This preserves a simple operational model:

- deleting a universe deletes its own writable storage scope
- substrate lifecycle is independent
- readonly common artifacts can still dedupe efficiently

## 4) Ref Semantics

Ref semantics should stay contextual rather than "globally resolvable if some store somewhere has
the hash."

Preferred rule:

- a ref is valid in the context of a universe
- the universe resolver may satisfy it from universe-local CAS or from mounted substrates
- substrates are invisible semantically; they are only a resolution implementation detail

This is easier to reason about than promising that any hash is valid everywhere.

## 5) Hosted API Surface

The base hosted API should still be a lower-level world API, but not one that pretends universe no
longer exists.

The intended base surface is:

- health
- list worlds
- get world
- create world from manifest
- create world from seed
- fork world
- submit events
- submit receipts
- submit commands
- read manifest/defs/state/journal/runtime/trace/workspace
- CAS/workspace utility APIs

What should not be core to the base hosted API:

- a separate universe CRUD surface
- coupled multi-universe GC administration
- world handles and handle lookup
- routed placement ownership semantics
- portal-level inter-universe routing concerns

Universe may be implicit in simple hosted mode or explicit in later multi-universe layers, but it
remains the correct semantic/storage boundary either way.

## 6) World Creation

World creation should still be treated as an ingress/journaled world operation, not as a metadata
table insert.

Desired behavior:

- `create world` is submitted into ingress
- worker processes it authoritatively
- world existence is discovered from journal/checkpoint/materialized state
- the hosted API reflects that resulting world state
- the world belongs to a universe chosen by API scope/configuration at create time

Fork/seed expectations:

- fork should inherit the source world's universe by default
- explicit cross-universe clone/import can be considered later if needed
- seed/import create may target a universe explicitly; simple hosted mode can default to one
  configured universe

## 7) World Metadata

For the simplified hosted model, there should be no blobstore world metadata plane.

Required stance:

- do not persist a hosted world-registration object in blobstore
- do not treat blobstore as the source of truth for world existence or world identity
- derive world existence and basic world summaries from:
  - journal
  - checkpoints
  - materialized projections

Preferred direction:

- use `world_id` as the core hosted world identity
- derive as much as possible from journal/checkpoint/materialized state
- avoid rebuilding a mini control database inside blobstore
- treat richer admin metadata as a later platform concern

## 8) Read/Write Split With P11

This item still keeps the P11 read/write split:

- worker/journal path remains authoritative for writes and execution
- materializer remains the read-serving path
- control does not restore worlds ad hoc for normal reads

The simplification here is not about changing P11.
It is about aligning hosted execution, storage, and semantics around world-scoped transport plus
universe-scoped storage.

## What P14 Should Deliver

P14 should leave hosted in this shape:

1. hosted topics and local routing structures are world-scoped, not universe-keyed
2. hosted control stops treating universe as a transport key while keeping it as the semantic and
   storage boundary
3. hosted does not introduce a separate `universe` concept in the base model
4. hosted CAS/blob retention, GC, cleanup, and ref validity are universe-scoped
5. world creation remains journal/worker-authoritative
6. heavyweight world-registration metadata is reduced or removed
7. the hosted API is framed as a lower-level world API that a later routing/platform service can
   compose on top
8. readonly substrate CASes remain an optional later extension for cross-universe dedupe/common
   artifacts

## Historical Note

An earlier P14 implementation direction introduced `universe_id` and used it as the hosted
storage segmentation key.

That work did capture a real need:

- transport should not be universe-keyed
- hosted needs scoped retention and cleanup
- global writable CAS across everything is operationally awkward

But the preferred long-term framing is narrower and cleaner:

- keep the transport simplification
- keep the storage boundary aligned with universe
- reserve a separate storage concept only for the uncommon case where multiple universes are meant
  to share one writable GC domain on purpose

If code still carries `universe_id` internally, it should be treated as transitional or as an
implementation detail unless and until that heavier shared-writable semantic model is explicitly
chosen.

## What P14 Explicitly Does Not Do

P14 does **not** attempt to solve:

- full routed multi-universe control
- higher-order universe management
- portal routing policy across universes
- a shared writable universe spanning multiple universes
- full lifecycle semantics
- hosted secret syncing (`aos world create --sync-secrets` still depends on P15)

Those are later layers.

## Relationship To Later Routing Work

The later routing/platform work should build on top of this simpler hosted shape:

- hosted node/world API at the bottom
- universe as the semantic namespace and storage/GC boundary
- optional readonly substrates as lower storage implementation detail
- higher-level routing/platform metadata above that
- portals as later inter-/intra-universe routing concern

That layering is cleaner than carrying both universe and universe as top-level nouns from the
start.

## CLI/Product Outcome

After P14, simple hosted mode should still feel like this:

1. start `aos-node-hosted` in non-routed mode
2. configure `aos` to talk to that hosted API
3. operate against the configured universe
4. create worlds from manifest / seed / fork
5. list and inspect worlds

This should feel close to local mode, except the backing system is Kafka/blobstore and reads are
materialized.

## DoD

1. Hosted control APIs no longer depend on universe-scoped transport keys for the base hosted world
   API.
2. Hosted transport and local routing state are world-scoped.
3. Hosted keeps universe as the semantic and storage scope.
4. Hosted supports world create from manifest, seed, and fork within a universe scope.
5. Hosted has no blobstore world registry or other blob-backed world metadata plane.
6. Hosted world list/get/runtime information is served from materialized world projections.
7. Hosted remains projection-backed for normal reads and does not regress into ad hoc world restore
   on the read path.
8. Hosted does not introduce `universe` unless later work explicitly chooses a shared
   writable GC domain spanning multiple universes.
9. Any future cross-universe dedupe/common-artifact story is modeled as optional readonly
   substrates, not as a second default semantic namespace.

## Follow-On

Three follow-on items naturally come after this:

1. P15 hosted secrets and secret syncing.
2. A separate routed hosted control plane with explicit universe metadata and managed routing.
3. Optional readonly substrate CAS support if cross-universe dedupe/common artifacts become worth
   the added implementation complexity.
