# v0.20 Fabric Migration Plan

## Purpose

Move Fabric out of `exa-fac` and into the AOS repository so Fabric becomes normal AOS edge
infrastructure instead of a factory-specific dependency.

This plan is written to be executed from:

```text
/Users/lukas/dev/aos
```

The source repository is:

```text
/Users/lukas/dev/exa-fac
```

The target repository is:

```text
/Users/lukas/dev/aos
```

## Decision

Fabric should move under the AOS repo for v0.20.

The dependency direction remains:

```text
aos-effect-adapters
  -> fabric-client
  -> fabric-protocol
```

The forbidden direction remains:

```text
fabric-* -> aos-node / aos-kernel / aos-effect-adapters
```

Fabric is not an `exa-fac` domain feature. It is the host/session execution substrate that AOS
uses to satisfy `host.*` effects. `exa-fac` should consume AOS and Fabric through AOS APIs, not host
the generic runtime code that AOS depends on.

## Why This Move Is Happening

The current layout has the dependency ownership backwards:

- `exa-fac` is intended to hold concrete factory worlds, agents, UIs, and infra scripts.
- Fabric is generic remote execution infrastructure.
- AOS already needs Fabric as a backend for the existing `host.*` effect surface.
- The AOS checkout already has local path dependencies back into `exa-fac` for Fabric crates.

That means `exa-fac` is currently acting like an upstream runtime repo, even though conceptually it
is downstream application/factory code.

Moving Fabric into AOS gives us:

- one repo for AOS runtime, adapters, and edge providers,
- no local cross-repo Cargo path dependencies from AOS into `exa-fac`,
- a single place to evolve `HostTarget`, schemas, adapter routing, and Fabric protocol together,
- simpler CI once Fabric becomes part of the normal AOS workspace,
- a cleaner future where `exa-fac` contains only factory-specific worlds, agents, UIs, and
  deployment/config scripts.

## Boundary After Migration

### AOS Owns

AOS owns the generic runtime and execution infrastructure:

```text
crates/fabric-protocol/
crates/fabric-client/
crates/fabric-controller/
crates/fabric-host/
crates/fabric-cli/
crates/aos-effect-adapters/
crates/aos-effect-types/
crates/aos-node/
crates/aos-cli/
spec/
roadmap/v0.20-fabric/
```

AOS also owns the third-party smolvm submodule required by the first Fabric host provider:

```text
third_party/smolvm/
```

The ignored smolvm runtime bundle remains local generated state:

```text
third_party/.cache/
third_party/smolvm-release/
```

### Exa Factory Owns

`exa-fac` owns concrete software factory assets:

```text
worlds/
agents/
apps/
infra/
factory-specific scripts
factory-specific config
factory-specific roadmap/docs
```

After this migration, `exa-fac` should not contain Fabric Rust crates. If it needs to start Fabric
locally, it should call AOS/Fabric binaries built from the AOS repo or use documented AOS dev
commands.

## Source To Destination Map

Move these source paths from `exa-fac` to `aos`:

| Source in `/Users/lukas/dev/exa-fac` | Destination in `/Users/lukas/dev/aos` | Notes |
| --- | --- | --- |
| `crates/fabric-protocol/` | `crates/fabric-protocol/` | Shared Fabric request, response, event, and OpenAPI types. |
| `crates/fabric-client/` | `crates/fabric-client/` | HTTP client, controller client, host client, NDJSON exec stream decoding. |
| `crates/fabric-controller/` | `crates/fabric-controller/` | Controller API, SQLite state, scheduler, host registration, proxying. |
| `crates/fabric-host/` | `crates/fabric-host/` | Host daemon, smolvm provider, exec/fs/session implementation. |
| `crates/fabric-cli/` | `crates/fabric-cli/` | Development CLI binary named `fabric`. |
| `scripts/bootstrap-smolvm-release.sh` | `dev/fabric/bootstrap-smolvm-release.sh` | Stages local ignored smolvm/libkrun release bundle. |
| `scripts/build-fabric-host.sh` | `dev/fabric/build-fabric-host.sh` | Builds and signs `fabric-host` on macOS. |
| `scripts/test-smolvm-e2e.sh` | `dev/fabric/test-smolvm-e2e.sh` | Host smolvm E2E wrapper. |
| `scripts/test-controller-smolvm-e2e.sh` | `dev/fabric/test-controller-smolvm-e2e.sh` | Controller plus host smolvm E2E wrapper. |
| `docs/fabric/smolvm-macos.md` | `dev/fabric/smolvm-macos.md` | macOS libkrun, signing, and smoke-test notes. |
| `roadmap/02-fabric/fabric.md` | `roadmap/v0.20-fabric/fabric.md` | Main Fabric architecture and milestone document. |
| `roadmap/02-fabric/p1-fabric-host-daemon.md` | `roadmap/v0.20-fabric/p1-fabric-host-daemon.md` | Host daemon phase document. |
| `roadmap/02-fabric/p2-fabric-controller.md` | `roadmap/v0.20-fabric/p2-fabric-controller.md` | Controller phase document. |
| `roadmap/02-fabric/p3-aos-host-adapter.md` | `roadmap/v0.20-fabric/p3-aos-host-adapter.md` | AOS adapter integration phase document. |
| `roadmap/02-fabric/p10-attached-host-daemon.md` | `roadmap/v0.20-fabric/p10-attached-host-daemon.md` | Later attached-host provider design. |

