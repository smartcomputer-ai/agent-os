# P3: Crate Refactor (World Engine, Adapters, Hosted Worker)

**Priority**: P3  
**Effort**: High  
**Risk if deferred**: High (`aos-worker` will either duplicate too much logic or be forced through the wrong local-first crate boundaries)  
**Status**: Proposed

## Goal

Refactor the current `aos-host` area into a small set of responsibility-driven crates that:

1. Preserve existing local/test/dev ergonomics.
2. Make hosted worker implementation possible without dragging in local daemon assumptions.
3. Keep shared logic at the correct boundary: single-world execution and adapters.
4. Avoid exploding the workspace into many tiny crates.

Primary outcomes:

1. `aos-world` becomes the descriptive reusable single-world engine crate.
2. `aos-adapters` owns the adapter registry and built-in adapters.
3. `aos-worker` owns the hosted/FDB worker runtime.
4. `aos-cli`, `aos-smoke`, and `aos-agent-eval` continue to use the single-world engine path without depending on hosted worker machinery.

## Design Constraints

- End-state target is three primary crates in this area: `aos-world`, `aos-adapters`, `aos-worker`.
- A temporary `aos-host` compatibility facade is acceptable during migration, but it must stop accumulating new responsibilities.
- Do not introduce separate permanent crates for `control`, `http`, `testkit`, `runtime-core`, or `local-daemon`.
- Share the single-world engine, not the entire outer runtime loop.
- Hosted worker orchestration is allowed to stay FDB-shaped in v1.
- Local/filesystem runtime behavior is not required to share the same outer process model as hosted.

## Why This Refactor Is Needed

`aos-host` currently mixes four concerns that want different futures:

1. Single-world execution (`WorldHost`, hosted/local open paths, snapshot/replay-facing helpers).
2. Built-in adapters and adapter registry.
3. Local daemon/control/http process concerns.
4. Test/dev harnesses used heavily by smoke tests and evals.

This shape is acceptable for local development, but it is the wrong boundary for hosted worker work.

Current consumers reinforce that split:

- `aos-smoke` and `aos-agent-eval` primarily want `WorldHost`, `TestHost`, manifest loading, and a few test/dev utilities.
- `aos-cli` wants both the single-world engine and the current local daemon/control/http path.
- `aos-worker` will want the single-world engine and adapters, but should not inherit a local daemon/webserver model.

## Non-Goals

- A big-bang rename-and-rewrite of every consumer in one step.
- A fully unified hosted/local runtime architecture.
- A permanent compatibility commitment to `aos-host` naming.
- A new public API surface beyond what is needed to define the new crate boundaries.

## Recommended End State

### 1) `aos-world`

Purpose:

- Reusable single-world execution engine.
- Works for local/filesystem flows, tests, smoke harnesses, and hosted worker inner execution.
- Becomes the descriptive replacement for the reusable parts of today’s `aos-host`.

Owns:

- `WorldHost` or renamed equivalent (`WorldEngine` is a reasonable later rename, but not required in this refactor).
- Local/open-hosted world boot paths.
- Manifest loading needed by the engine path.
- Hosted journal/store bridge for execution-by-persistence-identity.
- Trace/query helpers that operate at single-world scope.
- Test harnesses (`TestHost`, ideally renamed later to something like `WorldHarness`).
- Batch/local helper wrappers only if they remain genuinely useful for tests and CLI fallback.

Must not own:

- Hosted multi-world worker orchestration.
- Hosted worker heartbeats/leases/claiming loops.
- Shared public ingress server for hosted mode.

### 2) `aos-adapters`

Purpose:

- Adapter registry and built-in adapter implementations.
- Reusable by both `aos-world` and `aos-worker`.

Owns:

- Adapter traits.
- Adapter registry and routing helpers.
- Built-in adapters (`http`, `llm`, `blob_get`, `blob_put`, host/fs/session adapters, mocks, stubs).
- Adapter-specific config types.

Notes:

- `adapters/timer.rs` is not really a general external adapter; it is local daemon scheduling glue.
- Keep timer scheduler logic out of `aos-adapters` unless it is refactored into a genuinely reusable abstraction.

### 3) `aos-worker`

Purpose:

- Hosted/FDB worker process.
- Owns the outer distributed runtime loop.

Owns:

- Worker membership heartbeat.
- Lease renew/acquire/release.
- Ready hint scans and collaborative claiming.
- Multi-world lifecycle management.
- Effect/timer queue claiming and reapers.
- Process-local CAS cache for hosted worlds.
- Worker admin/control surfaces.

Must not own:

- The single-world execution engine internals that belong in `aos-world`.
- Local/filesystem daemon/webserver assumptions from today’s `aos-host`.

### 4) `aos-cli`

Purpose after refactor:

- Operator/dev tool.
- Continues to use `aos-world` directly for local/filesystem flows.
- Talks to `aos-worker` as an external process for hosted mode.

Recommended direction:

- Absorb any remaining local-only daemon/control/http wrapper code that exists solely to support `aos run` local behavior.
- Do not create a separate permanent crate just for the current local daemon/webserver.

### 5) Temporary `aos-host` Compatibility Facade

Purpose:

- Keep the workspace moving while imports migrate.
- Re-export from `aos-world` and `aos-adapters`.
- Temporarily hold local daemon/control/http pieces until they either move to `aos-cli` or are retired.

Rules:

- No new net-new runtime features should be added to `aos-host`.
- `aos-host` exists only to reduce migration churn.
- Once smoke/evals/CLI/worker are migrated, `aos-host` should be deleted or left as a deprecated shell.

## Module Mapping From Today

Recommended destination for current `crates/aos-host/src/*` modules:

### Move to `aos-world`

- `host.rs`
- `hosted.rs`
- `manifest_loader.rs`
- `trace.rs`
- `testhost.rs`
- engine-facing parts of `config.rs`
- engine-facing parts of `error.rs`
- engine-facing parts of `util.rs`
- `modes/batch.rs` if retained

### Move to `aos-adapters`

- `adapters/registry.rs`
- `adapters/traits.rs`
- `adapters/http.rs`
- `adapters/llm.rs`
- `adapters/blob_get.rs`
- `adapters/blob_put.rs`
- `adapters/mock.rs`
- `adapters/stub.rs`
- `adapters/host/*` if that subtree is still considered a built-in adapter set rather than world-engine logic
- adapter-facing parts of `config.rs`
- adapter-facing parts of `error.rs` if split is useful

### Keep out of `aos-adapters`

- `adapters/timer.rs`

Reason:

- It is scheduler/runtime glue for local daemon mode, not a clean external adapter boundary.

### Move to `aos-worker`

- New hosted worker modules implementing the P3 runtime plane.
- New config types for leases, scan budgets, worker identity, hosted CAS cache, shard ownership, and background loops.
- Any hosted worker admin/control surface that is specific to worker operation.

### Move to `aos-cli` or Keep Temporarily in Compatibility Layer

- `control.rs`
- `http/mod.rs`
- `http/api.rs`
- `http/publish.rs`
- `modes/daemon.rs`
- `cli/*`
- CLI-only pieces of `world_io.rs`
- CLI-only pieces of `util.rs`

Rationale:

- These are local process/operator concerns, not reusable single-world engine concerns.
- They do not belong in `aos-worker` either.
- Keeping them out of the final `aos-world`/`aos-adapters` split avoids dragging local daemon/webserver assumptions into hosted code.

## Recommended Naming

### Crates

- `aos-world`: reusable single-world engine
- `aos-adapters`: adapter registry and built-ins
- `aos-worker`: hosted/FDB worker process

### Config Types

- `WorldConfig` in `aos-world`
- `AdapterConfig` / adapter-specific config types in `aos-adapters`
- `WorkerConfig` in `aos-worker`

### Harness Types

- Keep `TestHost` temporarily for migration stability.
- Consider a later rename to `WorldHarness` or `WorldTestKit` after the crate split settles.

## Required Config Split

Today’s `HostConfig` mixes unrelated concerns. It should be decomposed.

### `aos-world::WorldConfig`

Should cover:

- module cache dir
- eager module load
- placeholder secret policy
- strict effect binding behavior
- any purely engine-facing execution knobs

### `aos-adapters::*Config`

Should cover:

- HTTP adapter config
- LLM adapter/provider config
- adapter route/provider wiring
- other built-in adapter configuration

### `aos-worker::WorkerConfig`

Should cover:

- lease timings
- batch/scan budgets
- worker identity and heartbeat settings
- hosted CAS cache sizing
- effect/timer claim timeouts
- shard/scan configuration

### Local Daemon / CLI Config

Should cover:

- local control socket path
- local HTTP bind/enable flags
- local daemon-specific UX toggles

This should live in `aos-cli` or a temporary compatibility layer, not in `aos-worker`.

## Consumer Migration Plan

### `aos-smoke`

Target dependency shape:

- depend on `aos-world`
- depend on `aos-adapters` when adapter types are used
- do not depend on `aos-worker`

Expected migration:

- swap `aos_host::testhost::TestHost` -> `aos_world::testkit::TestHost` (or compatible re-export during transition)
- swap `aos_host::manifest_loader` -> `aos_world::manifest_loader`
- swap adapter imports to `aos_adapters::*`

