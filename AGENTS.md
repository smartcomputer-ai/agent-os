# CLAUDE.md

This file provides guidance to coding agents when working with code in this repository.

## What This Is

**AgentOS** is a deterministic, event-sourced computing substrate for AI agents. **AIR (Agent Intermediate Representation)** is the typed control-plane IR governing modules, plans, schemas, policies, and capabilities.

**Current state**: Specification-only repository. No implementation exists yet.

## Reading the Specs

**Read specs in this order:**

1. **spec/01-overview.md** - Core concepts, mental model, why this exists
2. **spec/02-architecture.md** - Runtime components, event flow, storage layout
3. **spec/03-air.md** - **CRITICAL**: Complete AIR v1 spec (schemas, modules, plans, capabilities, policies)
4. **spec/04-reducers.md** - Reducer semantics, ABI, relationship to plans
5. **spec/07-workflow-patterns.md** - How to coordinate complex workflows (patterns, compensations, retries)
6. **spec/05-cells.md** - Keyed reducers (v1.1)
8. **spec/06-parallelism.md** - Future direction (deferred)
9. **spec/10-air-implementation.md** - Rust implementation guide with code skeletons

**spec/schemas/** - JSON Schemas for AIR node validation

## Core Architecture (TL;DR)

**World**: Single-threaded deterministic event log. Replay journal + receipts = identical state.

**Three layers**:
- **Reducers** (WASM state machines): Domain logic, business invariants, emit events. May emit micro-effects (timer, blob) ONLY. See spec/04-reducers.md
- **Plans** (DAG orchestration): Multi-step effect workflows under governance. All risky effects (http, llm, payments, email). See spec/03-air.md §11
- **Effects/Adapters**: Execute external actions, return signed receipts. See spec/02-architecture.md

**Governance**: propose → shadow → approve → apply → execute → receipt → audit

**Critical boundaries (v1)**:
- **Reducers**: Own state and business logic. Emit DomainIntent events for external work. May emit at most ONE micro-effect per step (blob.{put,get}, timer.set). NO network effects.
- **Plans**: Orchestrate effects (http, llm, payments, email) triggered by intents. Raise result events back to reducers. NO compute or business logic.
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

## Implementation Path (if building)

See **spec/03b-air-implementation.md** for detailed Rust implementation guide.

**Build order**: CBOR+hashing → store/loader → validator → WASM runner → effect manager → plan executor → governance loop → shadow-run

**Testing invariant**: "Replay-or-die" - replay from genesis must produce byte-identical snapshots.

## Project Structure (Rust Workspace)

All crates use Rust edition 2024. Crates live under `crates/` and are organized to keep deterministic core small and effectful code at the edges.

- `aos-air-types` — AIR data types and semantic validation. Bundles JSON Schemas, Expr AST, and checks (DAG, references, bindings).
- `aos-air-exec` — Pure, deterministic expression/value evaluator used by plan predicates and bindings.
- `aos-cbor` — Canonical CBOR encode/decode and SHA-256 hashing helpers used across the stack.
- `aos-store` — Content-addressed store primitives and (later) manifest loader utilities.
- `aos-wasm-abi` — Shared no_std envelopes for reducer/pure-component ABIs (kernel and SDK share these types).
- `aos-wasm` — Deterministic Wasm runner wrapper (wasmtime profile, reducer ABI integration).
- `aos-effects` — Effect intent and receipt types plus adapter-facing traits.
- `aos-kernel` — Deterministic stepper, plan executor, policy/capability gates, journal/snapshots.
- `aos-wasm-sdk` — Reducer-side helper library targeting `wasm32-unknown-unknown` (entry wrapper, micro-effect helpers).
- `aos-testkit` — In-memory store/adapters, deterministic clock/RNG, replay harness; for tests and shadow runs.
- `aos-cli` — Operational tooling: init world, run loop, tail journal; wires adapters via features.

Optional adapters (planned as separate crates):
- `aos-adapter-http`, `aos-adapter-llm`, `aos-adapter-fs`, `aos-adapter-timer` — Concrete adapter implementations. Keep async/provider deps out of the kernel.

## Test Strategy (Concise, Deterministic)

- Unit tests live next to code: place `mod tests` at the bottom of the same file with `#[cfg(test)]`. Keep them short, one behavior per test.
- Integration tests go under `tests/` when they cross crate boundaries, hit I/O, spawn the kernel stepper, or involve adapters. Use `aos-testkit` fixtures.
- Naming: use `function_under_test_condition_expected()` style; structure as arrange/act/assert. Prefer explicit inputs over shared mutable fixtures.
- Determinism: no wall-clock or randomness in tests. If needed, use seeded RNG and deterministic clock from `aos-testkit`.
- Errors: assert on error kinds/types (e.g., custom errors with `thiserror`) instead of string matching. Prefer `matches!`/`downcast_ref` over brittle text.
- Parallel-safe: tests run in parallel by default. Avoid global state and temp dirs without unique prefixes. Only serialize when necessary.
- Property tests (optional): add a small number of targeted property tests (e.g., canonical encoding invariants). Gate heavier fuzzing behind a feature.
- Doctests: keep crate-level examples compilable; simple examples belong in doc comments and are run with `cargo test --doc`.
- Snapshots/"goldens": for canonical CBOR and journals, store fixtures under `tests/data/`. Regenerate consciously; diff byte-for-byte to protect determinism.
- Replay-or-die: for kernel/plan tests, run once to produce a journal, then replay from genesis and assert byte-identical snapshots.
- Async tests: if needed, use `#[tokio::test(flavor = "current_thread")]` to keep scheduling deterministic.

Example unit test layout (in-file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_canonical_cbor_is_stable() {
        let bytes1 = to_canonical_cbor(&serde_json::json!({"a":1,"b":2})).unwrap();
        let bytes2 = to_canonical_cbor(&serde_json::json!({"b":2,"a":1})).unwrap();
        assert_eq!(bytes1, bytes2);
    }
}
```

Example integration test (under `tests/`):

```rust
#[test]
fn replay_is_byte_identical() {
    // arrange: build a tiny world with testkit
    // act: run plan once (adapters stubbed), persist journal
    // assert: replay from genesis yields identical snapshot bytes
}
```

## Keeping Documentation Updated

**IMPORTANT**: When modifying specs or architecture:
1. Update the relevant spec files in `spec/`
2. Update this file (AGENTS.md or CLAUDE.md) if the high-level architecture changes
3. Note: CLAUDE.md is a symlink to AGENTS.md - they are the same file

The specs are the source of truth. This file is just an index.
