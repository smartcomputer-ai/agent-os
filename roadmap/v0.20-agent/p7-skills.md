# P7: Skills as an Implementation-Layer Feature

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (worlds will keep inventing ad hoc prompt bundles and repo-instruction loading, but the more critical context/session seams can still land first)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.16-factory/p5-context-engine.md`, `roadmap/v0.16-factory/p6-session-management-improvements.md`

## Goal

Define skills after the context engine and session model are in place, and keep them above the core session SDK.

Primary outcome:

1. skills become a reusable implementation-layer concept,
2. skill sources can come from workspaces, CAS refs, static assets, or repo-local files,
3. the context engine can consume resolved skills without knowing how they were stored,
4. `aos-agent` core remains skill-agnostic.

## Problem Statement

The system clearly needs a notion of reusable agent capability bundles:

1. coding instructions,
2. repo-local guidance such as `AGENTS.md`,
3. prompt modules,
4. recommended tool/profile sets,
5. maybe future memories or examples.

But if we define skills too early, they will accidentally become:

1. a hidden workspace feature,
2. a hidden prompt-pack feature,
3. a hidden session primitive,
4. or a hardcoded Demiurge behavior.

That would repeat the same boundary mistake as the current workspace coupling.

## Design Stance

### 1) Skills live above the core session SDK

`aos-agent` core should not require a skill model to run a session.

Skills belong to:

1. embedding worlds,
2. context-source loaders,
3. context-engine policy,
4. optional tool/profile bundles.

### 2) A skill resolves to contributions, not to magic behavior

A resolved skill should contribute explicit things such as:

1. context refs,
2. tool-profile hints,
3. tool enable/disable overrides,
4. structured metadata for inspection,
5. optional guardrails or activation conditions.

The session kernel should consume those results through normal context and config paths.

### 3) Skill storage is source-agnostic

Possible sources include:

1. workspace files,
2. blob refs in CAS,
3. static files shipped with a world,
4. repo-local instruction files such as `AGENTS.md`,
5. future registries or package stores.

No one source should be privileged by the core contracts.

### 4) Skill activation is a world policy choice

Activation may come from:

1. world defaults,
2. session defaults,
3. run overrides,
4. repo detection logic,
5. explicit operator input.

That policy belongs above the core session kernel.

### 5) Skills should integrate through the context engine

The clean composition is:

1. skill source resolves to structured contributions,
2. context engine decides what to include for a run,
3. session kernel consumes the resulting context and tool config.

This keeps skills from becoming a second hidden prompt system.

## Scope

### [ ] 1) Define skill descriptor and resolved-contribution contracts

Add types for:

1. skill identity,
2. source metadata,
3. activation metadata,
4. resolved context refs,
5. resolved tool/profile contributions,
6. inspection/debug info.

These contracts should be small and implementation-oriented.

### [ ] 2) Add a skill-resolution seam above the context engine

Recommended direction:

1. skill resolver loads and normalizes sources,
2. context engine receives normalized resolved skills,
3. session kernel remains unaware of raw skill storage.

### [ ] 3) Support repo-local instruction files as one source

The first practical proof should include repo-local files such as:

1. `AGENTS.md`,
2. future `SOUL.md` or equivalent conventions,
3. world-provided instruction files.

Important rule:

1. these files are one skill source,
2. they are not the universal core model.

### [ ] 4) Support workspace-backed skills as an opt-in source

Workspace-backed skills are still useful, especially for versioned shared packs.

They should remain possible, but only as an implementation-layer source plugged into the resolver.

### [ ] 5) Add observability for skill selection

We need to be able to answer:

1. which skills were active,
2. which contributions they produced,
3. which contributions were actually selected into the run context,
4. why a skill was ignored or dropped.

### [ ] 6) Prove the model in Demiurge or a focused fixture

The first proof should demonstrate:

1. explicit skill activation,
2. repo-local instruction loading,
3. context-engine selection of resolved skill contributions,
4. tool/profile effects remaining explicit and inspectable.

## Non-Goals

P7 does **not** attempt:

1. a skill marketplace,
2. skill version negotiation across universes,
3. subagent skill inheritance,
4. a new package manager,
5. hiding skill effects from the context engine or run config.

## Deliverables

1. Skill descriptor and resolved-contribution contracts.
2. Skill resolver seam above the context engine.
3. Repo-local instruction-file support as a concrete source.
4. Optional workspace-backed skill source.
5. Skill-selection observability.

## Acceptance Criteria

1. Skills are not required for core session execution.
2. Repo-local instruction files can participate as skills without becoming the universal storage model.
3. Workspace-backed skills remain possible without reintroducing workspace coupling into `aos-agent` core.
4. Skill resolution feeds the context engine through normalized contributions.
5. A representative world or fixture proves end-to-end skill activation and inspection.

## Recommended Implementation Order

1. define the skill descriptor and resolved-contribution model,
2. add the skill-resolution seam above the context engine,
3. implement repo-local instruction files as the first practical source,
4. add workspace-backed skills as an optional source,
5. add inspection/reporting,
6. prove the model in Demiurge or a focused fixture.
