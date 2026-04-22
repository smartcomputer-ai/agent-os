# P4: Tool Bundle Refactoring for Agent Core

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (`aos-agent` core will keep hardening around one accidental default tool surface, which will make hosted agents, workspace-centric agents, and the context/session seams harder to design cleanly)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.16-factory/factory.md`, `roadmap/v0.10-agent-sdk/p4-agent-workflows.md`, `roadmap/v0.13-demiurge2/p2-workflow-authoring-primitives.md`

## Goal

Make `aos-agent` core tool-surface agnostic and move opinionated tools into
explicit bundles.

Primary outcome:

1. session and run contracts remain generic,
2. `aos-agent` core no longer implies one default user-visible tool surface,
3. host tools and workspace tools become symmetric optional bundles,
4. bundles stay inside `aos-agent` for now, but are explicitly selectable and extendable,
5. worlds such as Demiurge can either:
   - link `aos-agent` as a library and wrap it directly, or
   - keep using the evented `SessionWorkflow` adapter,
   without inheriting accidental tool assumptions by default.

## Problem Statement

The current codebase has already separated some important concerns:

1. session state and run config do not carry a workspace id,
2. prompt refs are hash refs rather than workspace paths,
3. session bootstrap can already be driven directly through library helpers.

But the shipped defaults still mix core agent behavior with one particular tool story:

1. the default tool registry and profiles bundle together host, query, and workspace-facing tools,
2. the tool runner contains special-cased bundle behavior rather than extension seams,
3. the default AIR bundle pulls in optional effect surfaces,
4. hosted agents that should have workspace tools but no host tools are not modeled cleanly,
5. chat-only or narrow domain agents still inherit a coding-agent shaped default.

That is the wrong boundary for the next steps.
The core session kernel should not care whether a world chooses:

1. no tools,
2. host tools,
3. workspace tools,
4. query/inspect tools,
5. some world-specific combination of those.

## Design Stance

### 1) Keep the session kernel tool-surface agnostic

`aos-agent` core should own:

1. session and run contracts,
2. transcript and tool-batch state,
3. LLM turn orchestration,
4. context-engine hooks,
5. generic tool planning and receipt settlement.

It should not assume that host tools, workspace tools, or any other bundle are present.

### 2) Keep tool bundles inside `aos-agent` for now

For now, the right home for the shipped bundles is still `aos-agent`.

Reasoning:

1. the bundle logic is tightly coupled to the existing tool contracts and reducer helpers,
2. splitting into separate crates now would add packaging churn before the extension seams are stable,
3. we still want one obvious place for reusable built-in bundles to live.

Recommended split inside `aos-agent`:

1. core session kernel,
2. built-in tool bundles,
3. thin assembly helpers for embedding worlds.

### 3) Treat host and workspace tooling symmetrically as optional bundles

The right principle is:

1. host is not core,
2. workspace is not core,
3. both are optional bundle choices.

Examples:

1. local coding agent:
   - inspect + host + maybe workspace.
2. hosted coding agent:
   - inspect + workspace, but no host.
3. chat-only agent:
   - no tools.
4. world-debug agent:
   - inspect + domain-specific tools.

The base defaults should not force one of those profiles.

### 4) Make bundle and per-tool selection explicit

We need two explicit layers of selection:

1. bundle selection
   - host,
   - workspace,
   - inspect/query,
   - future domain bundles.
2. per-tool selection
   - include/exclude specific tools within a bundle.

Recommended direction:

1. explicit bundle constructors in `aos-agent`,
2. explicit registry/profile builders,
3. explicit world-level assembly by the embedding world.

### 5) Keep the first API shape minimal and concrete

The first implementation should optimize for explicitness, not abstraction depth.

That means we do not need a large plugin framework here.
We just need a small assembly surface that makes bundle choice obvious.

Illustrative shape:

```rust
let registry = ToolRegistryBuilder::new()
    .with_bundle(tool_bundle_inspect())
    .with_bundle(tool_bundle_workspace())
    .with_tool(custom_tool_spec())
    .without_tool("workspace.commit")
    .build()?;
