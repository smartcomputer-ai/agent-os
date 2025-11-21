# Secrets Architecture (v1)

## Goals
- Keep reducers deterministic and keep plaintext out of journals/snapshots.
- Make secret usage auditable via aliases and versions; values never appear in logs or receipts.
- Support injection of secrets into effects by default; allow reducer read access only when explicitly granted.
- Enable rotation of secrets and the master key without changing reducers/plans.
- Preserve offline determinism for shadow and replay, with clear handling of ephemeral inputs.

## Core primitives
- **Alias**: logical name unique per manifest (e.g. `payments/stripe`).
- **Version**: monotonically increasing integer for a given alias. All references must pin a version to stay deterministic.
- **SecretRef**: `{ alias: string, version: int, scope: "persistent" | "ephemeral" }`. Used anywhere an auth value is accepted.
- **StorageKind** (per alias):
  - `hybrid` (recommended): ciphertext blob in CAS, DEK wrapped by external vault/KMS KEK.
  - `cas_encrypted`: ciphertext blob in CAS; KEK provided by node-local config.
  - `vault`: value stored and served by external vault/KMS adapter (no CAS payload).
  - `env`: ephemeral; pulled from env/.env at runtime; non-reproducible unless provided.

## Schema surface (AIR)
- `common.schema.json`
  - Add `SecretRef` and helper unions `TextOrSecretRef`, `BytesOrSecretRef`.
- Effect def schemas (`spec/defs/http.json`, `spec/defs/llm.json`, any auth-bearing defs)
  - Auth fields become `oneOf [string, SecretRef]` (headers, bearer tokens, API keys, provider creds, etc.).
- Manifest (`manifest.schema.json`)
  - New `secrets` array of entries `{ alias, version, scope, storage: { kind, handle, dek_wrapped_with? }, policy: { allowed_caps? } }`.
  - Validation: every `SecretRef` alias/version must exist in `secrets`; caps/plans must be allowlisted by policy.
- Capability types
  - Add `secret.get` micro-effect for reducers and `vault.get/put/rotate` effect kinds for plans/adapters.

## Roles
- **Reducers**: default path uses `SecretRef` in emitted intents; no plaintext. When granted `secret.get`, a reducer may synchronously fetch a pinned alias+version; response is deterministic and redacted from journal (receipt contains alias+hash only).
- **Plans**: orchestration only; see aliases, not values. May call `vault.*` to create/rotate secrets.
- **Executor/Effect manager**: resolves `SecretRef` just before dispatch; injects plaintext into adapter request; scrubs value from receipts/logs.
- **Adapters**: enforce per-field redaction and ensure returned receipts never echo secret bytes.

## Storage and key hierarchy
- Each secret uses a per-secret DEK; payload stored encrypted.
- KEK (master key) lives outside the manifest: OS keyring, KMS, or sealed file configured per node.
- Hybrid flow: ciphertext + wrapped DEK stored in CAS; executor fetches blob, unwraps DEK via vault/KMS KEK, decrypts, injects, and discards plaintext after use.
- CAS-only flow: same as hybrid but unwrap/unwrap happens locally with node KEK (larger blast radius; dev only).
- Vault-only flow: adapter fetches value directly; receipts carry alias+version digest only.

## Access flows
1) **Default (injection into effects)**
   - Reducer emits intent referencing a cap; plan step includes `SecretRef` in auth fields.
   - Executor validates alias/version against manifest, resolves secret, injects, redacts receipts.
2) **Reducer direct access (opt-in)**
   - Reducer declares a slot bound to a `secret.get` cap grant pinned to alias+version.
   - Kernel issues micro-effect `secret.get`, returns bytes to reducer; receipt contains alias+hash; deterministic because version is fixed.
   - Use only when business logic truly needs the value inside the reducer; default remains effect injection.

## Ephemeral secrets (env/.env)
- Manifest `scope: "ephemeral"`, `storage.kind: "env"`, `storage.handle: ENV_VAR_NAME`.
- Executor reads env at runtime; shadow/replay use placeholder `<secret:alias@version>` unless env is supplied.
- Policy should disallow env secrets in production; allowed in dev/test for convenience.

## Rotation
- **Secret rotation**: plan calls `vault.put` (or encrypted `blob.put`) with new plaintext; new version recorded in manifest `secrets`; consumers reference the new version. Old versions remain readable until revoked.
- **Master key rotation**: generate new KEK; rewrap stored DEKs (no payload re-encrypt needed). If KEK is compromised, regenerate DEKs and re-encrypt payloads.

## Audit and redaction
- Receipts and journal entries record alias, version, storage kind, and a digest of ciphertext; never plaintext.
- Effect adapters must redact headers/bodies that carried secrets before receipt emission.

## Shadow/predict/replay behavior
- Secret resolution in shadow uses deterministic placeholders; predicted receipts include alias+version digest only.
- Replay is deterministic because alias+version and ciphertext hashes are stable; vault-backed paths require vault availability or preloaded stubs.

## Operational guidance
- Enforce policies tying aliases to caps/plans; deny at enqueue if alias is not allowlisted.
- Cache decrypted secrets in-memory per step; never persist.
- Fail closed when vault/KMS unavailable unless manifest marks alias as `allow_shadow_stub: true` for offline testing.

