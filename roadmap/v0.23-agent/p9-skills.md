# P9: Skills as an Implementation-Layer Feature

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (worlds will keep inventing ad hoc prompt bundles and repo-instruction loading, but the core session/context/trace seams can land first)  
**Status**: Complete for v0.30 SDK scope; full skill descriptors, resolvers, source loaders, and harness proof are deferred implementation-layer work  
**Depends on**: `roadmap/v0.23-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.23-agent/p6-turn-planner.md`, `roadmap/v0.23-agent/p7-run-traces-and-intervention.md`

## Goal

Define skills after the tool, session, context, and trace seams are in place, and keep them above the core session SDK.

Primary outcome:

1. skills become a reusable implementation-layer concept,
2. skill sources can come from repo-local files, workspaces, CAS refs, or static assets,
3. resolved skills feed the turn planner through normalized contributions,
4. tool/profile effects remain explicit and inspectable,
5. `aos-agent` core can run without any skill model.

For the v0.30 core push, do not expand this into a required SDK subsystem. Repo-local instruction
files, workspace context packs, and factory playbooks can first participate as ordinary P6 context
inputs. Full skill descriptors, activation, versioning, and contribution reporting can follow after
the core seams are stable.

## Problem Statement

The system needs reusable agent behavior bundles:

1. coding instructions,
2. repo-local guidance such as `AGENTS.md`,
3. prompt modules,
4. recommended tool/profile sets,
5. examples or memories,
6. optional activation conditions.

But if skills land too early, they can accidentally become:

1. a hidden workspace feature,
2. a hidden prompt-pack feature,
3. a hidden session primitive,
4. a hardcoded Demiurge behavior.

That would repeat the boundary mistake P4 is meant to fix.

## Design Stance

### 1) Skills live above the core session SDK

`aos-agent` core should not require a skill model to run a session.

Skills belong to:

1. embedding worlds,
2. context-source loaders,
3. context-engine selection,
4. optional tool/profile bundle assembly,
5. operator/debug surfaces.

### 2) A skill resolves to explicit contributions

A resolved skill should contribute structured data such as:

1. context refs,
2. tool-profile hints,
3. tool enable/disable suggestions,
4. advisory instruction refs,
5. examples,
6. inspection metadata,
7. activation reasoning.

The session kernel should consume those through normal context and run config paths.

### 3) Skill storage is source-agnostic

Possible sources include:

1. repo-local instruction files,
2. workspace files,
3. blob refs in CAS,
4. static files shipped with a world,
5. future registries or package stores.

No one source should be privileged by core contracts.

### 4) Activation is world-owned logic

Activation may come from:

1. world defaults,
2. session defaults,
3. run overrides,
4. repo detection,
5. explicit operator input.

The activation logic belongs above the core session kernel.

### 5) Skills integrate through turn planning and tools

The clean flow is:

1. skill source resolves to structured contributions,
2. turn planner chooses what to include for a run,
3. tool/profile contributions are applied explicitly,
4. run trace records active skills and selected contributions.

This keeps skills from becoming a second hidden prompt system.

## v0.30 Decision

Do not add a first-class skill subsystem to `aos-agent` core for v0.30.

The SDK already has the right integration seams:

1. resolved skill instructions can enter the planner as `TurnObservation::InputObserved` with `TurnInputLane::Skill`,
2. skill-provided prompt/message refs are ordinary `TurnInput` values selected or dropped by the turn planner,
3. skill tool suggestions can be applied through explicit session/run tool profile, enable, disable, or force config,
4. `TurnPlanned` traces show which inputs and tools were selected without adding a hidden skill prompt path,
5. embedding worlds, Demiurge, or future harnesses can implement repo/workspace/CAS/static skill resolution above the SDK.

The important boundary is that skills must not bypass turn planning. A resolver may produce normalized
contributions, but it should not directly mutate LLM request message refs or tool refs.

## Scope

### [x] 1) Defer skill descriptor and contribution contracts

Add small contracts for:

1. skill identity,
2. source metadata,
3. activation metadata,
4. context contributions,
5. tool/profile contributions,
6. inspection/debug info.

Deferred:

1. core session execution does not need skill descriptors.
2. future descriptors should live above the session reducer and resolve into existing turn/tool config inputs.
3. no skill storage source should become privileged in core contracts.

### [x] 2) Use the existing turn-planner seam

Required outcome:

1. resolver loads and normalizes skill sources,
2. turn planner receives normalized contributions,
3. session kernel remains unaware of raw skill storage,
4. run trace can report active and dropped skills.

Done for SDK scope:

1. `TurnInputLane::Skill` exists for resolved skill contributions.
2. `TurnObservation::InputObserved` can add durable planner inputs without a new skill-specific reducer path.
3. `TurnToolInput` and explicit tool profile/override config cover tool suggestions.
4. `TurnPlanned` traces record selected message refs and selected tools.

Deferred:

1. a concrete resolver interface,
2. active/dropped skill reporting beyond normal turn-plan trace data,
3. cross-source loading and activation behavior.

### [x] 3) Defer repo-local instruction files

The first practical proof should include repo-local files such as:

1. `AGENTS.md`,
2. future `SOUL.md` or equivalent conventions,
3. world-provided instruction files.

These files are one skill source, not the universal core model.

Deferred:

1. repo-local loaders belong in an embedding world, Demiurge, or a harness/resolver crate.
2. once loaded, their instruction refs should be submitted as skill-lane `TurnInput`s.

### [x] 4) Defer workspace-backed skills as opt-in

Workspace-backed skills are useful for versioned shared packs.

They should remain possible, but only as an implementation-layer source plugged into the resolver.

Deferred:

1. workspace-backed skills remain compatible with the same normalized contribution path.
2. core `aos-agent` should not depend on workspace skill storage.

### [x] 5) Keep observability in the existing trace path

We need to answer:

1. which skills were active,
2. why they were active,
3. which contributions they produced,
4. which contributions entered the turn plan,
5. which tool/profile suggestions were applied or ignored.

Done for SDK scope:

1. selected skill-lane inputs are observable as selected refs in `TurnPlanned`.
2. selected skill-provided tools are observable as selected tool ids in `TurnPlanned`.
3. richer active/dropped skill diagnostics can be added later as resolver-owned metadata or custom trace entries.

### [x] 6) Defer end-to-end skill proof

The first proof should demonstrate:

1. explicit skill activation,
2. repo-local instruction loading,
3. context-engine selection of resolved skill contributions,
4. tool/profile effects remaining explicit,
5. trace inspection of skill decisions.

This should preferably be an `aos-harness-py` deterministic fixture, with Demiurge integration as
a later consumer proof if needed.

Deferred:

1. deterministic end-to-end skill activation belongs with P10/harness work.
2. Demiurge integration is a consumer proof, not an SDK prerequisite.

## Non-Goals

P9 does **not** attempt:

1. a skill marketplace,
2. skill version negotiation across universes,
3. subagent skill inheritance,
4. a new package manager,
5. policy/capability gating or approval semantics,
6. hidden skill effects outside context/run config.

## Acceptance Criteria

1. [x] Skills are not required for core session execution.
2. [x] Repo-local instruction files can participate as skills without becoming the universal storage model.
3. [x] Workspace-backed skills remain possible without reintroducing workspace coupling.
4. [x] Skill resolution feeds the turn planner through normalized contributions.
5. [x] Tool/profile contributions are explicit and inspectable.
6. [x] End-to-end `aos-harness-py` skill activation and trace visibility are deferred to P10/harness work.
