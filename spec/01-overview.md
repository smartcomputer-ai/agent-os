# AgentOS + AIR: Overview

AgentOS is a computing substrate for AI agents and humans to co‑author reliable, auditable software systems. It combines a deterministic, event‑sourced core (the "world") with a small, typed control‑plane IR called AIR (Agent Intermediate Representation). Heavy computation and integration live in sandboxed WASM modules and effect adapters; all external I/O is governed by explicit capabilities and produces signed receipts. The result is a runtime that can safely evolve itself under policy, while remaining portable and easy to reason about.

## Why Now

Agents are writing code and orchestrating services, but today they sit on stacks never designed for self‑modification. State sprawls across disparate systems, upgrades are ad hoc, and audits are partial at best. Meanwhile, the building blocks needed to fix this—deterministic containers, WASM runtimes, and content‑addressable storage—have matured to the point where we can draw a crisp line between pure, replayable logic and side‑effects with receipts.

At the same time, organizations need governed automation: proposals that can be rehearsed in shadow runs, least‑privilege approvals, and full provenance for every change. AgentOS makes these patterns first‑class rather than bolted on.

## The Problem

Modern "agent systems" are an accumulation of scripts, queues, functions, and SaaS APIs with ambient authority. The result is a mess of interrelated problems.

**Non‑deterministic execution** means time, randomness, and network IO leak into core logic, making replay unreliable. **State fragmentation** scatters code, config, schemas, and policies across different tools and formats, so upgrades risk data drift. **Weak audit trails** mean effects happen without durable receipts, turning incident forensics into guesswork. And **governance fatigue** sets in because approvals and budgets are out‑of‑band and hard to enforce consistently.

## The Approach

AgentOS treats each running system as a world: a single‑threaded, replayable event log with periodic snapshots. All changes—code, schemas, policies, capabilities, and plans—are expressed in AIR, a small, typed IR the kernel can validate and execute deterministically. Application logic runs as WASM modules (reducers for state machines, pure components for pure functions). Any interaction with the outside world is an explicit effect, executed by adapters and recorded as a signed receipt. Risky changes are rehearsed in a shadow run before apply.

In short: propose → shadow → approve → apply → execute → receipt → audit.

## Core Principles

**Determinism by default.** Replaying from the log yields identical state every time; time and I/O only enter at the effect layer.

**Homoiconic in spirit.** AIR is a canonical, typed representation for modules, plans, schemas, policies, and capabilities that agents can read and edit programmatically.

**Capability security.** No ambient authority; all effects are scoped, budgeted, and policy‑gated.

**Receipts everywhere.** Every external effect yields a signed receipt, so audits can reconstruct complete cause→effect chains.

**Portability.** Worlds are content‑addressed bundles that can be moved, forked, or replayed anywhere.

**Minimal trusted base.** Keep the kernel small; push complexity to WASM modules and adapters with typed boundaries.

## What AgentOS Is (and Is Not)

AgentOS **is** a deterministic, event‑sourced kernel with receipts and capabilities. It **is** a unified control plane (AIR) for plans, policies, schemas, modules, and capability grants. And it **is** a safe home for agents to propose, simulate, and apply their own upgrades under policy.

AgentOS **is not** a general‑purpose programming language—compute lives in your language of choice compiled to WASM. It **is not** a blockchain or consensus layer, nor a replacement for your network stack. And it **is not** a traditional mutable database; state is derived from events, while effects and adapters handle heavy I/O and indexing.

## Mental Model

A **world** is the unit of computation and ownership. It processes one event at a time, which makes reasoning about it straightforward. Horizontal scaling comes from running many worlds.

**AIR** is the typed, canonical "blueprint" for a world's control plane: modules, plans, schemas, policies, and capabilities.

**Modules** are WASM artifacts. Reducers handle state transitions; pure components perform pure computation.

**Effects and adapters** represent explicit external actions. Adapters execute effects and produce signed receipts that feed back into the log.

**Policy and capabilities** are declarative rules and scoped tokens that gate effects and plans. Budgets are checked conservatively before dispatch and settle precisely on receipt.

## Why AIR matters

Previous stacks scattered control‑plane intent across config files, workflow engines, and policy tools. AIR unifies these into one small, typed object space the kernel understands and can simulate. That makes least‑privilege checks, rehearsals, diffs, and audits straightforward and first‑class.

## First Version Scope

The first version ships with a single‑threaded world with an append‑only journal and snapshots. AIR v1 defines five core forms: **defschema**, **defmodule**, **defplan**, **defcap**, and **defpolicy**, along with canonical encoding and typed patches. Deterministic WASM execution powers reducers and pure modules. Four adapters—HTTP, blob/FS, timer, and LLM—each come with capabilities and signed receipts. The constitutional loop and shadow runs protect the apply step, and a provenance "why graph" connects effects to state.

Migrations, multi‑world fabric, and complex policy engines are deferred to later versions. The architecture leaves clean hooks for them.

## Who It's For

AgentOS is for teams building governed automations and agentic workflows that must be auditable and reversible. It's for individual developers who want portable, forkable "worlds" they can share and replay. And it's for organizations that need least‑privilege capability gating and cost/budget controls for LLMs and APIs.