Do not move these as normal tracked source:

| Source | Reason |
| --- | --- |
| `third_party/smolvm-release/` | Generated local release bundle. Recreate with `dev/fabric/bootstrap-smolvm-release.sh`. |
| `third_party/.cache/` | Generated download cache. |
| `.fabric-host/`, `.fabric-ctrl/`, `var/` | Local daemon/controller state. |
| `target/` | Cargo build output. |
| `Cargo.lock` from `exa-fac` | AOS already has its own lockfile. Regenerate/update the AOS lockfile. |
| `README.md` from `exa-fac` | It is factory repo documentation. Port only relevant Fabric runbook sections into AOS docs if needed. |
| `rust-toolchain.toml` from `exa-fac` | It only pins stable. AOS already builds edition 2024; add an AOS toolchain file only as a separate repo decision. |

## Target AOS Repo Shape

After migration, AOS should look like:

```text
aos/
  Cargo.toml
  Cargo.lock
  .gitmodules
  .gitignore
  crates/
    aos-*
    fabric-protocol/
    fabric-client/
    fabric-controller/
    fabric-host/
    fabric-cli/
  roadmap/
    v0.20-fabric/
      rec.md
      migration-plan.md
      fabric.md
      p1-fabric-host-daemon.md
      p2-fabric-controller.md
      p3-aos-host-adapter.md
      p10-attached-host-daemon.md
  dev/
    fabric/
      smolvm-macos.md
      bootstrap-smolvm-release.sh
      build-fabric-host.sh
      test-smolvm-e2e.sh
      test-controller-smolvm-e2e.sh
    hosted/
      hosted-up.sh
      hosted-down.sh
      hosted-topics-ensure.sh
  third_party/
    smolvm/
    .cache/              # ignored
    smolvm-release/      # ignored
```

## Invariants To Preserve

1. AOS workflows continue to emit `host.*` effects.
2. Fabric remains a backend/provider for those effects, not a separate workflow-visible
   `fabric.*` effect catalog.
3. AOS remains authoritative for effect lifecycle, replay, stream-frame admission, and receipt
   admission.
4. Fabric controller remains authoritative for session allocation, idempotency, host choice, and
   controller-side session records.
5. Fabric host remains authoritative only for the live process/VM/workspace it owns.
6. `fabric-protocol` must not depend on any `aos-*` crate.
7. `fabric-client` must not depend on any `aos-*` crate.
8. `fabric-controller` must not depend on `aos-*` crates or `fabric-host`.
9. `fabric-host` must not depend on `aos-*` crates.
10. AOS-specific translation belongs in `aos-effect-adapters`.

## Phase 0: Preflight

Run these from `/Users/lukas/dev/aos` before changing files:

```sh
git status --short
git branch --show-current
git -C /Users/lukas/dev/exa-fac status --short
git -C /Users/lukas/dev/exa-fac rev-parse HEAD
git -C /Users/lukas/dev/exa-fac submodule status
```

Record the `exa-fac` commit and the smolvm submodule commit in the PR/commit message.

If the AOS worktree already has unrelated edits, do not revert them. Either commit/stash them
explicitly or keep the migration patch scoped around them.

Current known source facts at the time this plan was written:

