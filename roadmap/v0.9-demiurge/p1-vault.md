# P1: Vault (Secrets for LLM)

**Priority**: P1  
**Effort**: Medium  
**Risk if deferred**: High (blocks real LLM calls)  
**Status**: Draft

## Goal

Provide a minimal, production-sane secret resolver so `llm.generate` can receive
API keys via `SecretRef` without ever writing plaintext to journals or receipts.

## Non-Goals (v0.9)

- Full vault backends (KMS, HashiCorp Vault, cloud secret managers).
- Secret rotation workflows or `vault.*` effects.
- UI for secret management.

## Decision Summary

1) **Use existing `defsecret`/`SecretRef`** in manifests; plans/reducers never
   see plaintext.
2) **Resolver v0.9 = env-backed only**: `binding_id = "env:FOO"` loads the
   value from process env (and world `.env` if loaded).
3) **Fail closed by default**: if a manifest declares secrets and no resolver
   is configured, the kernel errors unless `allow_placeholder_secrets` is
   explicitly set for local dev.
4) **Secret ACLs enforced**: `allowed_caps` and `allowed_plans` are required
   for LLM secrets used by Demiurge plans.
5) **No plaintext in receipts**: keep adapter receipts and journals strictly
   free of secret values (already enforced by secret injection path).

## Implementation Notes

- Add an env-backed resolver in host/CLI boot:
  - `binding_id` syntax: `env:VAR_NAME` (exact lookup).
  - Load `.env` from world root before boot (CLI already does this in
    `load_world_env`).
  - Construct `MapSecretResolver` from the env vars referenced by secrets in
    the loaded manifest.
- Kernel config:
  - `KernelConfig.secret_resolver` set by host/CLI when secrets exist.
  - `allow_placeholder_secrets` stays default `false` in host, `true` only for
    explicit dev modes.
- Document the `defsecret` pattern for LLM keys, with a sample:
  - `binding_id: "env:LLM_API_KEY"`
  - `allowed_caps: ["cap_llm"]`
  - `allowed_plans: ["demiurge/chat_plan@1"]`

## Tests

- Resolver picks up `env:` bindings and injects into `llm.generate` params.
- Missing env var fails with `SecretResolverMissing` (unless placeholders are allowed).
- Secret ACLs deny plans/caps not on the allowlist.

## Done

- Host/CLI support env-backed resolver with `.env` loading.
- LLM secret can be injected via `SecretRef` end-to-end.
