# P9: Local Secrets Env-Only Cutover

**Priority**: P9  
**Effort**: Medium  
**Risk if deferred**: Medium-High (the local Demiurge flow stays broken and local secret semantics remain half-hosted, half-local)  
**Status**: Completed

## Goal

Make local secret handling match the intended local product model:

- local secrets come only from `.env` files and process environment variables
- local secrets are never stored in the world
- local secrets are never stored in the local node backend
- local does not use hosted-style secret binding/value/version APIs

This is a local product/runtime cutover item, not a vault feature.

## Completed Outcome

P9 is now complete for the intended local scope:

- local secretful worlds load without hosted secret APIs
- local secret resolution is derived from env, `.env`, and local `aos.sync.json` mapping only
- local `--sync-secrets` is a compatibility no-op rather than a hosted secret write path
- local secret HTTP CRUD remains unsupported
- local Demiurge works again on the local node from env/`.env`

The implementation keeps secret values in-memory only. They are not persisted into the world,
SQLite runtime metadata, CAS, or snapshots.

## Primary Stance

For local mode:

- declared secret bindings remain part of the manifest contract
- secret values come from local process context only
- the resolver is ephemeral and in-memory
- local secret CRUD APIs should remain unsupported
- local `--sync-secrets` should not mean "copy secrets into the node"

The local node should treat secret values as runtime configuration, not persisted world state.

## Why This Is Needed

The current codebase is in an inconsistent middle state:

1. Local embedded control intentionally does not implement secret bindings/values/versions.
2. Local authoring code already knows how to resolve required bindings from `aos.sync.json`,
   `.env`, and `env`.
3. Local `aos world create --local-root ... --sync-secrets` still routes through hosted secret
   sync behavior.
4. The runtime auto-wires env secrets only for `binding_id = env:VAR_NAME`, which does not match
   current Demiurge bindings like `llm/openai_api`.
5. The kernel currently rejects any secretful manifest that does not arrive with a resolver, even
   when the local run only needs a subset of declared bindings.

That means the intended local Demiurge flow is still broken even though the desired secret stance
is already clear.

## Desired Local Secret Model

### Source of truth

Local secret values come from:

1. process environment
2. local `.env`
3. optional local `aos.sync.json` secret mapping that maps manifest bindings to env or dotenv keys

Important:

- `aos.sync.json` is local configuration metadata, not a persisted secret store
- the local node may read it to construct a resolver
- the local node must not copy its values into SQLite, CAS, snapshots, or world state

### Resolver lifetime

The local node should construct a local secret resolver when a world is created, loaded, or
reopened from a local root.

That resolver should:

- validate the local secret config shape early
- snapshot the available env/dotenv values at load time
- hold those values only in memory
- be refreshed only when the world is recreated/reloaded/restarted

The local runtime should not re-read process env or `.env` on every effect dispatch.

### When missing values should fail

Missing secret values should generally fail at effect-time injection, not world-load time.

Reason:

- a local world may declare multiple alternative bindings
- only a subset may be needed in a given run
- Demiurge is the immediate example: OpenAI and Anthropic bindings may both be declared, but only
  one provider may actually be used

So the local resolver should distinguish:

- early config errors
  - malformed `aos.sync.json`
  - duplicate bindings
  - unknown secret sources
  - invalid source kinds
  - unreadable `.env`
- runtime missing binding values
  - fail only when a specific effect actually tries to materialize the secret

## Explicit Non-Goals

- build a local secret vault
- persist local secret values in SQLite
- persist local secret values in CAS
- add local secret version history
- make local secrets look like hosted universe secret storage
- require all declared local secrets to exist before a world can boot

## Current Gaps To Close

### 1. Local CLI create/patch semantics

Current local `world create --local-root ... --sync-secrets` and `world patch --sync-secrets`
still perform hosted universe secret sync.

That must change.

Required direction:

- local `--sync-secrets` becomes a no-op compatibility flag, or
- local `--sync-secrets` is removed from docs and later deprecated, or
- local `--sync-secrets` becomes a local validation/refresh flag only

But it must no longer attempt to write secret bindings or values through node APIs.

### 2. Local world-load resolver injection

The embedded local runtime must gain a way to attach a local env/dotenv-backed secret resolver
when opening or creating a world from a local root.

Required behavior:

- local node create/load/reopen paths can derive a resolver from local-root config
- that resolver is passed into `KernelConfig.secret_resolver`
- local secretful manifests stop failing purely because hosted-style secret sync was skipped

