# P15: Hosted Secrets via Blobstore-Backed Vault

**Priority**: P15  
**Effort**: High  
**Risk if deferred**: High (hosted worlds cannot resolve arbitrary secret bindings safely; remote
CLI flows such as `--sync-secrets` remain broken; worker-env-only operation does not scale to real
multi-world hosted deployment)  
**Status**: In Progress

## Completed In First Cut

The initial dev/test hosted vault is now implemented in `aos-node-hosted`.

Completed:

- `crates/aos-node-hosted/src/vault/` now contains the hosted secret subsystem:
  - `config.rs`
  - `crypto.rs`
  - `blobstore.rs`
  - `service.rs`
  - `resolver.rs`
- hosted control now exposes the CLI-compatible secret routes:
  - `GET /v1/universes/{universe}/secrets/bindings`
  - `PUT /v1/universes/{universe}/secrets/bindings/{binding}`
  - `GET /v1/universes/{universe}/secrets/bindings/{binding}`
  - `DELETE /v1/universes/{universe}/secrets/bindings/{binding}`
  - `POST /v1/universes/{universe}/secrets/bindings/{binding}/versions`
  - `GET /v1/universes/{universe}/secrets/bindings/{binding}/versions`
  - `GET /v1/universes/{universe}/secrets/bindings/{binding}/versions/{version}`
- hosted worker/runtime now injects a hosted `SecretResolver` into kernel opens and world creation,
  so secretful manifests can run under hosted without env-predeclared worker wiring.
- hosted materializer replay also carries the same resolver seam so secretful manifests can be
  replay-opened there too.
- local/dev hosted uses the built-in unsafe dev KEK by default, with env overrides for KEK id/hex
  and optional env fallback.
- the first backend is blobstore-only:
  - object-store-backed when hosted blobstore bucket config is present
  - in-memory when running embedded tests / no shared blobstore is configured
- `aos world create --sync-secrets` now has the hosted API surface it needs.
- integration coverage now includes hosted secret route CRUD/version flow plus resolver round-trip
  on the hosted runtime.

Deliberately simple in this first cut:

- no commercial secret-manager integration yet
- no sophisticated rotation workflow
- no secret GC policy yet
- embedded/no-bucket mode uses in-memory hosted secret storage, so values are not durable across
  process restart there

## Goal

Add a hosted secret subsystem that works in the Kafka/blobstore architecture without introducing a
shared SQL database and without storing secret values in:

- world state
- journal
- CAS
- snapshots
- query projections

The first hosted secret backend should be:

- universe-scoped
- blobstore-backed
- encrypted at rest via envelope encryption
- usable by separate control and worker processes

This is the hosted testing/dev vault and the architectural seam for later commercial secret-manager
integration.

## Problem Statement

Local and hosted have different secret constraints.

### Local today

Local is intentionally env-only for values:

- binding declarations persist in AIR/manifest definitions
- actual values come from env / `.env` / local `aos.sync.json`
- local builds an in-memory resolver at load time
- values are not persisted in local world store or local node DB

That is the correct local stance.

### Hosted problem

Hosted workers cannot rely on the same model:

1. a worker does not know ahead of time which worlds it may host
2. a worker cannot reasonably predeclare all possible env vars for all possible worlds
3. control and worker planes are expected to be separate processes / containers / VMs
4. we do not want to reintroduce a shared DB just for secrets
5. we still need a real hosted testing/dev path before integrating a commercial secret manager

So hosted needs a real secret backend.

## Primary Stance

Hosted secret storage must be a separate subsystem from world persistence.

Required stance:

- binding metadata may be persisted
- encrypted secret versions may be persisted
- secret plaintext must never be persisted
- secret values must not enter the world journal or world state
- secret values must not be written to world CAS
- secret values must not appear in checkpoints/snapshots
- secret values must not appear in query/materializer projections

The hosted secret vault is a control/runtime support system, not part of world state.

## Blobstore-Only Direction

The first hosted vault should use blobstore only.

Reason:

- control and worker planes can both access blobstore
- no shared SQL service is needed
- it matches the rest of the hosted durability model
- it is good enough for dev/test and still viable as a production fallback

Do not introduce:

- a new shared SQLite/Postgres/FDB control DB just for secrets
- secret storage inside the general world CAS
- secret storage inside the materializer DB

## Code Boundary

The hosted secret subsystem should live under:

- `crates/aos-node-hosted/src/vault/`

Recommended split:

- `vault/config.rs`
  - KEK/env configuration such as `HostedSecretConfig`
- `vault/crypto.rs`
  - envelope encrypt/decrypt helpers
- `vault/blobstore.rs`
  - blobstore-backed binding/version persistence
- `vault/service.rs`
  - control-plane binding/version operations
- `vault/resolver.rs`
  - worker/runtime `SecretResolver` implementation
