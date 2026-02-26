# Agents

This file provides guidance to coding agents when working with code in this repository.

## What This Is

**AgentOS** is a deterministic, event-sourced computing substrate for AI agents.

**AIR (Agent Intermediate Representation)** is the typed control-plane IR governing schemas, modules, effects, capabilities, policies, secrets, and manifests.

## Reading the Specs (index)

1. **spec/01-overview.md** — Core concepts and mental model.
2. **spec/02-architecture.md** — Runtime components, storage layout, governance phases.
3. **spec/03-air.md** — **CRITICAL** AIR v1 spec (post-plan active model).
4. **spec/04-reducers.md** — Workflow module semantics on reducer ABI.
5. **spec/05-workflows.md** — Workflow patterns in the workflow-only architecture.
6. **spec/06-cells.md** — Keyed workflow instances.

Reference shelves:
- **spec/schemas/** (JSON Schemas)
- **spec/defs/** (built-ins: Timer/Blob/HTTP/LLM/Workspace/Introspect)
- **spec/patch.md** (historical notes)

## Core Architecture (TL;DR)

**World**: Single-threaded deterministic event log. Replay journal + receipts = identical state.

**Workspaces**: Versioned tree registry (`sys/Workspace@1`). Tree ops (`workspace.*`) are internal effects, cap-gated, and used by `aos ws` plus `aos push`/`aos pull`.

**Active layers**:
- **Workflow modules** (WASM, `module_kind: workflow`): deterministic state machines on reducer ABI; own orchestration/state transitions.
- **Pure modules** (`module_kind: pure`): side-effect-free compute helpers.
- **Effects/Adapters**: execute external actions and return signed receipts.

**Governance path**:
- propose -> shadow -> approve -> apply -> execute -> receipt -> audit

Shadow reports bounded observed effects/in-flight state and ledger deltas. Primary state is unchanged until apply.

**Critical boundaries (v1/v0.11)**:
- Only workflow modules may emit effects.
- Emitted effects must be declared in `abi.reducer.effects_emitted`.
- Capability + policy must both pass before dispatch.
- Domain ingress wiring is `routing.subscriptions`.
- Receipt continuation routing is manifest-independent and uses pending intent identity.
- Strict quiescence blocks apply while in-flight runtime work exists.

## Key Principles

1. Determinism by default (replay-identical state)
2. Capability security (no ambient authority)
3. Receipts everywhere (signed, auditable)
4. Minimal trusted base
5. Content-addressed, portable worlds

## Implementation Notes

**Testing invariant**: "Replay-or-die" — replay from genesis must produce byte-identical snapshots.

**Key implementation notes**:
- Loader accepts authoring sugar + canonical JSON, validates against schemas, emits canonical CBOR.
- Validator enforces module ABI contracts, routing semantics, capability bindings, and effect allowlists.
- Effect manager canonicalizes params, runs cap/policy gates, dispatches adapters, validates receipts.
- Event and receipt ingress are schema-validated and canonicalized once; journal stores canonical CBOR.
- Manifest changes are journaled as `Manifest` records; replay applies them in order.
- Module build/cache: reducers/workflows compiled via `aos-wasm-build`, cached under `.aos/cache/{modules|wasmtime}`.
- Workspace sync uses `aos.sync.json` plus `aos push`/`aos pull`; filesystem names are segment-encoded with `~`-hex when needed.

## Project Structure (Rust workspace, edition 2024)

Crates keep deterministic core small and effectful code at the edges:

- `aos-air-types` — AIR data types + semantic validation.
- `aos-air-exec` — Pure expression/value evaluator.
- `aos-cbor` — Canonical CBOR + SHA-256 helpers.
- `aos-store` — Content-addressed store + manifest loader utilities.
- `aos-effects` — Effect intent/receipt types and adapter traits.
- `aos-kernel` — Deterministic stepper, governance, policy/cap ledgers, journal/snapshots, workflow runtime state.
- `aos-wasm-abi` — no_std envelopes shared by workflow/pure components.
- `aos-wasm` — Deterministic Wasmtime wrapper for module execution.
- `aos-wasm-sdk` — Workflow/reducer helper library for `wasm32-unknown-unknown`.
- `aos-wasm-build` — Deterministic workflow/reducer compiler + cache.
- `aos-host` — WorldHost runtime + TestHost harness + fixtures (`e2e-tests` feature).
- `aos-smoke` — CLI runners for numbered demos in `crates/aos-smoke/fixtures/`.

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
- `aos-host` integration tests often require `--features e2e-tests`.

## Examples Ladder

- See `crates/aos-smoke/fixtures/README.md` for numbered demos.

## Keeping Documentation Updated

When architecture/spec behavior changes:
1. Update relevant files in `spec/`.
2. Update this file (AGENTS.md / CLAUDE.md symlink) for top-level guidance.
3. Keep this file aligned with active runtime semantics (post-plan workflow model).

Specs are source-of-truth; this file is a practical index.
