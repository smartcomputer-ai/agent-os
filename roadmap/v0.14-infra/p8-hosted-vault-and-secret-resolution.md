# P8: Hosted Vault And Secret Resolution

**Priority**: P8  
**Effort**: High  
**Risk if deferred**: Medium-High (the hosted secret substrate is now in place, but `vault.*` runtime effects, tighter secret cap scoping, and metadata-only version responses are still unfinished)  
**Status**: In Progress (Phase 1 shipped; follow-on hardening and `vault.*` work remain)

## Goal

Make secrets first-class in hosted AgentOS so uploaded worlds can run without relying on the local `aos-cli` behavior of injecting values from the creator's process environment.

This milestone should provide a hosting-native answer to two different needs:

1. a universe-shared secret store that travels with the hosted universe and can resolve on any eligible worker,
2. an explicit worker-env path for operator-managed node credentials and compatibility with existing `env:`-style worlds.

## Implementation Status

Status below reflects the repo as of 2026-03-11.

Done:

- kernel secret declarations, normalization, digest checking, policy enforcement, and effect-time secret injection are in place,
- the internal resolver contract is version-aware: `resolve(binding_id, version, expected_digest)`,
- hosted universes have persisted secret bindings and versioned secret records in FDB / memory persistence,
- hosted-vault secret values are envelope-encrypted before persistence and decrypted only in worker memory,
- hosted workers resolve secrets through a universe-aware binding registry with both `hosted_vault` and explicit `worker_env` sources,
- hosted control-plane routes exist for binding CRUD, hosted secret upload, and version inspection,
- hosted world create performs secret preflight and fails early for missing bindings / disabled bindings / missing hosted versions,
- admin-plane secret writes append audit records without storing plaintext in the world journal or manifest.

Still to do:

- `vault.put` and `vault.rotate` are still stubbed runtime effect adapters,
- `sys/secret@1` is still an empty schema and does not yet scope `vault.*` by op / binding,
- secret version GET routes currently expose full stored version records instead of metadata-only responses,
- worker-env usage is implemented, but the operator guidance / docs are still implicit in code and roadmap text rather than called out in runtime docs,
- no workflow-safe secret-upload transport exists yet for implementing hosted `vault.put`.

## Current State

What already exists:

- `defsecret`, `SecretRef`, secret normalization, digest checking, and secret injection in the kernel.
- `sys/secret@1` capability type and builtin `vault.put` / `vault.rotate` effect kinds.
- `allowed_caps`-based secret policy on resolved secret declarations.
- local/env-backed resolver wiring in `aos-cli` and `aos-world`.
- hosted secret binding persistence and hosted-vault secret version persistence.
- hosted envelope encryption with configurable KEK selection and an explicit unsafe dev/test default KEK mode.
- hosted resolver wiring in `aos-fdb-node` for `hosted_vault`, `worker_env`, and optional compatibility fallback.
- hosted control-plane secret CRUD / upload APIs and create-from-manifest secret preflight.

What is still missing or incomplete:

- `vault.put` / `vault.rotate` runtime semantics are not implemented yet; they still return stub error receipts.
- `sys/secret@1` remains too coarse for hosted secret-management effects.
- the secret version inspection API currently returns encrypted storage fields, not just metadata.

The practical result is that hosted worlds no longer depend on the uploader's local env for normal secret resolution, but the roadmap item is not fully closed until the remaining hardening and workflow-level secret-management pieces land.

## Design Stance

- Keep AIR `defsecret`, `SecretRef`, and runtime secret injection semantics intact for v0.20.
- Treat `binding_id` as an opaque logical key in hosted mode.
- Support both hosted-vault and worker-env sources, but make hosted-vault the default and recommended hosted path.
- Do not put plaintext secret material in world journals, manifests, snapshots, receipts, or normal universe CAS.
- Keep secret values outside the deterministic world log; only metadata, digests, aliases, versions, and binding IDs are auditable in-world.
- Reuse the existing kernel secret-injection path, but extend the internal resolver contract to include secret version.
- Keep world-scoped authorization where it already belongs:
  - normal effect execution remains gated by the target effect cap plus `defsecret.policy.allowed_caps`,
  - secret-management operations (`vault.*`) get their own hosted control path and tighter cap constraints.

