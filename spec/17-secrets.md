# Secrets Architecture (v1)

## Goals
- Keep reducers deterministic and keep plaintext out of journals/snapshots.
- Make secret usage auditable via aliases and versions; values never appear in logs or receipts.
- Support injection of secrets into effects by default; allow reducer read access only when explicitly granted.
- Enable rotation of secrets and the master key without changing reducers/plans.
- Preserve offline determinism for shadow and replay, with clear handling of ephemeral inputs.

## Core primitives
- **Alias**: logical name unique per manifest (e.g. `payments/stripe`).
- **Version**: monotonically increasing integer for a given alias. References pin a version for deterministic replay.
- **BindingId**: stable opaque id that maps to a backend at runtime (in node-local resolver config, not in the manifest).
- **SecretRef**: `{ alias: string, version: int }`. Used anywhere an auth value is accepted.
- **Expected digest (optional)**: manifest may pin a ciphertext/value digest to catch drift when a resolver backend changes.

## Schema surface (AIR)
- `common.schema.json`
  - Add `SecretRef` and helper unions `TextOrSecretRef`, `BytesOrSecretRef`.
- Effect def schemas (`spec/defs/http.json`, `spec/defs/llm.json`, any auth-bearing defs)
  - Auth fields accept `SecretRef` in addition to literal strings.
- Manifest (`manifest.schema.json`)
  - `secrets` array entries: `{ alias, version, binding_id, expected_digest?, policy: { allowed_caps?, allowed_plans? } }`.
  - Validation: every `SecretRef` must resolve to a declared alias/version; caps/plans must be allowlisted by policy.
- Capability types
  - `secret.get` micro-effect for reducers, `vault.get/put/rotate` for plans/adapters; executor uses resolver map keyed by `binding_id`.

## Roles
- **Reducers**: default path uses `SecretRef` in emitted intents; no plaintext. When granted `secret.get`, a reducer may synchronously fetch a pinned alias+version; response is deterministic and redacted from journal (receipt contains alias+hash only).
- **Plans**: orchestration only; see aliases, not values. May call `vault.*` to create/rotate secrets.
- **Executor/Effect manager**: resolves `SecretRef` just before dispatch; injects plaintext into adapter request; scrubs value from receipts/logs.
- **Adapters**: enforce per-field redaction and ensure returned receipts never echo secret bytes.

## Storage and resolver model
- Single logical interface; backends are chosen per environment in a **resolver map** (node-local config): `binding_id → { backend, handle, kms_key?/kek?, cache? }`.
- Backends can be env/local file/KMS/vault; the manifest never embeds backend details. Changing backends only touches the resolver config.
- Each secret has a per-secret DEK; ciphertext may live in CAS or inside the backend. KEK/KMS details stay outside the manifest.

## Access flows
1) **Default (injection into effects)**
   - Reducer emits intent referencing a cap; plan step includes `SecretRef` in auth fields.
   - Executor validates alias/version, resolves via resolver map (by binding_id), injects, redacts receipts.
2) **Reducer direct access (opt-in)**
   - Reducer slot bound to a `secret.get` grant pinned to alias+version.
   - Kernel issues `secret.get`, uses resolver backend, returns bytes; receipt contains alias+digest.

## Ephemeral backends
- Optional; if a resolver maps a binding to `env`, executor reads the env var. Determinism can be enforced by providing `expected_digest`; otherwise shadow uses placeholders. Avoid in production unless explicitly allowed.

## Rotation
- **Secret rotation**: plan calls `vault.put` to write new plaintext; new version + optional expected_digest recorded in manifest. Resolver backend choice need not change.
- **Master key rotation**: backend-specific; rewrap DEKs with new KEK/KMS key. Manifest unaffected unless expected_digest changes.

## Audit and redaction
- Receipts/journal record alias, version, binding_id, and resolved digest; never plaintext.
- Effect adapters must redact headers/bodies that carried secrets before receipt emission.

## Shadow/predict/replay behavior
- Shadow uses deterministic placeholders unless resolver is available; if expected_digest is set, predictions include it.
- Replay is deterministic when expected_digest is pinned; otherwise requires the same resolver/backends to be present.

## Operational guidance
- Enforce policies tying aliases/binding_ids to caps/plans; deny if resolver missing (unless stub-allowed for shadow).
- Cache decrypted secrets per step only; never persist.
- Fail closed on digest mismatch versus expected_digest.