```

Equivalent map-based helpers are also acceptable if they stay explicit.

Important properties:

1. bundle constructors return ordinary tool definitions,
2. embedding worlds can add, remove, or override individual tools,
3. the final registry is assembled explicitly by the caller,
4. no hidden preset layer is required to get useful behavior.

### 5) Keep the bundles easily extendable

Embedding worlds should be able to:

1. select only the built-in bundles they want,
2. add or override individual tools,
3. define world-local presets without forking the entire registry implementation.

The current "one default registry" story is too rigid for that.

### 6) Slim the base AIR and adapter story

We should support both:

1. evented composition:
   - a world emits `aos.agent/SessionIngress@1` into `aos.agent/SessionWorkflow@1`,
2. direct library composition:
   - a world links `aos-agent` and calls the session kernel directly inside its own workflow.

For the evented reusable adapter:

1. keep one broad reusable adapter if needed in the short term,
2. but make the installed registry/profile explicit and empty-by-default or base-only by default,
3. avoid implying host or workspace access just because the adapter is imported.

### 7) Prefer the target API over backward compatibility

This refactor does not need to preserve the current `aos-agent` tool API shape.

Recommended stance:

1. optimize for the desired long-term SDK boundary,
2. allow aggressive breaking changes to default registry/profile constructors, bundle wiring, and related AIR packaging where that simplifies the model,
3. do not keep compatibility shims unless they are very cheap and do not blur the new boundary,
4. migrate Demiurge and fixtures forward rather than contorting the new API around old defaults.

The current tool surface is still early enough that a cleaner target is worth more than temporary continuity.

## Scope

### [ ] 1) Define the package boundary explicitly

Document and implement a three-layer split:

1. session kernel:
   - contracts,
   - reducer helpers,
   - context hooks,
   - generic tool orchestration.
2. built-in tool bundles inside `aos-agent`:
   - host bundle,
   - workspace bundle,
   - inspect/query bundle,
   - later app-specific bundles.
3. adapter layer:
   - `SessionWorkflow@1`,
   - world-specific wrappers such as Demiurge.

The important part is that the package boundary becomes obvious in code, AIR, and docs.

### [ ] 2) Extract bundle-specific execution behind extension seams

The current generic tool runner should stop hardcoding bundle-specific behavior inline.

Required outcome:

1. workspace composite tools move behind a bundle-specific runner or hook,
2. later host-specific or domain-specific composite bundles can use the same seam,
3. core tool-batch planning no longer knows the details of bundle internals.

### [ ] 3) Replace one implicit default registry with explicit builders

Refactor the registry surface around explicit composition.

Recommended direction:

1. bundle constructors such as:
   - `tool_bundle_host()`,
   - `tool_bundle_workspace()`,
   - `tool_bundle_inspect()`.
2. registry builders that merge selected bundles,
3. per-tool include/exclude hooks,
4. explicit embedding-world assembly rather than a library-provided preset layer.

### [ ] 4) Slim the base AIR package and make optional surfaces explicit

Refactor AIR assets so that:

1. base `aos-agent/air` is session-kernel oriented,
2. optional effect surfaces are documented and selected deliberately,
3. consumer worlds and wrappers do not inherit host or workspace capabilities by accident.

Whether the optional AIR surfaces stay in one import root or split into multiple roots can remain an implementation choice for this slice.

### [ ] 5) Migrate Demiurge and representative fixtures to explicit bundle selection

Representative consumers should stop depending on one implicit coding-agent preset.

Required outcome:

1. current task-driven Demiurge remains functional with explicit bundle choice,
2. a hosted-style fixture can prove workspace-without-host,
3. a local-coding fixture can prove host-plus-inspect,
4. direct library wrapping remains an acceptable migration path.

## Non-Goals

P4 does **not** attempt:

1. the full context engine API itself,
2. the final session-management model,
3. subagent or session-tree semantics,
4. splitting built-in bundles into separate crates,
5. marketplace or external packaging design for bundles.

This slice is specifically about making tools explicit and composable.
Backward compatibility with the current implicit registry/profile API is not a goal.

## Deliverables

1. Tool-surface-agnostic `aos-agent` core boundary.
2. Built-in tool bundles inside `aos-agent`.
3. Explicit bundle and per-tool selection API.
4. Slimmer base AIR story.
5. Demiurge and representative fixtures updated to explicit bundle selection.

## Acceptance Criteria

1. A consumer can link `aos-agent` and build a session-capable agent with no user-visible tools by default.
2. Built-in host and workspace tools are both available as explicit bundle choices inside `aos-agent`.
3. A hosted-style consumer can select workspace tools without host tools.
4. A local-style consumer can select host tools without requiring workspace tools.
5. The generic tool runner no longer contains hardcoded bundle behavior in its base path.
6. `aos-agent` docs describe built-in tools as optional bundles rather than as core session behavior.

## Recommended Implementation Order

1. document the boundary and define the target layering,
2. extract bundle-specific execution behind extension seams,
3. add bundle constructors and explicit registry/profile builders,
4. slim the base AIR and preset story,
5. migrate Demiurge and representative fixtures to explicit bundle selection.
