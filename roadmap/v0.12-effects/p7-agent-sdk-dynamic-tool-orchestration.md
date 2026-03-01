# P7: Agent SDK Dynamic Tool Orchestration and Runtime Tool Gating

**Priority**: P7  
**Status**: Proposed  
**Date**: 2026-03-01

## Goal

Evolve `aos-agent` so tool management is owned by the session workflow
state and reducer logic, not by workspace tool catalogs.

This slice makes dynamic tool availability and tool-batch orchestration the
core agent responsibility:

1. enable/disable tools deterministically from workflow state,
2. gate tools on runtime context (for example host fs tools disabled when no
   active host session),
3. orchestrate parallel tool execution plans,
4. shuttle tool calls/results between `llm.generate` and host execution loops
   through explicit workflow events.

Prompt management stays in scope as related context plumbing, but prompt
refactors are not part of this push.

## Problem Statement

Current `aos-agent` behavior is too workspace-centric for tools:

1. `WorkspaceSnapshot@1` carries `tool_catalog` + `tool_catalog_ref`.
2. `RunConfig@1` / `SessionConfig@1` carry `tool_catalog` and `tool_refs`.
3. `materialize_llm_generate_params_with_workspace` derives tool refs from
   workspace snapshot fallback logic.

This model has two issues:

1. Tool authority and runtime availability are not modeled as first-class
   workflow state.
2. Dynamic gating (for example disable `host.fs.*` when no `host.session`) is
   awkward and externalized.

## Design Principles

1. Workflow-owned: tool inventory, availability, and orchestration live in
   `SessionState`.
2. Deterministic: effective tools and batch plans are pure functions of state +
   events.
3. Runtime-context aware: tool availability is derived from explicit runtime
   facts in state (host session active, capability flags, profile toggles).
4. Host-loop compatible: keep external loops as executors/bridges, but make
   workflow the source-of-truth for what is executable.
5. Provider-aligned: allow provider-specific preferences (OpenAI
   `apply_patch`, Anthropic/Gemini `edit_file`) without duplicating core logic.
6. Zero-setup startup: default tool registry + profiles must be preloaded at
   agent install/push time so a new session can run without bootstrap events.

## High-Level Architecture

## 1) Tool Inventory Layer (static per agent profile)

The agent workflow owns a declarative tool inventory in state, keyed by stable
`tool_name`.

Each entry maps:

1. `tool_name` (LLM-visible),
2. `tool_ref` (hash ref for provider tool schema),
3. `executor` (`effect` mapping for host/other effects or host-loop internal),
4. `availability_rules` (for example requires active host session),
5. `parallelism_hint` (`parallel_safe` + optional `resource_key`).

The source of inventory is no longer workspace tool catalog blobs. Inventory is
preloaded from agent package defaults at install/push time, and may be updated
explicitly via override events.

## 1.1) Install-Time Profile Preload (Required)

Agent installation into a world must materialize default tool registry and
provider profiles with zero runtime setup choreography.

Requirements:

1. Fresh session state already contains a valid registry and default profiles.
2. `RunRequested` can emit `llm.generate` immediately without prior
   `ToolRegistrySet`.
3. Override events remain optional for customization, not mandatory bootstraps.
4. World push/install flow carries profile/tool blobs or embedded defaults
   deterministically with the agent module package.

## 2) Tool Runtime Layer (dynamic per turn)

Workflow computes `effective_tools` each turn from:

1. selected profile,
2. run/session overrides (`enable`, `disable`, `force`),
3. runtime context (host session lifecycle and similar facts),
4. policy/capability hints surfaced into state.

The computed effective set is what gets sent as `runtime.tool_refs` to
`llm.generate`.

## 3) Tool Orchestration Layer (batch planner)

After host bridge submits observed LLM tool calls, workflow:

1. validates each call against `effective_tools`,
2. marks rejected calls deterministically (`Ignored` or `Failed`),
3. creates execution groups for parallel/serial execution,
4. records `ActiveToolBatch` plan in state.

