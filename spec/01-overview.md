# AgentOS Overview

AgentOS is a computing substrate for AI agents and humans to co‑author reliable, auditable software systems. It combines a deterministic, event‑sourced core (the "world") with a small, typed control‑plane IR called AIR (Agent Intermediate Representation). Heavy computation and integration live in sandboxed WASM modules and effect adapters; all external I/O is governed by explicit capabilities and produces signed receipts. The result is a runtime that can safely evolve itself under policy, while remaining portable and easy to reason about.

## Who It's For

AgentOS is for teams building governed automations and agentic workflows that must be auditable and reversible. It's for individual developers who want portable, forkable "worlds" they can share and replay. And it's for organizations that need least‑privilege capability gating and deterministic audit trails for LLMs and APIs.

AgentOS is currently in early development. The architecture is being designed in the open, and we invite feedback, contributions, and collaboration from anyone interested in building deterministic substrates for agents. _If this vision resonates with you, join us in shaping what agent-native computing should look like._


## Why Now

Agents are writing code and orchestrating services, but today they sit on stacks never designed for self‑modification. State sprawls across disparate systems, upgrades are ad hoc, and audits are partial at best. Meanwhile, the building blocks needed to fix this—deterministic containers, WASM runtimes, and content‑addressable storage—have matured to the point where we can draw a crisp line between pure, replayable logic and side‑effects with receipts.

At the same time, organizations need governed automation: proposals that can be rehearsed in shadow runs, least‑privilege approvals, and full provenance for every change. AgentOS makes these patterns first‑class rather than bolted on.

## History: Living Systems and Deterministic Substrates

Early computing environments such as **Lisp Machines** were remarkable not for their hardware speed but for their *integrity*. Every component of the system—editor, compiler, window system, even device drivers—was live data within the same image. Programs could examine and modify themselves. Code and data were represented in the same structures, and the boundary between user, system, and language was thin. It created a sense of *living computation*.

These environments eventually lost to more modular and performant systems like Unix and commodity hardware. They were expensive, non‑standard, and difficult to distribute. But they represented a purer vision of computation as a *dynamic, introspectable, self‑modifying space*.

**Urbit** later revived this aspiration in a new guise: a self‑contained, deterministic, addressable personal computer running in a sandboxed network. Urbit's design offered determinism, upgradability, and identity, but it targeted human operators rather than machine agents. Yet its underlying ideas—deterministic kernels, functional reducers, and a persistent event log—remain valuable. The Urbit whitepaper framed this approach as a ["solid state interpreter" and "operating function"](https://media.urbit.org/whitepaper.pdf), terminology that captures what AgentOS aims to provide for agents.

Today we face a new incarnation of the same tension between *dynamic, self‑modifying systems* and *practical engineering*. Large Language Models (LLMs) and autonomous agents can generate code, migrate state, and act on our behalf—but they currently sit atop traditional stacks that were never designed for self‑referential evolution. Agents must glue together scripts, services, and APIs that each have their own state and persistence models. They can't truly *own* their runtime; they supervise it from above.

The question is: **what if the runtime itself were designed for agents?** What if agents could propose, simulate, and apply changes to their own world safely, down to the kernel, without leaving a coherent, deterministic substrate?

## The Problem

Modern "agent systems" are an accumulation of scripts, queues, functions, and SaaS APIs with ambient authority. The result is a mess of interrelated problems.

**Non‑deterministic execution** means time, randomness, and network IO leak into core logic, making replay unreliable. **State fragmentation** scatters code, config, schemas, and policies across different tools and formats, so upgrades risk data drift. **Weak audit trails** mean effects happen without durable receipts, turning incident forensics into guesswork. And **governance fatigue** sets in because approvals and controls are out‑of‑band and hard to enforce consistently.

Agents deserve better. They need a substrate that is deterministic by default, unified in its control plane, auditable at every boundary, and safe to evolve.

## The Approach

AgentOS treats each running system as a world: a single‑threaded, replayable event log with periodic snapshots. All changes—code, schemas, policies, capabilities, and plans—are expressed in AIR, a small, typed IR the kernel can validate and execute deterministically. Application logic runs as WASM modules (reducers for state machines, pure components for pure functions). Any interaction with the outside world is an explicit effect, executed by adapters and recorded as a signed receipt. Risky changes are rehearsed in a shadow run before apply.

In short: `propose → shadow → approve → apply → execute → receipt → audit`.

## Core Principles

AgentOS is designed around six core principles that enable safe, governed self‑modification:

1. **Determinism by default.** Any computation within a world produces the same results when replayed from its log. Time and I/O only enter at the effect layer, where they are captured as receipts. This makes debugging, auditing, and reasoning about behavior tractable.

2. **Homoiconic in spirit.** Everything that defines the world, such as code, schemas, policies, UI, capabilities, is represented as structured data the system can inspect and modify. AIR is a canonical, typed representation for modules, plans, schemas, policies, and capabilities that agents can read and edit programmatically.

3. **Capability security.** No ambient authority. All effects are scoped by explicit capability tokens and policy‑gated. Least‑privilege is enforced mechanically, not by convention.

4. **Receipts everywhere.** Every external effect yields a signed receipt. Audits can reconstruct complete cause→effect chains from log to receipt to state change. Incident forensics become deterministic replay rather than guesswork.

5. **Portability and composability.** Worlds are self‑contained, content‑addressed bundles that can be moved, forked, or replayed anywhere. Modules interact through typed events with clear state boundaries, making composition predictable.

6. **Minimal trusted base.** Keep the kernel small and auditable; push complexity to WASM modules and adapters with typed boundaries. The trusted core validates, routes, and enforces policy, nothing more.

## What AgentOS Is (and Is Not)

AgentOS **is** a deterministic, event‑sourced kernel with receipts and capabilities. It **is** a unified control plane (AIR) for plans, policies, schemas, modules, and capability grants. And it **is** a safe home for agents to propose, simulate, and apply their own upgrades under policy.

AgentOS **is not** a general‑purpose programming language, compute lives in your language of choice compiled to WASM. It **is not** a blockchain or consensus layer, nor a replacement for your network stack. And it **is not** a traditional mutable database; state is derived from events, while effects and adapters handle heavy I/O and indexing.

## Mental Model

- **World**: The fundamental unit of computation and ownership. A world consists of:
  - An append‑only event log that drives all state changes
  - A materialized state snapshot after replaying the log
  - A set of WASM modules (reducers) that define how state evolves
  - A structured control plane (AIR) that describes what exists and how it connects

  A world processes one event at a time, which makes reasoning about it straightforward. Horizontal scaling comes from running many worlds. Worlds can be forked, replayed, shadow‑run, rolled back, and exported as snapshots.

- **AIR**: The typed, canonical "blueprint" for a world's control plane. It describes modules, plans, schemas, policies, and capabilities as structured data the kernel can inspect, validate, and simulate. Agents or humans modify AIR by proposing plans (diffs), which can be simulated and then applied, producing new log events and snapshots.

- **Modules**: WASM artifacts. **Reducers** are deterministic state machines with a canonical signature: they consume an event and current state, then return new state and a list of effect intents. They cannot access the outside world directly; the host system executes effects if allowed by policy and capability. Pure components perform side‑effect‑free computation.

- **Effects and adapters**: Explicit external actions—sending an email, calling an API, invoking an LLM, storing a blob. Each effect type is declared in the capability catalog and implemented by an external adapter. When an effect executes, the adapter produces a **receipt**—a signed record of what happened (parameters, response hash, timestamp, cost). Receipts are appended as events to the world log, preserving determinism on replay.

- **Policy and capabilities**: Declarative rules and scoped tokens that gate effects and plans. Capabilities define what kinds of effects are allowed (HTTP to specific hosts, LLM with model/token constraints, blob storage). Policies evaluate allow/deny decisions based on effect kind, capability, and origin (plan vs reducer). Budget enforcement is deferred to a future milestone.

- **Agents**: Interact with the world through the same protocol as humans: they read the current AIR and state, generate new code (reducers, plans, or even policies), compile to WASM, draft a plan describing the changes and new module hashes, and submit the plan as a proposal. Agents can live outside the world (simpler) or inside it as privileged modules that can propose but not unilaterally apply plans.

## First Version Scope

The first version ships with a single‑threaded world with an append‑only journal and snapshots. AIR v1 defines five core forms: **defschema**, **defmodule**, **defplan**, **defcap**, and **defpolicy**, along with canonical encoding and typed patches. Deterministic WASM execution powers reducers. Pure modules are planned for v1.1+; `module_kind` is `"reducer"` only in v1. Built-in adapters cover HTTP, blob/FS, timer, LLM, and kernel-resident introspection (`introspect.*`) guarded by the `query` capability. Each comes with capabilities and signed receipts. Version 1.1 adds first-class Cells (keyed reducers) with per-key state and mailboxes. The constitutional loop and shadow runs protect the apply step, and a provenance "why graph" connects effects to state.

Migrations, multi‑world fabric, and complex policy engines are deferred to later versions. The architecture leaves clean hooks for them.