### 3. Runtime support for non-`env:` local bindings

The existing runtime helper that auto-loads env secrets only understands `env:VAR_NAME`.

That is too narrow for the local authored-world model, where bindings like `llm/openai_api` map to
env or dotenv via local config.

Required direction:

- keep `env:VAR_NAME` support
- add a local-root-aware resolver path for authored local worlds
- do not require changing all local manifests to `env:*` bindings just to make local work again

### 4. Demiurge local config/docs/scripts

The local Demiurge docs and scripts still describe hosted-shaped secret sync behavior.

They must be updated so local usage is:

- set env vars or place them in `worlds/demiurge/.env`
- start local node
- create/select local world
- run task

No `aos universe secret binding set ...` should be part of the normal local Demiurge flow.

## Proposed Implementation Shape

### Local resolver construction

Introduce a local secret resolver assembly path that takes:

- local world root
- optional sync map path
- current process env snapshot

and produces:

- validated binding-to-value map
- in-memory `MapSecretResolver`

This should reuse the existing authored-world sync/secret resolution code as much as possible.

### Load-time validation

At local world create/load time:

1. parse local secret config
2. validate source declarations and binding mapping shape
3. attempt to load `.env` and env-backed sources
4. build an in-memory resolver containing all values that are presently available

This stage should not fail merely because one declared binding is absent, unless the config itself
is malformed.

### Effect-time enforcement

At secret injection time:

- if the required binding exists in the in-memory resolver, inject it
- if it does not, fail the effect clearly

Expected error quality:

- mention the missing binding id
- mention that local secrets come from env/`.env`
- avoid implying that the user should create hosted secret bindings

### Local `--sync-secrets`

Short-term acceptable options:

1. no-op on local targets
2. local-only "validate and refresh resolver inputs" behavior

Preferred short-term choice:

- treat it as a no-op compatibility flag for local
- remove it from local docs immediately

That preserves compatibility while making the model clear.

## CLI And Product Surface Consequences

### CLI messaging

Local warning/error text should stop saying:

- "sync hosted universe secrets"
- "bind them manually with `aos universe secret binding set ...`"

and instead say:

- set the required env var
- or add it to the local `.env`
- or update local `aos.sync.json`

### HTTP/control surface

Local secret endpoints should remain unsupported.

That is consistent with the intended architecture:

- local secret values are runtime-only process configuration
- not a persisted control-plane resource

### Local reload behavior

If a local user changes `.env` while the node is running, the values should not silently change in
the current world runtime.

Expected model:

- restart or recreate to pick up changed env/`.env`

That keeps local behavior deterministic enough and avoids hidden mid-run config mutation.

## Completed Implementation

### Phase 1

Completed.

Local `world create` / `world patch` no longer route local `--sync-secrets` through hosted secret
sync behavior.

### Phase 2

Completed.

The embedded local runtime now injects a local-root-aware in-memory resolver on create/load/reopen
for secretful manifests.

### Phase 3

Completed.

Local CLI messaging and behavior now describe env/`.env` semantics instead of hosted secret
binding/value APIs.

### Phase 4

Completed.

Demiurge docs and local smoke/task scripts now use the env/`.env` model directly.

### Phase 5

Completed for current scope.

`--sync-secrets` remains accepted on local targets as a compatibility no-op.

## Acceptance Criteria

This P item is complete when all of the following are true:

1. A local world with declared secrets can be created without hosted secret APIs.
2. Local secret values are never written to local SQLite, CAS, snapshots, or world state.
3. Local Demiurge works again from env/`.env` only.
4. Local mock Demiurge can start even when live-provider secrets are absent.
5. If a live provider is selected and the needed binding is missing, the failure happens at effect
   time with a local-specific error.
6. Local docs and smoke scripts no longer mention hosted secret binding commands.
7. Local secret HTTP CRUD remains unsupported.

Status:

- `1` completed
- `2` completed
- `3` completed
- `4` completed
- `5` completed by design in the resolver/effect path and no longer blocked at world load
- `6` completed
- `7` completed

## Could Be Added Later

- explicit local "reload secrets" command that rebuilds the in-memory resolver without pretending
  secrets are persisted resources
- richer local diagnostics showing which bindings are configured, without exposing secret values
- a future hosted/local shared secret-provider abstraction, as long as local still avoids storing
  secret material in world/backend state