- `vault/mod.rs`
  - exports and top-level assembly

Important boundary:

- this should not live under `control/` because workers also need it
- this should not live under `worker/` because control owns admin writes
- this should not live under `blobstore/` because blobstore is only the storage backend

`vault/` is the right subsystem boundary; blobstore is an implementation detail behind it.

## Replaceability Requirement

The hosted vault interface must be designed so the first blobstore-backed implementation can later
be replaced by a commercial secret manager without changing the rest of the product surface.

Required stability boundary:

- manifests and secret binding ids remain unchanged
- shared DTOs remain unchanged:
  - `SecretBindingRecord`
  - `SecretVersionRecord`
  - `SecretBindingSourceKind`
- worker/runtime continues to depend on `SecretResolver`
- control/CLI continue to depend on the same hosted secret binding/version API shape

This implies an internal hosted vault abstraction under `src/vault/`:

- admin/service trait for binding/version CRUD
- runtime/provider trait for reading and decrypting/resolving secrets

First implementation:

- blobstore-backed hosted vault

Later implementations:

- commercial secret manager adapter
- KMS/HSM-backed vault adapter

What must not become public contract:

- blobstore object key layout
- exact encryption implementation details
- dev KEK behavior
- any blobstore-specific persistence assumptions

Those belong to the first backend implementation only.

## Scope Split

P15 covers:

- hosted secret binding metadata
- hosted encrypted secret version storage
- hosted resolver integration for worker/runtime
- hosted control API for secret bindings/versions
- CLI compatibility for `aos universe secret ...` and `aos world create --sync-secrets`

P15 does not cover:

- commercial secret manager integration
- HSM/KMS production hardening beyond the first KEK seam
- secret rotation orchestration policy
- secret GC policy beyond basic active/latest semantics

## Reuse From Older Design

The older FDB-backed implementation is useful conceptually in three places:

1. `HostedSecretConfig`
- KEK id
- KEK bytes / provider-derived unwrap material
- optional env fallback stance

2. Envelope encryption model
- generate random DEK per secret version
- encrypt plaintext with DEK
- wrap DEK with KEK
- persist:
  - `ciphertext`
  - `dek_wrapped`
  - `nonce`
  - `enc_alg`
  - `kek_id`

3. Runtime split
- one admin-side “put secret value” service
- one worker/runtime-side `SecretResolver`

What should not be reused directly:

- the assumption of a shared transactional metadata database
- the old persistence trait shape

The storage backend must change to blobstore.

## Shared Contracts Already Exist

The shared protocol already has the right high-level records:

- `SecretBindingRecord`
- `SecretVersionRecord`
- `PutSecretVersionRequest`
- `SecretBindingSourceKind`

And the runtime already has the right seam:

- `SecretResolver`
- `ResolvedSecret`

P15 should reuse those rather than inventing another secret model.

## Hosted Secret Model

## 1) Binding Metadata

Bindings are universe-scoped metadata records.

Each binding record should contain at least:

- `binding_id`
- `source_kind`
- `env_var` for `worker_env`
- `required_placement_pin`
- `latest_version`
- `created_at_ns`
- `updated_at_ns`
- `status`

For the first hosted blobstore-backed vault, bindings should be stored as small metadata objects in
blobstore.

Suggested shape:

- `prefix/secrets/{universe_id}/bindings/{binding_id}.cbor`

## 2) Version Records

Each secret version is an encrypted record.

Persist:

- `version`
- `digest`
- `ciphertext`
- `dek_wrapped`
- `nonce`
- `enc_alg`
- `kek_id`
- `created_at_ns`
- `created_by`
- `status`

Suggested shape:

- `prefix/secrets/{universe_id}/bindings/{binding_id}/versions/{version}.cbor`

Optionally maintain a small “latest pointer” convenience object if needed, but it is not required
if `latest_version` already lives on the binding record.

## 3) Source Kinds

The current shared source kinds are enough for the first cut:

- `node_secret_store`
- `worker_env`

### `node_secret_store`

Meaning:

- binding resolves through the hosted vault
- ciphertext/version metadata lives in blobstore
- worker unwraps and decrypts at resolution time

### `worker_env`

Meaning:

- binding resolves from a named worker environment variable
- value is never persisted in blobstore

This gives a useful hybrid model:

- blobstore-backed hosted vault for general hosted secrets
- worker env path for a few special operator-managed cases

## Encryption Model

The first implementation should use the envelope model from the older code.

Recommended first cut:

- random 32-byte DEK per secret version
- AES-256-GCM-SIV for data encryption
- AES-256-GCM-SIV for KEK wrapping in dev/test mode
- combined nonces for:
  - data encryption
  - DEK wrapping

Persist only encrypted fields.

Do not persist plaintext or derived plaintext caches.