### `aos-agent-eval`

Target dependency shape:

- depend on `aos-world`
- depend on `aos-adapters`
- do not depend on `aos-worker`

Expected migration:

- same as smoke: harness, manifest loading, adapters
- keep eval-specific orchestration outside of hosted worker code

### `aos-cli`

Target dependency shape:

- depend on `aos-world`
- depend on `aos-adapters`
- optionally spawn/talk to `aos-worker` as an external process when hosted mode is added

Expected migration:

- local/fs commands continue using `aos-world`
- move local daemon/control/http wrapper code out of the reusable engine boundary
- hosted worker operation should not require linking the full worker as a library into `aos-cli`

## Test Migration Plan

Split current `aos-host` tests by the boundary they actually validate.

### Move to `aos-world`

- world execution
- manifest loading
- snapshot/replay
- hosted open/restore at the single-world engine level
- local batch/test harness behavior
- single-world governance/state/query behavior

### Move to `aos-adapters`

- adapter unit/integration tests
- mock/stub adapter behavior
- provider-specific adapter tests

### Move to `aos-worker`

- multi-world worker lifecycle
- lease/heartbeat/claiming behavior
- hosted effect/timer queues and reapers
- worker failover and idle-release behavior

### Move to `aos-cli` or Temporary Compatibility Area

- local daemon control server tests
- local HTTP server tests
- CLI-facing run/stop/head integration tests tied to the local process wrapper

## Execution Plan

### 1) Extract `aos-adapters` first

Why first:

- It has the cleanest boundary.
- It immediately removes a large amount of unrelated code from the engine crate.
- Both future `aos-world` and `aos-worker` will need it.

Required work:

- create `crates/aos-adapters`
- move registry/traits/built-ins
- split config so adapter-specific types move with adapters
- update imports in `aos-cli`, `aos-smoke`, `aos-agent-eval`, and `aos-host`

### 2) Create `aos-world`

Required work:

- create `crates/aos-world`
- move `WorldHost` and related single-world helpers
- move `HostedStore` / `HostedJournal` bridge
- move test harnesses and manifest loading
- move or absorb `BatchRunner` if still useful
- split engine-facing config/error/util pieces

### 3) Turn `aos-host` into a compatibility facade

Required work:

- re-export `aos-world` and `aos-adapters` surfaces
- keep only temporary local daemon/control/http wrapper code plus compatibility exports
- stop adding new code to it

### 4) Migrate smoke/evals/CLI off `aos-host`

Required work:

- update `aos-smoke`
- update `aos-agent-eval`
- update `aos-cli`
- keep import churn manageable with temporary re-exports where helpful

### 5) Introduce `aos-worker`

Required work:

- add new crate/binary
- depend on `aos-world`, `aos-adapters`, `aos-fdb`
- implement hosted worker runtime from `p3-universe-runtime-plane.md`
- avoid reusing local daemon/http/control modules from the old `aos-host` boundary

### 6) Remove or drain remaining local daemon code from compatibility layer

Preferred end state:

- move local-only process wrapper concerns into `aos-cli`
- delete `aos-host`

Acceptable transitional state:

- keep `aos-host` as a deprecated compatibility shell for one milestone if migration churn is still too high

## Acceptance Criteria

- `aos-world`, `aos-adapters`, and `aos-worker` exist with the intended boundaries.
- `aos-smoke` and `aos-agent-eval` depend on `aos-world` rather than the old `aos-host` boundary.
- `aos-worker` depends on `aos-world` and `aos-adapters`, not on local daemon/http/control code.
- Adapter code no longer lives inside the single-world engine crate.
- `HostConfig` has been split into world/adapter/worker/local-daemon concerns.
- `aos-host` is either removed or clearly reduced to a deprecated compatibility facade.

## Suggested Implementation Order

1. Extract `aos-adapters`.
2. Introduce `aos-world`.
3. Add compatibility re-exports in `aos-host`.
4. Migrate `aos-smoke` and `aos-agent-eval`.
5. Migrate `aos-cli`.
6. Build `aos-worker`.
7. Delete or deprecate `aos-host`.

## Explicitly Avoid

- `aos-runtime-core`
- `aos-control`
- `aos-http`
- `aos-testkit`
- `aos-local-daemon`
- any attempt to make hosted worker orchestration backend-agnostic before the FDB path is proven

The right abstraction line is not “everything runtime-shaped in one crate”.  
The right abstraction line is:

- `aos-world` for single-world execution
- `aos-adapters` for effect adapters
- `aos-worker` for hosted worker orchestration
