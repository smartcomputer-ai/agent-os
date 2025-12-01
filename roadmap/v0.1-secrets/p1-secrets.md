# Secrets Architecture (v1)

## Goals
- Keep reducers deterministic and keep plaintext out of journals/snapshots.
- Make secret usage auditable via aliases and versions; values never appear in logs or receipts.
- Support injection of secrets into effects by default; no reducer read access in v1.
- Enable rotation of secrets and the master key without changing reducers/plans.
- Preserve offline determinism for shadow and replay, with clear handling of ephemeral inputs.

## Core primitives
- **Alias**: logical name unique per manifest (e.g. `payments/stripe`).
- **Version**: monotonically increasing integer for a given alias. References pin a version for deterministic replay.
- **BindingId**: stable opaque id that maps to a backend at runtime (in node-local resolver config, not in the manifest).
- **SecretRef**: `{ alias: string, version: int }`. Used anywhere an auth value is accepted.
- **Expected digest (optional)**: `Hash` (`sha256:…`) of the *plaintext* secret bytes (canonical form). Used to catch backend drift and keep master-key rotations manifest-stable even if ciphertext/KEK changes.

## Schema surface (AIR)
- `defsecret.schema.json` (defkind)
  - Fields: `name` (`alias@version`), `binding_id`, optional `expected_digest`, optional `allowed_caps`, `allowed_plans`.
- `common.schema.json`
  - Adds effect kinds `vault.put`, `vault.rotate`.
- Built-in schemas (`spec/defs/builtin-schemas.air.json`)
  - Adds `sys/SecretRef@1`, plus ergonomic variants `sys/TextOrSecretRef@1` and `sys/BytesOrSecretRef@1` using normal `variant` types (no special unions).
  - HTTP/LLM and any auth-bearing schemas should use these variants for tokens/keys instead of bespoke fields.
- Manifest (`manifest.schema.json`)
  - `secrets` array entries are `NamedRef` (`{name, hash}`) pointing to `defsecret` nodes stored in CAS.
  - Validation: defsecret `name` must parse to `(alias, version>=1)`; (alias,version) pairs unique; every `SecretRef` in plans/modules/caps resolves to a declared secret; `allowed_caps`/`allowed_plans` names must exist. Secret ACL check runs after normal cap/policy allow/deny.
- **Canonical value form**: SecretRefs in canonical JSON/CBOR are always the variant `{"$tag":"secret","$value":{"alias":<text>,"version":<int>}}`. Authoring sugar like `{"secret": {...}}` must be normalized to that shape before hashing.
- Capability types
  - `vault.put`, `vault.rotate`; executor uses resolver map keyed by `binding_id`.

## Roles
- **Reducers**: use `SecretRef` in emitted intents; no plaintext. Reducers do **not** fetch secret bytes in v1.
- **Plans**: orchestration only; see aliases, not values. May call `vault.*` to create/rotate secrets.
- **Executor/Effect manager**: resolves `SecretRef` just before dispatch; injects plaintext into adapter request; scrubs value from receipts/logs. After standard cap/policy gates, enforce per-secret ACL (`allowed_caps`/`allowed_plans`).
- **Adapters**: enforce per-field redaction and ensure returned receipts never echo secret bytes.

## Storage and resolver model
- Single logical interface; backends are chosen per environment in a **resolver map** (node-local config): `binding_id → { backend, handle, kms_key?/kek?, cache? }`.
- Backends can be env/local file/KMS/vault; the manifest never embeds backend details. Changing backends only touches the resolver config.
- Each secret has a per-secret DEK; ciphertext may live in CAS or inside the backend. KEK/KMS details stay outside the manifest.

## Access flows
1) **Default (injection into effects)**
   - Reducer emits intent referencing a cap; plan step includes `SecretRef` in auth fields.
   - Executor validates alias/version, resolves via resolver map (by binding_id), injects, redacts receipts.

## Ephemeral backends
- Optional; if a resolver maps a binding to `env`, executor reads the env var. Determinism can be enforced by providing `expected_digest`; otherwise shadow uses placeholders. Avoid in production unless explicitly allowed; fail closed on missing resolver.

## Rotation
- **Secret rotation (design-time plan)**: a governance plan calls `vault.put` to store new plaintext, receives `{alias, version, digest}`, and emits a manifest patch that adds/bumps a `defsecret` node plus a manifest `secrets` ref `{name, hash}`. Patch flows through proposal → shadow → approve → apply; runtime plans cannot mutate the manifest.
- **Master key rotation**: backend-specific; rewrap DEKs with new KEK/KMS key. Manifest unaffected unless expected_digest changes.

## Audit and redaction
- Receipts/journal record alias, version, binding_id, and resolved digest; never plaintext. Adapters redact any fields that carried secrets.
- Effect adapters must redact headers/bodies that carried secrets before receipt emission.

## Shadow/predict/replay behavior
- Injection-only paths are fully replayable from journal+receipts; no secret bytes are needed.
- Shadow uses deterministic placeholders unless resolver is available; if expected_digest is set, predictions include it.
- Replay is deterministic when expected_digest is pinned and resolver available.

## Operational guidance
- Enforce policies tying aliases/binding_ids to caps/plans; deny if resolver missing (unless stub-allowed for shadow).
- Cache decrypted secrets per step only; never persist.
- Fail closed on digest mismatch versus expected_digest.
