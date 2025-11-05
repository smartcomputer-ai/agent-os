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
5. **spec/05-cells.md** - Keyed reducers (v1.1)
6. **spec/06-parallelism.md** - Future direction (deferred)
7. **spec/03b-air-implementation.md** - Rust implementation guide with code skeletons

**spec/schemas/** - JSON Schemas for AIR node validation

## Core Architecture (TL;DR)

**World**: Single-threaded deterministic event log. Replay journal + receipts = identical state.

**Three layers**:
- **Reducers** (WASM state machines): Domain logic, emit events. Micro-effects only (timer, fs.blob). See spec/04-reducers.md
- **Plans** (DAG orchestration): Multi-step workflows. All risky effects (http, llm, payments). See spec/03-air.md §11
- **Effects/Adapters**: Execute external actions, return signed receipts. See spec/02-architecture.md

**Governance**: propose → shadow → approve → apply → execute → receipt → audit

**Critical rule**: Reducers emit domain events (intents) → triggers start Plans → Plans do http/llm/payments → Plans raise result events → Reducers update state. NEVER orchestrate http/llm/payments in reducers.

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
