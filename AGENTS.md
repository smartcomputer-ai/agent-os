# Agents

This file provides guidance to coding agents when working with code in this repository. 

## What This Is

**AgentOS** is a deterministic, event-sourced computing substrate for AI agents. **AIR (Agent Intermediate Representation)** is the typed control-plane IR governing modules, plans, schemas, policies, and capabilities.

## Reading the Specs (index)

1. **spec/01-overview.md** — Core concepts and mental model.
2. **spec/02-architecture.md** — Runtime components, storage layout, governance phases.
3. **spec/03-air.md** — **CRITICAL** AIR v1 spec (schemas, modules, plans, caps, policies).
4. **spec/04-reducers.md** — Reducer ABI/semantics; micro-effect rules.
5. **spec/05-workflows.md** — Workflow patterns; see also **spec/07-workflow-patterns.md**.

Reference shelves: **spec/schemas/** (JSON Schemas), **spec/defs/** (built-ins: Timer/Blob/HTTP/LLM/Workspace/Introspect), **spec/patch.md** (historical notes). Future/experimental material lives in 10+ (e.g., 11-cells, 12-plans-v1.1, 13-parallelism, 14-example-reducer-harness, 15-reducer-sdk, 16-query-interfaces, 17-secrets).


## Core Architecture (TL;DR)

**World**: Single-threaded deterministic event log. Replay journal + receipts = identical state.

**Workspaces**: Versioned tree registry (`sys/Workspace@1`) for code/artifacts. Tree ops (`workspace.*`) are plan-only internal effects, cap-gated, and used by `aos ws` plus `aos push`/`aos pull`.

**Three layers**:
- **Reducers** (WASM state machines): Domain logic, business invariants, emit events. May emit micro-effects (timer, blob) ONLY. See spec/04-reducers.md
- **Plans** (DAG orchestration): Multi-step effect workflows under governance. All risky effects (http, llm, payments, email). See spec/03-air.md §11
- **Effects/Adapters**: Execute external actions, return signed receipts. See spec/02-architecture.md

**Governance path**: propose → shadow (predict) → approve → apply → execute → receipt → audit. Shadow spins a mirror world from the candidate manifest, predicts effects/plan results, records ledger deltas + manifest hash; primary state is unchanged until apply.

**Critical boundaries (v1)**:
- **Reducers**: Own state and business logic. Emit DomainIntent events for external work. May emit at most ONE micro-effect per step (blob.{put,get}, timer.set). NO network effects.
- **Plans**: Orchestrate effects (http, llm, payments, email) triggered by intents. Raise result events back to reducers. NO compute or business logic.
- **Workspace ops**: `workspace.*` effects are plan-only internal effects; reducers never touch trees directly.
- **Flow**: Reducer emits intent → Manifest trigger starts Plan → Plan performs effects → Plan raises result event → Reducer advances state.
- **Rule**: NEVER orchestrate http/llm/payments/email in reducers. NEVER put business logic in plans. Keep responsibilities clear.

**Workflow patterns** (see spec/07-workflow-patterns.md):
- **Single-plan**: One plan orchestrates full flow (best for governance/audit)
- **Multi-plan**: Event-driven choreography (best for service boundaries)
- **Reducer-driven**: Reducer owns state machine, plans are thin wrappers (best for complex business logic)
- **Hybrid**: Plan orchestrates, reducer tracks (best for high-value workflows needing both)

## Key Principles

1. Determinism by default (replay-identical state)
2. Capability security (no ambient authority)
3. Receipts everywhere (signed, auditable)
4. Minimal trusted base
5. Content-addressed, portable worlds

## Implementation Notes

**Testing invariant**: "Replay-or-die" - replay from genesis must produce byte-identical snapshots.

**Key implementation notes**:
- Loader must accept both JSON lenses (authoring sugar and canonical JSON), validate against schemas, and emit canonical CBOR
- Validator enforces semantic checks: DAG acyclicity, capability bindings, policy compliance, effect allowlists
- Plan executor evaluates expressions, guards edges, awaits receipts deterministically
- Effect manager routes intents through policy gates, invokes adapters, validates receipt signatures
- Event ingress is normalized like effect params: every DomainEvent/ReceiptEvent is schema-validated, canonicalized once, journaled as canonical CBOR, and routing/correlation uses the schema-decoded value
- Manifest changes are journaled as `Manifest` records; replay applies them in order to keep control plane state aligned.
- Module build/cache: reducers compiled via `aos-wasm-build`, cached under `.aos/cache/{modules|wasmtime}`; kernel can warm-load cached compiled modules.
- Workspace sync uses `aos.sync.json` plus `aos push`/`aos pull`; filesystem names are encoded per segment with `~`-hex when needed.
- See `spec/02-architecture.md` for runtime components and `spec/03-air.md` for AIR semantics

## Project Structure (Rust workspace, edition 2024)

Crates keep deterministic core small and effectful code at the edges:

- `aos-air-types` — AIR data types + semantic validation (DAG, refs, bindings, schemas).
- `aos-air-exec` — Pure expression/value evaluator for plan predicates/bindings.
- `aos-cbor` — Canonical CBOR + SHA-256 helpers.
- `aos-store` — Content-addressed store + manifest loader utilities.
- `aos-effects` — Effect intent/receipt types and adapter traits.
- `aos-kernel` — Deterministic stepper, governance (proposal/shadow/approve/apply), plan executor, policy/cap ledgers, journal/snapshots.
- `aos-wasm-abi` — no_std envelopes shared by reducers/pure components.
- `aos-wasm` — Deterministic Wasmtime wrapper for reducers.
- `aos-wasm-sdk` — Reducer helper library for `wasm32-unknown-unknown`.
- `aos-wasm-build` — Deterministic reducer compiler + cache.
- `aos-host` — WorldHost runtime + TestHost test harness + fixtures (with `e2e-tests` feature).
- `aos-smoke` — CLI runners for numbered demos in `crates/aos-smoke/fixtures/`.

Planned adapters: `aos-adapter-http`, `aos-adapter-llm`, `aos-adapter-fs`, `aos-adapter-timer`.

## Test Strategy (Concise, Deterministic)

- Unit tests live next to code: place `mod tests` at the bottom of the same file with `#[cfg(test)]`. Keep them short, one behavior per test.
- Integration tests go under `tests/` when they cross crate boundaries, hit I/O, spawn the kernel stepper, or involve adapters. Use `aos-host` fixtures (enable `e2e-tests` feature).
- Naming: use `function_under_test_condition_expected()` style; structure as arrange/act/assert. Prefer explicit inputs over shared mutable fixtures.
- Determinism: no wall-clock or randomness in tests. If needed, use seeded RNG and deterministic clock from `aos-host` fixtures.
- Errors: assert on error kinds/types (e.g., custom errors with `thiserror`) instead of string matching. Prefer `matches!`/`downcast_ref` over brittle text.
- Parallel-safe: tests run in parallel by default. Avoid global state and temp dirs without unique prefixes. Only serialize when necessary.
- Property tests (optional): add a small number of targeted property tests (e.g., canonical encoding invariants). Gate heavier fuzzing behind a feature.
- Doctests: keep crate-level examples compilable; simple examples belong in doc comments and are run with `cargo test --doc`.
- Snapshots/"goldens": for canonical CBOR and journals, store fixtures under `tests/data/`. Regenerate consciously; diff byte-for-byte to protect determinism.
- Replay-or-die: for kernel/plan tests, run once to produce a journal, then replay from genesis and assert byte-identical snapshots.
- Async tests: if needed, use `#[tokio::test(flavor = "current_thread")]` to keep scheduling deterministic.
- `aos-host` note: integration tests expect fixtures; run with `--features e2e-tests` when invoking `cargo test -p aos-host`.

## Examples Ladder (index)
- See `crates/aos-smoke/fixtures/README.md` for the numbered demos.
## Keeping Documentation Updated

**IMPORTANT**: When modifying specs or architecture:
1. Update the relevant spec files in `spec/`
2. Update this file (AGENTS.md or CLAUDE.md) if the high-level architecture changes
3. Note: CLAUDE.md is a symlink to AGENTS.md - they are the same file

The specs are the source of truth. This file is just an index.