Host loop executes according to this plan and reports settlements back via
ingress events.

## 4) Shuttle Layer (LLM <-> Tool bridge)

Host loop responsibilities remain bridge-only:

1. execute `llm.generate` effect and parse tool calls from receipt output blob,
2. send `ToolCallsObserved` ingress event to workflow,
3. execute planned tool calls (parallel where allowed),
4. send per-call settled events and final batch-settled event with results refs.

Workflow remains orchestration source-of-truth.

## Breaking Contract Changes (`@1` Replaced In Place)

v0.12 will replace `aos.agent/*@1` contracts in place (explicitly allowed for
this phase).

## Remove Workspace-Tool Coupling

Remove tool-catalog fields from workspace/session-run contracts:

1. `WorkspaceSnapshot@1`:
   - remove `tool_catalog`
   - remove `tool_catalog_ref`
2. `WorkspaceSnapshotReady@1`:
   - remove `tool_catalog_bytes`
3. `SessionConfig@1`:
   - remove `default_tool_catalog`
   - remove `default_tool_refs`
   - add tool-policy fields (below)
4. `RunConfig@1`:
   - remove `tool_catalog`
   - remove `tool_refs`
   - add tool-policy fields (below)

Keep prompt fields as-is for now.

## Add Tool Management Schemas

Add SDK schemas (names indicative):

1. `aos.agent/ToolSpec@1`
2. `aos.agent/ToolExecutor@1`
3. `aos.agent/ToolAvailabilityRule@1`
4. `aos.agent/ToolRuntimeContext@1`
5. `aos.agent/EffectiveToolSet@1`
6. `aos.agent/ToolCallObserved@1`
7. `aos.agent/ToolExecutionPlan@1`
8. `aos.agent/PlannedToolCall@1`
9. `aos.agent/ToolBatchPlan@1`

## Session State Changes

Extend `SessionState@1` with workflow-owned tool manager state:

1. `tool_registry` (map tool name -> `ToolSpec`)
2. `tool_profile` (active profile id)
3. `tool_runtime_context`:
   - `host_session_id: option<text>`
   - `host_session_status: option<variant ready|closed|expired|error>`
   - future runtime facts
4. `effective_tools` (materialized for current turn/provider)
5. `active_tool_batch` upgraded to include execution plan groups
6. optional `last_tool_plan_hash` (auditability)

## Ingress/Event Changes

Add/replace `SessionIngressKind@1` variants:

1. `ToolRegistrySet` (optional full replacement override)
2. `ToolProfileSelected` (switch profile)
3. `ToolOverridesSet` (enable/disable lists for run/session scope)
4. `HostSessionUpdated` (session context updates for gating)
5. `ToolCallsObserved` (LLM tool calls list from latest completion)

Keep and evolve:

1. `ToolCallSettled`
2. `ToolBatchSettled`

Deprecate/remove workspace tool catalog sync fields/events.
`ToolRegistrySet` is not part of required startup flow.

## Reducer Logic Evolution

## Effective Tool Computation

Replace workspace fallback helper with reducer-owned selection:

1. `compute_effective_tools(state, run_config, provider) -> EffectiveToolSet`
2. deterministic ordering by stable `tool_name` or explicit profile order
3. runtime-gated filters:
   - no active host session -> disable `host.exec` + `host.fs.*`
   - keep `host.session.open` enabled when profile permits
4. apply override masks:
   - deny wins over allow
   - unknown tool names are deterministic errors

## Batch Planning

When `ToolCallsObserved` arrives:

1. reject unknown/disabled tools,
2. build `ToolExecutionPlan` groups:
   - same `resource_key` => serialize
   - `parallel_safe=true` and no conflict => same parallel group
   - non-parallel-safe => singleton groups
3. persist plan in `active_tool_batch`.

This is the canonical plan host loop executes.

## Run/Turn Loop Ownership

Workflow owns turn orchestration state transitions:

1. `RunRequested` -> emit `llm.generate` with computed `effective_tools`.
2. `ToolCallsObserved` -> plan and await settlements.
3. `ToolBatchSettled` -> mark batch complete and trigger next LLM turn command
   path (exact message assembly can remain host-assisted in this slice).

## Host Session Gating Semantics

Minimum gating requirements for coding agents:

1. `host.session.open` available when profile allows.
2. `host.exec`, `host.fs.read_file`, `host.fs.write_file`,
   `host.fs.edit_file`, `host.fs.apply_patch`, `host.fs.grep`,
   `host.fs.glob`, `host.fs.stat`, `host.fs.exists`, `host.fs.list_dir`
   require `tool_runtime_context.host_session_status == ready`.
3. on `HostSessionUpdated(closed|expired|error)`, all host session-bound tools
   become unavailable on next tool computation.

## Provider-Aligned Profile Defaults

Tool manager supports provider-tuned defaults without duplicating inventory:

1. OpenAI profile:
   - include `apply_patch` as primary edit path,
   - optionally keep `edit_file` disabled by default.
2. Anthropic/Gemini profile:
   - include `edit_file` as primary edit path,
   - optionally keep `apply_patch` disabled by default.

All profiles share common read/search/shell primitives when runtime-gated rules
allow them.

## Implementation Plan

### Phase 7.1: Contract Refactor (Breaking `@1`)

1. Replace affected schemas in `crates/aos-agent/air/schemas.air.json`.
2. Update `module.air.json` and `manifest.air.json` to new schema shape.
3. Regenerate/align Rust contracts in `crates/aos-agent/src/contracts/*`.
4. Define install-time preload contract for default tool registry + profiles.

### Phase 7.2: Reducer + Helpers

1. Add tool manager data model + reducers in `helpers/workflow.rs`.
2. Replace workspace-tool derivation logic in `helpers/llm.rs`.
3. Keep prompt-related workspace flow intact for now.

### Phase 7.3: Host Bridge Protocol

1. Add ingress handling for `ToolCallsObserved` and host session updates.
2. Define execution-plan read model for host loop.
3. Ensure parallel grouping is deterministic.

### Phase 7.4: Smoke/E2E Adoption

1. Update `aos-smoke` agent live fixture to use workflow-owned tool registry
   preloaded at install time (no required registry bootstrap ingress).
2. Add coding-agent oriented smoke scenario with host session gating.
3. Verify replay identity with planned/settled tool batches.

## Verification Matrix

1. No active host session:
   - `effective_tools` excludes host fs/exec tools.
2. Host session becomes ready:
   - host fs/exec tools appear deterministically.
3. Host session closes/expires:
   - host tools disappear on next computation.
4. Parallel planning:
   - non-conflicting parallel-safe calls grouped together.
   - conflicting/resource-key calls serialized.
5. Provider profile switching:
   - OpenAI defaults include `apply_patch` preference.
   - Anthropic defaults include `edit_file` preference.
6. Install-time preload:
   - first `RunRequested` succeeds without prior tool-registry setup events.
7. Replay:
   - same ingress stream yields byte-identical `SessionState`.

## Risks

1. Bridge complexity if host loops execute calls without honoring workflow plan.
2. Migration churn from replacing `@1` contracts in place.
3. Drift between packaged defaults and optional override payloads.

## Non-Goals

1. Prompt pack redesign (deferred).
2. Replacing `llm.generate` adapter contracts.
3. Eliminating host bridge execution loops in this slice.

## Deliverables / DoD

1. Tool inventory and availability are workflow-owned state.
2. Workspace no longer acts as tool catalog source.
3. New sessions run with preloaded tool registry/profiles without bootstrap
   setup events.
4. Dynamic host-session gating works (host tools off until session ready).
5. Parallel tool-batch execution plans are produced deterministically by reducer.
6. Host bridge shuttles LLM tool calls/results through explicit SDK events.