- Fabric source root: `/Users/lukas/dev/exa-fac`
- AOS source root: `/Users/lukas/dev/aos`
- smolvm source submodule path in `exa-fac`: `third_party/smolvm`
- smolvm submodule URL: `https://github.com/smol-machines/smolvm.git`
- smolvm commit observed from `exa-fac`: `f83a1bbdbb7abdde79c481dfe32df43934b59427`
- AOS already has `roadmap/v0.20-fabric/rec.md`
- AOS already has Fabric-related local path dependencies that point back into `exa-fac`; those must
  be replaced during the migration.

## Phase 1: Prepare AOS Workspace

### 1. Add ignored local Fabric runtime state

Update `/Users/lukas/dev/aos/.gitignore` to include:

```gitignore
# Local third-party runtime bundles downloaded for Fabric development.
third_party/.cache/
third_party/smolvm-release/
```

`.fabric-host/`, `.fabric-ctrl/`, and `var/` are ignored by AOS, so Fabric controller/host local
state can stay out of tracked source.

### 2. Add smolvm as an AOS submodule

From `/Users/lukas/dev/aos`:

```sh
mkdir -p third_party
git submodule add https://github.com/smol-machines/smolvm.git third_party/smolvm
git -C third_party/smolvm checkout f83a1bbdbb7abdde79c481dfe32df43934b59427
git add .gitmodules third_party/smolvm
```

If AOS already has a submodule at that path when executing this plan, update it to the recorded
commit instead of adding a duplicate.

### 3. Add Fabric crates to the AOS workspace

Update `/Users/lukas/dev/aos/Cargo.toml`:

```toml
[workspace]
members = [
  "crates/aos-authoring",
  "crates/aos-effect-types",
  "crates/aos-air-types",
  "crates/aos-air-exec",
  "crates/aos-cbor",
  "crates/aos-node",
  "crates/aos-wasm-abi",
  "crates/aos-wasm",
  "crates/aos-effects",
  "crates/aos-effect-adapters",
  "crates/aos-llm",
  "crates/aos-agent",
  "crates/aos-kernel",
  "crates/aos-harness-py",
  "crates/aos-cli",
  "crates/aos-wasm-sdk",
  "crates/aos-wasm-build",
  "crates/aos-sys",
  "crates/aos-smoke",
  "crates/aos-agent-eval",
  "crates/fabric-protocol",
  "crates/fabric-client",
  "crates/fabric-controller",
  "crates/fabric-host",
  "crates/fabric-cli",
]
resolver = "2"
```

Add workspace metadata and dependencies needed by the imported Fabric crates. AOS crates do not all
need to migrate to `workspace = true` in the same change; this can start as a Fabric-only workspace
dependency table:

```toml
[workspace.package]
version = "0.1.0"
edition = "2024"
publish = false

[workspace.dependencies]
anyhow = "1"
async-trait = "0.1"
axum = "0.8"
base64 = "0.22"
clap = { version = "4", features = ["derive"] }
futures-core = "0.3"
futures-util = "0.3"
globset = "0.4"
hex = "0.4"
libc = "0.2"
regex = "1"
reqwest = { version = "0.12", features = ["json", "stream"] }
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
thiserror = "2"
tokio = { version = "1", features = ["macros", "net", "rt-multi-thread", "signal", "sync", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
utoipa = { version = "5", features = ["axum_extras"] }
utoipa-swagger-ui = { version = "9", features = ["axum"] }
url = "2"
uuid = { version = "1", features = ["v4"] }
walkdir = "2"
```

Do not convert all existing AOS crate dependency declarations in this migration unless required by
Cargo. Keep the move mechanically small.

## Phase 2: Copy Fabric Source Into AOS

From `/Users/lukas/dev/aos`, copy source directories:

```sh
rsync -a /Users/lukas/dev/exa-fac/crates/fabric-protocol crates/
rsync -a /Users/lukas/dev/exa-fac/crates/fabric-client crates/
rsync -a /Users/lukas/dev/exa-fac/crates/fabric-controller crates/
rsync -a /Users/lukas/dev/exa-fac/crates/fabric-host crates/
rsync -a /Users/lukas/dev/exa-fac/crates/fabric-cli crates/

mkdir -p dev/fabric
rsync -a /Users/lukas/dev/exa-fac/docs/fabric/smolvm-macos.md dev/fabric/
rsync -a /Users/lukas/dev/exa-fac/scripts/bootstrap-smolvm-release.sh dev/fabric/
rsync -a /Users/lukas/dev/exa-fac/scripts/build-fabric-host.sh dev/fabric/
rsync -a /Users/lukas/dev/exa-fac/scripts/test-smolvm-e2e.sh dev/fabric/
rsync -a /Users/lukas/dev/exa-fac/scripts/test-controller-smolvm-e2e.sh dev/fabric/
```

