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
5. **spec/07-workflow-patterns.md** - **IMPORTANT**: How to coordinate complex workflows (patterns, compensations, retries)
6. **spec/05-cells.md** - Keyed reducers (v1.1)
7. **spec/06-parallelism.md** - Future direction (deferred)
8. **spec/10-air-implementation.md** - Rust implementation guide with code skeletons

**spec/schemas/** - JSON Schemas for AIR node validation

## Core Architecture (TL;DR)

**World**: Single-threaded deterministic event log. Replay journal + receipts = identical state.

**Three layers**:
- **Reducers** (WASM state machines): Domain logic, business invariants, emit events. May emit micro-effects (timer, fs.blob) ONLY. See spec/04-reducers.md
- **Plans** (DAG orchestration): Multi-step effect workflows under governance. All risky effects (http, llm, payments, email). See spec/03-air.md §11
- **Effects/Adapters**: Execute external actions, return signed receipts. See spec/02-architecture.md

**Governance**: propose → shadow → approve → apply → execute → receipt → audit

**Critical boundaries (v1)**:
- **Reducers**: Own state and business logic. Emit DomainIntent events for external work. May emit at most ONE micro-effect per step (fs.blob.{put,get}, timer.set). NO network effects.
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

## Keeping Documentation Updated

**IMPORTANT**: When modifying specs or architecture:
1. Update the relevant spec files in `spec/`
2. Update this file (AGENTS.md or CLAUDE.md) if the high-level architecture changes
3. Note: CLAUDE.md is a symlink to AGENTS.md - they are the same file

The specs are the source of truth. This file is just an index.