## Recommendation

Ship both paths, with a strict priority order:

1. **Primary**: universe-scoped hosted vault persisted in FDB and resolved by any hosted worker.
2. **Secondary**: worker-env bindings for operator-managed credentials, legacy compatibility, and cases where secret material must stay off the shared control plane.

Why both:

- hosted-vault is the only portable answer for uploaded worlds and worker mobility,
- worker-env is still useful for node-local platform credentials and simple rollouts,
- the two paths fit naturally under the existing secret-injection path once the internal resolver takes `(binding_id, version, expected_digest)`.

Why hosted-vault should be the default:

- it matches the hosted world portability model from P2/P3/P7,
- it avoids hidden coupling between a world's manifest and a specific worker image,
- it lets create-from-manifest fail early when required secret material is missing,
- it keeps operator intent in the hosted control plane instead of scattered across node env configuration.

## Core Model

### 1) Keep AIR Secret Metadata Stable

No required AIR surface change for the first hosted pass:

- `defsecret` still carries `name`, `binding_id`, `expected_digest`, and `allowed_caps`,
- manifests still reference `defsecret` nodes,
- workflows still emit `SecretRef { alias, version }`,
- the kernel still injects only at effect-dispatch time.

Important hosted interpretation change:

- in hosted mode, `binding_id` is no longer interpreted primarily by string prefix,
- it is first resolved through a universe-scoped hosted binding registry,
- direct `env:` parsing becomes a compatibility fallback, not the main model.

This keeps existing authored worlds valid while letting hosted deployments decouple secret resolution from local process env assumptions.

### 2) Add A Universe-Scoped Secret Binding Registry

Hosted universes need a persisted resolver map:

`binding_id -> secret source configuration`

Suggested record:

```text
SecretBindingRecord {
  binding_id: String,
  source_kind: "hosted_vault" | "worker_env",
  env_var?: String,
  required_placement_pin?: String,
  latest_version?: u64,
  created_at_ns: u64,
  updated_at_ns: u64,
  status: "active" | "disabled"
}
```

Semantics:

- `binding_id` is the exact lookup key referenced by `defsecret`.
- `source_kind = "hosted_vault"` means the secret value is versioned in hosted storage.
- `source_kind = "worker_env"` means the worker resolves it from its process environment.
- `required_placement_pin` is optional but strongly recommended for `worker_env`.
- `latest_version` is metadata only; deterministic replay still comes from `SecretRef(alias, version)` in the world manifest/effect params.

Compatibility rule:

- a hosted binding record may satisfy any authored `binding_id`, including legacy names like `env:OPENAI_API_KEY`.
- this avoids forcing manifest rewrites during upload.

### 3) Add A Universe-Scoped Hosted Vault Backing Store

Hosted-vault secret values should live in mutable universe-scoped storage, not in CAS.

Suggested records:

```text
SecretVersionRecord {
  binding_id: String,
  version: u64,
  digest: HashRef,
  ciphertext: bytes,
  dek_wrapped: bytes,
  nonce: bytes,
  enc_alg: String,
  kek_id: String,
  created_at_ns: u64,
  created_by?: String,
  status: "active" | "superseded" | "disabled"
}
```

Key properties:

- versions are immutable once written,
- version numbers are monotonic per `binding_id`,
- stored material is ciphertext, not plaintext,
- ciphertext is encrypted before it reaches FDB using a fresh per-secret-version DEK,
- the DEK is stored only in wrapped form,
- the worker only decrypts into memory during effect dispatch.

Storage stance:

- secrets are expected to be small, so inline encrypted bytes in FDB are acceptable for v1,
- do not store secret plaintext in CAS and do not store secret ciphertext in shared immutable CAS unless there is a strong later reason,
- record `kek_id` so envelope-encryption key rotation stays operationally possible.

### 3a) Encryption Key Management

Hosted-vault records should use envelope encryption.

Model:

1. generate a fresh DEK for each secret version,
2. encrypt the plaintext secret with that DEK,
3. wrap the DEK with a KEK,
4. store only ciphertext, wrapped DEK, nonce, algorithm metadata, digest, and `kek_id` in FDB.

Required rule:

- KEKs do not live in FDB, CAS, manifests, journals, or world snapshots.

Operational modes:

- **Production/hosted-safe mode (v1)**:
  - KEK material comes from `aos-fdb-node` config,
  - `kek_id` selects the active KEK,
  - workers unwrap DEKs only when they need to inject a secret into an effect.
- **Later hardened mode (deferred)**:
  - KEK material may come from an external KMS/HSM,
  - KMS-backed wrap/unwrap is explicitly deferred beyond the first production version.
- **Development/test mode**:
  - allow a deliberately insecure fixed KEK for local hosted development and tests,
  - this KEK may be hardcoded or derived from a stable built-in constant,
  - it must be clearly labeled unsafe and must not be the default in production configs.

Why this split is acceptable:

- it preserves one implementation path for encryption/decryption across dev and prod,
- it avoids making the first production rollout or local development depend on KMS setup,
- it keeps the dev/test convenience mode explicit instead of silently weakening production deployments.

Key-rotation semantics:

- rotating a secret creates a new secret version with a new DEK,
- rotating a KEK only requires re-wrapping stored DEKs and updating `kek_id`,
- manifest `expected_digest` values remain stable across KEK rotation because they describe plaintext, not ciphertext.

### 4) Extend The Internal Resolver Contract

The current resolver signature is:

```text
resolve(binding_id, expected_digest)
```

That works for env-style single-value backends, but it is not sufficient for a versioned hosted vault.

Hosted-vault resolution needs:

```text
resolve(binding_id, version, expected_digest)
```

Reason:

- the manifest pins a specific `alias@version`,
- the kernel already knows that version when it looks up the `SecretDecl`,
- hosted storage should be keyed by `(binding_id, version)`,
- relying on `expected_digest` alone to identify the version would make hosted semantics depend on an optional field.

This is an internal runtime API change, not an AIR schema change.

## Hosted Resolver Behavior

Hosted runtime should use a composite universe-aware resolver:

1. look up `binding_id` in the universe secret binding registry,
2. dispatch by `source_kind`,
3. verify `expected_digest` if present,
4. inject plaintext into adapter params only in worker memory.

Resolution rules:

- `hosted_vault`:
  - resolve `(binding_id, version)` from hosted secret storage,
  - decrypt ciphertext,
  - verify digest against the manifest's `expected_digest` when provided.
- `worker_env`:
  - read `env_var` from the worker process,
  - compute digest and verify `expected_digest` when provided.
- compatibility fallback:
  - if no binding record exists and hosted compatibility mode is enabled, `binding_id` beginning with `env:` may still read directly from env,
  - this fallback should be disabled by default once the hosted binding registry exists.

Resolver call shape:

- the kernel should call `resolve(binding_id, version, expected_digest)`,
- hosted mode then resolves against universe-scoped binding metadata instead of purely process-local state.

## Control Plane Surface

P5 already established the hosted HTTP control plane pattern. Secrets should be managed there, not through ad hoc worker flags.

Suggested routes:

```text
GET    /v1/universes/{universe_id}/secrets/bindings
PUT    /v1/universes/{universe_id}/secrets/bindings/{binding_id}
GET    /v1/universes/{universe_id}/secrets/bindings/{binding_id}
DELETE /v1/universes/{universe_id}/secrets/bindings/{binding_id}

POST   /v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions
GET    /v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions
GET    /v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions/{version}
```

Route semantics:

- `PUT secrets/bindings/{binding_id}` creates or updates the binding source metadata.
- `POST secrets/bindings/{binding_id}/versions` uploads new secret material for `hosted_vault` and allocates the next version.
- plaintext is not returned by control-plane GET routes.
- current implementation note: version GET routes still return full encrypted storage records (`ciphertext`, wrapped DEK, nonce, etc.); this should be narrowed to metadata-only responses before this milestone is considered fully complete.
- delete/disable should be metadata-only at first; physical GC is follow-on work.