Copy Fabric roadmap documents into the AOS v0.20 folder:

```sh
rsync -a /Users/lukas/dev/exa-fac/roadmap/02-fabric/fabric.md roadmap/v0.20-fabric/
rsync -a /Users/lukas/dev/exa-fac/roadmap/02-fabric/p1-fabric-host-daemon.md roadmap/v0.20-fabric/
rsync -a /Users/lukas/dev/exa-fac/roadmap/02-fabric/p2-fabric-controller.md roadmap/v0.20-fabric/
rsync -a /Users/lukas/dev/exa-fac/roadmap/02-fabric/p3-aos-host-adapter.md roadmap/v0.20-fabric/
rsync -a /Users/lukas/dev/exa-fac/roadmap/02-fabric/p10-attached-host-daemon.md roadmap/v0.20-fabric/
```

Do not copy generated or ignored paths:

```text
/Users/lukas/dev/exa-fac/target/
/Users/lukas/dev/exa-fac/var/
/Users/lukas/dev/exa-fac/third_party/.cache/
/Users/lukas/dev/exa-fac/third_party/smolvm-release/
```

## Phase 3: Fix Cargo Paths In AOS

### 1. Imported Fabric crates

The imported Fabric crate paths should continue to work in AOS:

```toml
fabric-client = { path = "../fabric-client" }
fabric-protocol = { path = "../fabric-protocol" }
smolvm = { path = "../../third_party/smolvm" }
smolvm-protocol = { path = "../../third_party/smolvm/crates/smolvm-protocol" }
```

The smolvm paths are still correct because `crates/fabric-host` is two directories below the repo
root.

### 2. Existing AOS crates

Replace any AOS dependency that points back into `exa-fac`.

Known examples:

```toml
# before
fabric-client = { path = "../../../exa-fac/crates/fabric-client" }
fabric-protocol = { path = "../../../exa-fac/crates/fabric-protocol" }

# after
fabric-client = { path = "../fabric-client" }
fabric-protocol = { path = "../fabric-protocol" }
```

For `aos-node` dev dependencies:

```toml
# before
fabric-protocol = { path = "../../../exa-fac/crates/fabric-protocol" }

# after
fabric-protocol = { path = "../fabric-protocol" }
```

Then verify no `exa-fac` paths remain:

```sh
rg -n "exa-fac|\\.\\./\\.\\./\\.\\./exa-fac" Cargo.toml crates
```

Expected result: no matches in active Cargo manifests.

## Phase 4: Keep Smolvm Runtime Optional For Normal AOS Work

The first migration can preserve the current Fabric crate shape, but normal AOS development should
not require a local libkrun bundle unless the developer builds or runs the smolvm host.

After the mechanical move, check whether these commands succeed without bootstrapping smolvm:

```sh
cargo check -p fabric-protocol
cargo check -p fabric-client
cargo check -p fabric-controller
cargo check -p fabric-cli
```

For `fabric-host`, current macOS linking may require `libkrun` when the smolvm runtime is enabled.
Use the existing bootstrap path for host builds:

```sh
dev/fabric/bootstrap-smolvm-release.sh
LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
cargo check -p fabric-host
```

If plain `cargo check --workspace` becomes painful because `fabric-host` always pulls smolvm, make
one of these follow-up changes:

1. Keep `fabric-host` in the workspace but make the smolvm provider dependency fully feature-gated.
2. Split smolvm-specific runtime code into `fabric-host-smolvm` or `fabric-smolvm-provider`.
3. Keep `fabric-host` checkable with `--no-default-features` and make workspace CI use that mode
   unless running Fabric host E2E.

Preferred short-term target:

```sh
cargo check -p fabric-host --no-default-features
```

should not require a libkrun release bundle.

## Phase 5: Update AOS Docs And Runbook References