## KEK Model

The hosted secret system needs a KEK seam now, even if the first implementation is simple.

Suggested config shape:

- `AOS_HOSTED_KEK_ID`
- `AOS_HOSTED_KEK_HEX` for dev/test only

Local development stance:

- when running `aos-node-hosted` locally for development/testing, the vault should use a built-in
  default dev KEK if no KEK env vars are configured
- developers should not need to provision or manage a KEK just to run the hosted node locally
- this default should be clearly marked as unsafe/dev-only
- production/staging deployment paths should require explicit KEK configuration or a managed KMS
  backend

First-cut stance:

- local/dev/test hosted deployments may use a configured dev KEK from env
- local/dev/test hosted deployments may also fall back to a built-in default dev KEK for
  convenience
- production should later replace this with KMS/HSM-backed unwrap logic

Important:

- `unsafe-dev` style default KEK is acceptable only for explicit local/test mode
- it must not silently become the production stance

## Runtime Resolution Model

Workers should resolve secrets through the existing `SecretResolver` seam.

Resolution flow for `node_secret_store`:

1. read binding metadata from blobstore
2. verify binding is active and version exists
3. read version record from blobstore
4. unwrap DEK with configured KEK
5. decrypt ciphertext
6. verify digest against expected digest if present
7. return `ResolvedSecret`

Resolution flow for `worker_env`:

1. read binding metadata
2. get configured `env_var`
3. resolve from worker process env
4. verify digest if present
5. return `ResolvedSecret`

Workers should not directly know about control APIs. They should talk to a hosted secret-provider
abstraction that hides blobstore record layout.

## Hosted Control API Surface

Hosted needs the shared secret routes expected by the CLI.

Required routes:

- `GET /v1/secrets/bindings`
- `PUT /v1/secrets/bindings/{binding_id}`
- `GET /v1/secrets/bindings/{binding_id}`
- `DELETE /v1/secrets/bindings/{binding_id}`
- `POST /v1/secrets/bindings/{binding_id}/versions`
- `GET /v1/secrets/bindings/{binding_id}/versions`
- `GET /v1/secrets/bindings/{binding_id}/versions/{version}`

And the hosted `/v1/universes/{universe}/...` wrappers expected by the remote CLI flow.

These routes should match the shared `aos-node` control DTOs.

## `--sync-secrets` Outcome

After P15, the following should work against hosted:

1. `aos universe create --select`
2. `aos world create --local-root worlds/demiurge --sync-secrets --select`
3. CLI loads values from local `aos.sync.json`
4. CLI uploads:
   - binding metadata
   - encrypted secret version records
5. hosted worker later resolves those bindings when the world needs them

This is the concrete product goal for Demiurge-on-hosted.

## Persistence/Layout Considerations

Blobstore-only means there is no query DB for secrets.

So the storage layout must support:

- direct binding lookup by `binding_id`
- direct version lookup by `binding_id + version`
- easy update of `latest_version`

That is enough for the first cut.

No list-optimized indexing beyond straightforward prefix listing is required initially.

## What Must Not Happen

Do not:

- store plaintext in blobstore
- store ciphertext in world CAS
- embed secret values in manifests
- journal secret values or ciphertext as world records
- materialize secret values into SQLite projections
- let workers cache decrypted values durably on disk by default

At most, decrypted values may exist:

- in process memory
- for the duration of runtime resolution/use

## Optional Fallbacks

Two optional behaviors may be supported, but should be explicit:

1. `env:` fallback for dev/test only
- useful for compatibility
- should not be the primary hosted model

2. small in-memory secret cache in workers
- reduce repeated blobstore/KMS round-trips
- bounded, process-local, non-durable

Neither should change the durable model.

## Separation From P14

P14 covers:

- universe/world lifecycle/admin surface

P15 covers:

- hosted secret binding/version surface
- hosted secret runtime resolution

P15 depends on P14’s remote universe/admin path being present, but it is still a separate feature
area and should remain separately trackable.

## DoD

1. Hosted exposes the shared secret binding/version API surface.
2. Hosted stores binding metadata and encrypted secret version records in blobstore.
3. Hosted secret plaintext is never persisted.
4. Hosted workers can resolve `node_secret_store` bindings through the runtime `SecretResolver`
   seam.
5. Hosted workers can resolve `worker_env` bindings without persisted values.
6. `aos universe secret ...` works against hosted.
7. `aos world create --sync-secrets` works against hosted.
8. An integration test proves end-to-end:
   - create universe
   - upload binding/value
   - create secretful world
   - resolve secret at runtime

## Follow-On

Later work can add:

- commercial secret manager backend
- KMS/HSM KEK unwrap
- better rotation workflows
- audit/reporting improvements
- stronger placement-aware secret policy

But the first blobstore-backed hosted vault should land before those.