Upload contract for `POST .../versions`:

- request body should carry the plaintext secret bytes directly over TLS,
- request metadata should carry `expected_digest`,
- the control plane encrypts before persistence using the configured KEK mode,
- the response returns `{ binding_id, version, digest }`.

This is intentionally not normal CAS upload.

## Why Normal CAS Must Not Carry Secret Plaintext

The current `vault.put` params use `value_ref: HashRef`. That is acceptable only if the referenced bytes live in a protected secret-staging channel, not normal universe CAS.

Using ordinary CAS for secret plaintext would be wrong because:

- CAS is immutable and content-addressed,
- CAS is designed for broad reuse and debugability,
- secret plaintext would become durable outside the secret-control boundary,
- later deletion or redaction would be much harder.

Therefore:

- v0.20 hosted bootstrap should use the secret admin API above for secret provisioning,
- do not implement hosted `vault.put` by pointing `value_ref` at normal CAS,
- if we later want workflow-driven `vault.put`, we should add a dedicated secret-upload handle or protected staging store rather than reusing ordinary `HashRef`.

## World Create / Upload Semantics

Hosted create-from-manifest should become secret-aware.

For each referenced secret declaration in the uploaded manifest:

1. resolve the `binding_id` through the universe secret binding registry,
2. verify the source exists and is `active`,
3. for `hosted_vault`, verify the referenced version is present when the manifest pins one,
4. return a typed create error if required bindings or versions are missing.

Suggested failure classes:

- `secret_binding_missing`
- `secret_binding_disabled`
- `secret_version_missing`
- `secret_backend_unavailable`

Recommended CLI upload flow:

1. load manifest and referenced `defsecret` nodes locally,
2. for each required binding:
   - either provision/update a hosted-vault binding through the secret API,
   - or confirm the universe binding already exists,
   - or intentionally configure a `worker_env` binding,
3. upload manifest/modules/workspaces,
4. call hosted world create,
5. let create fail early if secret prerequisites are missing.

This makes secret readiness part of hosted bootstrap instead of a late worker surprise.

## Worker-Env Path

Worker-env support remains valuable, but it must be treated as an explicit operator feature, not the default hosted story.

Recommended semantics:

- `worker_env` bindings are configured in the universe binding registry, not inferred only from `env:` prefixes,
- the binding record names the worker env var to read,
- worlds using such bindings should also be constrained by `placement_pin` to a compatible worker pool,
- failure to satisfy the env binding should surface as a hosted runtime error with redacted metadata only.

Operational stance:

- do not try to make `worker_env` universally portable,
- use it for credentials that are intentionally managed per node pool,
- prefer hosted-vault for tenant/world credentials and uploaded-app secrets.

Future refinement:

- workers may later advertise available secret bindings in heartbeat metadata,
- scheduling may then use secret-binding availability as another eligibility filter,
- that is not required for the first pass if `placement_pin` remains the primary control.

## `vault.put` / `vault.rotate` Semantics

The builtins should remain, but the hosted implementation should be staged carefully.

### Phase 1: Admin-plane secret provisioning

First ship:

- universe secret binding CRUD,
- hosted-vault version writes through the control plane,
- runtime secret resolution from hosted-vault and worker-env.

This solves the immediate hosted-world portability problem.

### Phase 2: Implement hosted `vault.put`

`vault.put` should write through the same hosted binding/version store, but only after a safe secret-upload transport exists.

Required behavior:

- allocate next version for `binding_id`,
- verify digest,
- persist encrypted bytes,
- return `{ alias, version, binding_id, digest }`,
- never expose plaintext in params, logs, receipts, or journal.

### Phase 3: Clarify or narrow `vault.rotate`

Current `vault.rotate` params are metadata-only:

```text
{ alias, version, binding_id, expected_digest }
```

That shape is reasonable only for adopting or validating a version that already exists in the backend.

Hosted interpretation should be:

- `vault.rotate` does not upload plaintext,
- it validates that `(binding_id, version)` already exists and matches the provided digest,
- it returns a receipt suitable for governance-driven manifest advancement.

If later experience shows this is not useful enough, `vault.rotate` should be revised explicitly rather than overloaded.

## Capability And Policy Model

The current `sys/secret@1` cap schema is empty. That is too coarse for hosted secret-management effects.

Recommended change:

- keep secret injection authorization where it is today:
  - target effect cap must allow the effect,
  - `defsecret.policy.allowed_caps` must allow the cap grant using that secret.
- tighten `sys/secret@1` for `vault.*` operations with optional scope fields such as:

```text
{
  ops?: set<"put" | "rotate">,
  binding_ids?: set<text>,
  binding_prefixes?: set<text>
}
```

Reason:

- `vault.put` / `vault.rotate` are privileged secret-management operations,
- they should not be granted as a single all-secrets wildcard by default,
- hosted secret administration also needs a clean audit boundary separate from normal effect execution.

## Audit, Replay, And Shadow

Replay and shadow rules should stay aligned with the current architecture:

- world journal, receipts, and snapshots continue to record only alias/version/binding/digest metadata,
- hosted secret values remain outside replay state,
- shadow runs may continue to use placeholders when real secret resolution is unavailable,
- governance shadow should not require plaintext secret access.

Additional audit requirement:

- hosted control-plane secret writes should produce durable admin audit records,
- those records should never contain plaintext,
- they should capture actor, binding_id, version, digest, and timestamp.

## FDB Persistence Shape

Suggested hosted keyspace additions:

```text
u/<u>/secret_bindings/<binding_id> -> SecretBindingRecord
u/<u>/secret_versions/<binding_id>/<version> -> SecretVersionRecord
u/<u>/secret_audit/<ts>/<binding_id>/<version> -> SecretAuditRecord
```

Rules:

- `secret_bindings` is mutable metadata,
- `secret_versions` is append-only per binding,
- version numbers are monotonic,
- deletes should initially disable bindings rather than physically erase versions.

## Rollout Plan

### 1) Hosted resolver and persistence

Status: Done.

- add FDB persistence for secret bindings and versioned hosted-vault values,
- add envelope encryption support with KEK selection and wrapped DEKs,
- add hosted composite resolver in `aos-fdb-node`,
- keep local env-resolver behavior unchanged.

### 2) Hosted control-plane API

Status: Mostly done.

- add universe secret-binding CRUD,
- add hosted secret version upload route,
- add create-from-manifest preflight for referenced bindings.

Remaining hardening:

- narrow version GET responses to metadata-only shapes.

### 3) Capability tightening

Status: Not done.

- extend `sys/secret@1` schema,
- enforce binding/operation scoping for `vault.*`.

### 4) Workflow-level vault effects

Status: Not done.

- implement `vault.put` only after there is a safe secret-upload transport,
- implement `vault.rotate` as adopt/validate-existing-version.

## Exit Criteria

P8 is complete only when all of the following are true:

- [x] hosted worlds can resolve declared secrets without relying on the uploader's local process env,
- [x] hosted workers resolve secrets through a universe-aware binding registry,
- [x] universe-shared hosted-vault storage exists in FDB and stores encrypted secret material by version,
- [x] hosted-vault storage uses envelope encryption with explicit KEK management and a clearly unsafe dev/test KEK mode,
- [~] worker-env bindings are supported explicitly; operator-facing documentation is still light and should be tightened,
- [x] hosted world create/upload fails early for missing required secret bindings,
- [~] no hosted secret plaintext is written to world journal, receipts, snapshots, manifest, or ordinary CAS; plaintext is avoided, but version GET responses should still be narrowed to metadata-only shapes,
- [x] the repo has a coherent implementation plan for `vault.put` and `vault.rotate` that does not rely on ordinary CAS plaintext blobs,
- [ ] `sys/secret@1` scopes `vault.*` operations by op / binding,
- [ ] hosted `vault.put` and `vault.rotate` semantics are implemented or explicitly deferred out of this milestone with the roadmap status updated accordingly.