Update AOS docs after the files are moved:

- `/Users/lukas/dev/aos/AGENTS.md`
- `/Users/lukas/dev/aos/README.md`
- `/Users/lukas/dev/aos/dev/fabric/smolvm-macos.md`
- `/Users/lukas/dev/aos/roadmap/v0.20-fabric/fabric.md`
- `/Users/lukas/dev/aos/roadmap/v0.20-fabric/p3-aos-host-adapter.md`

Required wording changes:

- Replace `../aos` sibling-checkout language with "Fabric lives in this AOS workspace."
- Remove guidance that says AOS should depend on Fabric by relative path through `../../../exa-fac`.
- Preserve the architecture rule that Fabric crates remain AOS-independent.
- Make clear that `exa-fac` is now downstream factory code.
- Keep `host.*` as the workflow-visible AOS effect API.

Update command examples so they are rooted at `/Users/lukas/dev/aos`:

```sh
dev/fabric/bootstrap-smolvm-release.sh
dev/fabric/build-fabric-host.sh

LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
RUST_LOG=info \
target/debug/fabric-host \
  --bind 127.0.0.1:8791 \
  --state-root .fabric-host \
  --host-id local-dev
```

## Phase 6: Continue P3 From The AOS Side

Once Fabric source builds inside AOS, continue the AOS integration work in the same repo.

The core P3 work remains:

1. Extend host target schemas.
   - `crates/aos-effect-types/src/host.rs`
   - `spec/defs/builtin-schemas-host.air.json`
   - any generated or mirrored schema docs/tests

2. Refactor the monolithic local host adapter.
   - split shared wrappers from backend implementations,
   - keep CBOR decode, receipt encode, CAS materialization, and stream-frame construction shared,
   - move local process/session behavior behind a backend,
   - add Fabric backend that calls `fabric-client`.

3. Register Fabric-backed provider adapter kinds.
   - `host.session.open.fabric`
   - `host.exec.fabric`
   - `host.session.signal.fabric`
   - `host.fs.read_file.fabric`
   - `host.fs.write_file.fabric`
   - `host.fs.edit_file.fabric`
   - `host.fs.apply_patch.fabric`
   - `host.fs.grep.fabric`
   - `host.fs.glob.fabric`
   - `host.fs.stat.fabric`
   - `host.fs.exists.fabric`
   - `host.fs.list_dir.fabric`

4. Preserve existing workflow-facing effect kinds.
   - workflows emit `host.session.open`, not `fabric.session.open`,
   - workflows emit `host.exec`, not `fabric.exec`,
   - world manifests bind `host.*` effects to logical route IDs,
   - host runtime config maps logical route IDs to provider adapter kinds.

5. Add AOS end-to-end tests.
   - open a sandbox session through the controller,
   - run long `host.exec` and observe time-based progress frames,
   - verify final receipts replay without re-running Fabric work,
   - verify filesystem RPCs translate into existing `sys/Host*` receipt shapes.

## Phase 7: Validation Gates

Run these first:

```sh
cargo check -p fabric-protocol
cargo test -p fabric-protocol
cargo check -p fabric-client
cargo test -p fabric-client
cargo check -p fabric-controller
cargo test -p fabric-controller
```

Then check AOS crates that directly depend on Fabric:

```sh
cargo check -p aos-effect-adapters
cargo test -p aos-effect-adapters
cargo check -p aos-node
cargo test -p aos-node
```

Then run the broader workspace check:

```sh
cargo check --workspace
```

If `cargo check --workspace` fails only because `fabric-host` requires smolvm/libkrun setup, either:

- run the smolvm bootstrap and set the required env vars, or
- make `fabric-host --no-default-features` checkable and adjust workspace/CI expectations.

For smolvm-backed host validation on macOS:

```sh
dev/fabric/bootstrap-smolvm-release.sh
dev/fabric/build-fabric-host.sh
dev/fabric/test-smolvm-e2e.sh
dev/fabric/test-controller-smolvm-e2e.sh
```

For manual smoke testing:

```sh
LIBKRUN_BUNDLE="$PWD/third_party/smolvm-release/lib" \
DYLD_LIBRARY_PATH="$PWD/third_party/smolvm-release/lib" \
SMOLVM_AGENT_ROOTFS="$PWD/third_party/smolvm-release/agent-rootfs" \
RUST_LOG=info \
target/debug/fabric-host \
  --bind 127.0.0.1:8791 \
  --state-root .fabric-host \
  --host-id local-dev
```

