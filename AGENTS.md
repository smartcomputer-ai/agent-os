# Agents

This file provides guidance to coding agents when working with code in this repository.

## What This Is

**AgentOS** is a deterministic, event-sourced computing substrate for AI agents.

**AIR (Agent Intermediate Representation)** is the typed control-plane IR governing schemas, modules, effects, secrets, and manifests.

## Reading the Specs (index)

1. **spec/01-overview.md** — Core concepts and mental model.
2. **spec/02-architecture.md** — Runtime components, storage layout, governance phases.
3. **spec/03-air.md** — **CRITICAL** AIR v1 spec (post-plan active model).
4. **spec/04-workflows.md** — Workflow module semantics and orchestration patterns.
5. **spec/05-effects.md** — Effects, async execution, durable open work, and continuation admission.
6. **spec/06-backends.md** — Backend contracts, SQLite/Kafka journals, local/object-store CAS, and recovery.

Future protocol notes:
- **spec/20-gc.md** — GC/reachability contract draft; deletion GC is not implemented yet.

Reference shelves:
- **spec/schemas/** (JSON Schemas)
- **spec/defs/** (built-ins: Timer/Blob/HTTP/LLM/Workspace/Introspect plus SDK support schemas)

## Core Architecture (TL;DR)

**World**: Single-threaded deterministic kernel state. Replay checkpoint/snapshot + journal frames + receipts = identical state.

**Workspaces**: Versioned tree registry (`sys/Workspace@1`). Tree ops (`workspace.*`) are internal effects used by `aos ws` plus `aos push`/`aos pull`.

**Active layers**:
- **Workflow modules** (WASM, `module_kind: workflow`): deterministic state machines on workflow ABI; own orchestration/state transitions.
- **Pure modules** (`module_kind: pure`): side-effect-free compute helpers.
- **Kernel** (`aos-kernel`): synchronous authoritative world execution, governance, manifests, effect authorization, snapshots, replay, and open-work state.
- **Unified node** (`aos-node`): owns hot worlds, serializes per-world execution, stages completed slices, durably flushes journal frames, serves hot control reads, and publishes opened async effects only after durable append.
- **Executors/Adapters**: async edge execution for external work; stream frames and receipts re-enter only as world input through owner admission.

**Governance path**:
- propose -> shadow -> approve -> apply -> execute -> receipt -> audit

Shadow reports bounded observed effects/in-flight state and ledger deltas. Primary state is unchanged until apply.

**Runtime shape (v0.19+)**:
- `aos-cli` is the executable surface; `aos node up` starts the unified node through the hidden `node-serve` entrypoint.
- `aos-node` is library-only and contains runtime/control/backend implementation.
- There are no separate `aos-node-local`, `aos-node-hosted`, or `aos-host` product crates.
- Direct HTTP/control acceptance is the default ingress path.
- Reads are hot in-process reads from active worlds, not projection/materializer reads.
- SQLite is the default journal backend; Kafka is an explicit journal backend, not the system architecture.
- Discovery/checkpoints are world-based. Journal partitioning is backend metadata only.
- Acceptance returns backend-neutral `accept_token`/sequence semantics, not Kafka-shaped submission offsets.

**Critical boundaries (v1/v0.19)**:
- Only workflow modules may emit effects.
- Emitted effects must be declared in `abi.workflow.effects_emitted`.
- Emitted effects must pass declared-effect and origin-scope authorization before dispatch.
- Open external work is recorded before execution starts.
- Opened async effects are published only after their containing frame has durably flushed.
- Executors never mutate world state directly; stream frames and receipts re-enter through world input.
- Domain ingress wiring is `routing.subscriptions`.
- Receipt continuation routing is manifest-independent and uses pending intent identity.
- Strict quiescence blocks apply while in-flight runtime work exists.

## Key Principles

1. Determinism by default (replay-identical state)
2. Explicit effects (no ambient I/O)
3. Receipts everywhere (signed, auditable)
4. Minimal trusted base
5. Content-addressed, portable worlds

## Implementation Notes

**Testing invariant**: "Replay-or-die" — replay from genesis must produce byte-identical snapshots.

**Key implementation notes**:
- Loader accepts authoring sugar + canonical JSON, validates against schemas, emits canonical CBOR.
- Validator enforces module ABI contracts, routing semantics, and effect allowlists.
- Effect manager canonicalizes params, authorizes effect origins, records open work, and validates admitted continuations/receipts.
- Event and receipt ingress are schema-validated and canonicalized once; journal stores canonical CBOR.
- Manifest changes are journaled as `Manifest` records; replay applies them in order.
- Node execution center: `aos-node/src/worker/` owns hot-world scheduling, slice staging, durable flush, replay/open, and checkpoint publication.
- Async execution edge: `aos-node/src/execution/` starts timers/external effects after durable flush; adapters live in `aos-effect-adapters`.
- Backend seams: `aos-node/src/model/backends.rs` defines journal, checkpoint, world inventory, and blob contracts; implementations live under `aos-node/src/infra/`.
- Control surface: `aos-node/src/control/` serves hot active-world reads and direct acceptance. Do not rebuild projection/materializer paths for default reads.
- Module build/cache: workflows compiled via `aos-wasm-build`, cached under `.aos/cache/{modules|wasmtime}`.
- Workspace sync uses `aos.sync.json` plus `aos push`/`aos pull`; filesystem names are segment-encoded with `~`-hex when needed.

## Project Structure (Rust workspace, edition 2024)

Crates keep deterministic core small and effectful code at the edges:

- `aos-air-types` — AIR data types, schemas, manifests, effects, and semantic validation.
- `aos-air-exec` — Pure AIR expression/value evaluator.
- `aos-cbor` — Canonical CBOR + SHA-256 helpers.
- `aos-kernel` — Deterministic synchronous world kernel: manifests, governance, effect authorization, internal effects, open work, journal tail, snapshots, replay, and workflow runtime state.
- `aos-wasm-abi` — `no_std` envelopes shared by workflow/pure WASM components.
- `aos-wasm` — Deterministic Wasmtime wrapper for module execution.
- `aos-wasm-sdk` — Workflow helper library for `wasm32-unknown-unknown`.
- `aos-wasm-build` — Deterministic workflow compiler + module cache.
- `aos-effect-types` — `no_std` built-in effect parameter/receipt payload types.
- `aos-effects` — Effect intent, receipt, stream frame, capability grant, normalization, and shared traits.
- `aos-effect-adapters` — Async adapter registry and implementations for HTTP, LLM, blob, timer, vault, and host/session effects.
- `aos-node` — Unified node library: worker runtime, hot control facade, direct HTTP/control acceptance, async effect runtime, replay/checkpoints, SQLite/Kafka journal backends, blobstore, vault, and node startup config.
- `aos-cli` — Primary `aos` binary: CLI parsing, profiles, workspace commands, authoring flows, control client, and `aos node` orchestration.
- `aos-authoring` — Authored-world build/upload/sync helpers, manifest loading, module placeholder resolution, and local state utilities.
- `aos-sys` — Shared `no_std` types for built-in system workflows such as `sys/Workspace@1` and `sys/HttpPublish@1`.
- `aos-llm` — Shared LLM client/provider abstraction used by adapters and agent tooling.
- `aos-agent` — `aos.agent/*` contract types, helper reducers, and workflow binaries for agent sessions.
- `aos-agent-eval` — Prompt/tool eval runner for agent session workflows.
- `aos-harness-py` — Python bindings for runtime/world harnesses; use the repo `.venv`.
- `aos-smoke` — CLI runners for numbered demos in `crates/aos-smoke/fixtures/`.
- `fabric-protocol` — Fabric request, response, event, and OpenAPI types; no `aos-*` dependencies.
- `fabric-client` — Fabric controller/host HTTP client and NDJSON exec stream helpers.
- `fabric-controller` — Fabric controller API, SQLite state, scheduling, host registration, and proxying.
- `fabric-host` — Fabric host daemon and smolvm-backed session provider.
- `fabric-cli` — Development CLI binary named `fabric`.

Fabric lives in this AOS workspace as generic host/session edge infrastructure. AOS-specific
translation stays in `aos-effect-adapters`; Fabric crates must not depend on `aos-node`,
`aos-kernel`, or `aos-effect-adapters`. The smolvm source dependency is the `third_party/smolvm`
submodule, while local generated runtime bundles live under ignored `third_party/.cache/` and
`third_party/smolvm-release/`.

## Test Strategy (Concise, Deterministic)

- Unit tests live next to code: place `mod tests` at the bottom with `#[cfg(test)]`.
- Integration tests go under `tests/` when crossing crate boundaries, doing I/O, or running kernel/effect flows.
- Naming: `function_under_test_condition_expected()`.
- Determinism: no wall-clock/randomness; use deterministic fixtures.
- Errors: assert on typed errors/kinds, not strings.
- Parallel-safe by default.
- Doctests should compile and run.
- Store CBOR/journal goldens under `tests/data/` and diff byte-for-byte.
- Replay-or-die: run once, journal, replay from genesis, assert byte-identical snapshots.
- Async tests: prefer `#[tokio::test(flavor = "current_thread")]` when determinism matters.
- Runtime/backend integration tests often require `--features e2e-tests` on `aos-node`, `aos-effect-adapters`, or `aos-kernel`.


## Other Notes
- When asked how many lines of code, use `cloc $(git ls-files)`
- If the user complains that the computer goes to sleep tell him to use `caffeinate -i`
- Python work in this monorepo should use the shared repo env at `.venv`, not per-project virtualenvs.
- Bootstrap/update that env with `./setup-python.sh`; it installs the local `aos-harness-py` package via `maturin develop`.


## Examples Ladder

- See `crates/aos-smoke/fixtures/README.md` for numbered demos.

## Keeping Documentation Updated

When architecture/spec behavior changes:
1. Update relevant files in `spec/`.
2. Update this file (AGENTS.md / CLAUDE.md symlink) for top-level guidance.
3. Keep this file aligned with active runtime semantics (post-plan workflow model).

Specs are source-of-truth; this file is a practical index.
