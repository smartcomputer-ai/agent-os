# AgentOS + AIR: Overview

AgentOS is a computing substrate for AI agents and humans to co‑author reliable, auditable software systems. It combines a deterministic, event‑sourced core (the “world”) with a small, typed control‑plane IR called AIR (Agent Intermediate Representation). Heavy computation and integration live in sandboxed WASM modules and effect adapters; all external I/O is governed by explicit capabilities and produces signed receipts. The result is a runtime that can safely evolve itself under policy, while remaining portable and easy to reason about.

## Why Now

- Agents are writing code and orchestrating services, but today they sit on stacks never designed for self‑modification. State sprawls, upgrades are ad hoc, and audits are partial.
- Deterministic containers, WASM runtimes, and content‑addressable storage make it practical to draw a crisp line between pure, replayable logic and side‑effects with receipts.
- Organizations need governed automation: proposals, rehearsal/shadow simulation, least‑privilege approvals, and full provenance for every change.

## The Problem

Modern “agent systems” are an accumulation of scripts, queues, functions, and SaaS APIs with ambient authority:

- Non‑deterministic execution: time, randomness, and network IO leak into core logic; replay is unreliable.
- State fragmentation: code, config, schemas, and policies live in different tools and formats; upgrades risk data drift.
- Weak audit: effects happen without durable receipts; incident forensics are guesswork.
- Governance fatigue: approvals and budgets are out‑of‑band and hard to enforce consistently.

## The Approach

AgentOS treats each running system as a world: a single‑threaded, replayable event log with periodic snapshots. All changes—code, schemas, policies, capabilities, and plans—are expressed in AIR, a small, typed IR the kernel can validate and execute deterministically. Application logic runs as WASM modules (reducers for state machines, pure components for pure functions). Any interaction with the outside world is an explicit effect, executed by adapters and recorded as a signed receipt. Risky changes are rehearsed in a shadow run before apply.

In short: propose → shadow → approve → apply → execute → receipt → audit.

## Core Principles

- Determinism by default: replay from the log yields identical state; time/IO only at the effect layer.
- Homoiconic‑in‑spirit: AIR is a canonical, typed representation for modules, plans, schemas, policies, and capabilities that agents can edit.
- Capability security: no ambient authority; all effects are scoped, budgeted, and policy‑gated.
- Receipts everywhere: every external effect yields a signed receipt; audits reconstruct cause→effect chains.
- Portability: worlds are content‑addressed bundles that can be moved, forked, or replayed anywhere.
- Minimal trusted base: keep the kernel small; push complexity to WASM modules and adapters with typed boundaries.

## What AgentOS Is (and Is Not)

Is:

- A deterministic, event‑sourced kernel with receipts and capabilities.
- A unified control plane (AIR) for plans, policies, schemas, modules, and capability grants.
- A safe home for agents to propose, simulate, and apply their own upgrades under policy.

Is not:

- A general‑purpose programming language (compute lives in your language of choice compiled to WASM).
- A blockchain/consensus layer or a replacement for your network stack.
- A traditional mutable database (state is derived from events; effects and adapters handle heavy IO and indexing).

## Mental Model

- World: the unit of computation/ownership; one event at a time; horizontally scalable by running many worlds.
- AIR: the typed, canonical “blueprint” for the world’s control plane (modules, plans, schemas, policies, capabilities).
- Modules: WASM artifacts. Reducers handle state transitions; pure components perform pure computation.
- Effects and adapters: explicit external actions producing signed receipts that feed back into the log.
- Policy and capabilities: declarative rules and scoped tokens that gate effects and plans; budgets settle on receipt.

## Why AIR matters

Previous stacks scattered control‑plane intent across config files, workflow engines, and policy tools. AIR unifies these into one small, typed object space the kernel understands and can simulate. That makes least‑privilege checks, rehearsals, diffs, and audits straightforward and first‑class.

## First Version Scope (at a glance)

- Single‑threaded world with append‑only journal and snapshots.
- AIR v0: defschema, defmodule, defplan, defcap, defpolicy; canonical encoding and typed patches.
- Deterministic WASM execution for reducers and pure modules.
- Adapters: HTTP, blob/FS, timer, and LLM, each with capabilities and signed receipts.
- Constitutional loop and shadow runs before apply; provenance (“why graph”) for effects and state.

Migrations, multi‑world fabric, and complex policy engines are deferred; the architecture leaves clean hooks for them.

## Who It’s For

- Teams building governed automations and agentic workflows that must be auditable and reversible.
- Individual developers who want portable, forkable “worlds” they can share and replay.
- Organizations that need least‑privilege capability gating and cost/budget controls for LLMs and APIs.