In another terminal:

```sh
cargo run -p fabric-cli -- host open --session-id stream-demo --image alpine:latest --net

cargo run -p fabric-cli -- \
  host exec stream-demo -- \
  sh -lc 'for i in $(seq 1 5); do echo "stdout tick $i"; echo "stderr tick $i" >&2; sleep 1; done'

cargo run -p fabric-cli -- host signal stream-demo close
```

## Phase 8: Clean Up Exa Factory

After AOS builds and the migration is committed in AOS, clean up `exa-fac` in a separate change.

Remove or stop tracking:

```text
crates/fabric-protocol/
crates/fabric-client/
crates/fabric-controller/
crates/fabric-host/
crates/fabric-cli/
dev/fabric/bootstrap-smolvm-release.sh
dev/fabric/build-fabric-host.sh
dev/fabric/test-smolvm-e2e.sh
dev/fabric/test-controller-smolvm-e2e.sh
dev/fabric/smolvm-macos.md
third_party/smolvm
```

Update `exa-fac` docs:

- say Fabric now lives in `/Users/lukas/dev/aos`,
- say `exa-fac` is downstream and consumes AOS/Fabric,
- remove language about later adding AOS as a submodule,
- remove local Cargo workspace references if no Rust crates remain,
- keep only factory-specific plans, worlds, agents, UIs, and infra scripts.

If `exa-fac` still needs helper scripts to start a local AOS/Fabric stack, those scripts should call
the AOS repo explicitly or accept `AOS_REPO=/Users/lukas/dev/aos`.

## Rollback Plan

If the import breaks AOS badly before any follow-up refactor is complete:

1. Revert the AOS commit that added Fabric crates, submodule, scripts, and Cargo workspace members.
2. Restore the old temporary path dependencies in AOS only if needed to continue P3 development.
3. Leave `exa-fac` unchanged until the AOS import is green.

Do not delete Fabric from `exa-fac` until the AOS-side migration has passed basic checks.

## AOS-Side Validation Status

Status as of 2026-04-21: complete.

The AOS-side migration gates have been run and passed:

- `cargo check -p fabric-protocol`
- `cargo test -p fabric-client`
- `cargo test -p fabric-controller`
- `cargo check -p aos-effect-adapters`

The smolvm-backed host/controller validation has also been run and passed from this AOS checkout:

- `dev/fabric/bootstrap-smolvm-release.sh`
- `dev/fabric/build-fabric-host.sh`
- `dev/fabric/test-smolvm-e2e.sh`
- `dev/fabric/test-controller-smolvm-e2e.sh`

## Acceptance Criteria

The migration is complete when:

- `aos` contains all Fabric crates and docs listed in the source map.
- `aos` owns the smolvm submodule at the recorded commit or an intentionally updated commit.
- AOS Cargo manifests no longer refer to `../../../exa-fac/crates/fabric-*`.
- `cargo check -p fabric-protocol` passes in AOS.
- `cargo test -p fabric-client` passes in AOS.
- `cargo test -p fabric-controller` passes in AOS.
- `cargo check -p aos-effect-adapters` passes in AOS.
- The Fabric host can still be built and signed with `dev/fabric/build-fabric-host.sh` after
  `dev/fabric/bootstrap-smolvm-release.sh`.
- Roadmap docs under `roadmap/v0.20-fabric/` describe Fabric as an AOS-owned edge runtime.
- `exa-fac` no longer presents itself as the owner of generic Fabric runtime crates after its
  follow-up cleanup.

## Open Follow-Ups

These are not required for the mechanical migration but should be handled soon after:

1. Decide whether `fabric-cli` remains a standalone `fabric` binary or becomes an `aos fabric`
   command group.
2. Decide whether smolvm should stay inside `fabric-host` behind features or move to a separate
   provider crate.
3. Normalize overlapping dependency versions between AOS and Fabric, especially `reqwest`,
   `rusqlite`, `thiserror`, and `base64`.
4. Add CI jobs that separate pure protocol/client/controller checks from smolvm host E2E checks.
5. Update `exa-fac` infra scripts to launch AOS-owned Fabric services instead of building local
   Fabric crates.
