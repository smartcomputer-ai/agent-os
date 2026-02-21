# P3: Agent Workspace Contract (Prompt Packs + Tool Catalog)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: High (agent config remains app-specific and blocks reusable SDK patterns)  
**Status**: Proposed

## Goal

Define a reusable SDK-level contract for agent content/configuration stored in a **dedicated workspace** per agent session/app.

This replaces Demiurge-specific workspace/tool registry prototype behavior with a generalized pattern that works for:
- chat agents,
- coding agents,
- operator/factory agents,
- future multimodal scenarios (static context docs/images/assets).

## Problem

Today, Demiurge proves the pattern but is app-specific:
- tool JSON defs live in a workspace and are scanned ad hoc,
- prompt management is not first-class,
- workspace refresh logic is coupled to Demiurge internals,
- runtime adoption of updated workspace content is not an SDK contract.

We need an SDK design where:
1. agent content is workspace-native and introspectable,
2. updates can be pushed from anywhere (UI/CLI/automation),
3. reducers/plans stay within AIR boundaries,
4. adoption is explicit (no hidden autosync),
5. tool execution remains plan-only and policy/cap gated.

## Decision Summary

1. Each agent uses a dedicated **Agent Workspace** for prompts, tool descriptors, and static context assets.
2. Workspace adoption is **explicit event-driven sync/apply**, not automatic background sync.
3. Tool definitions are split into:
   - **descriptor layer** (workspace, LLM-facing),
   - **binding layer** (AIR/reducer/plan routing, authority).
4. Prompt packs are workspace-defined and selected by config/run overrides.
5. Reducer owns active configuration state; plans perform workspace effects.
6. Existing session lifecycle semantics remain (run config immutability, deterministic step flow).

## Non-Goals (v0.10)

- No kernel changes for agent workspace semantics.
- No implicit polling/autosync from reducers.
- No runtime execution of arbitrary workspace scripts.
- No attempt to treat workspace files as authority for effect execution.

## Architecture

### 1) Dedicated Agent Workspace

Each agent references one workspace (or a pinned version of it) as content source.

Proposed layout:

```text
<agent-workspace>/
  agent.workspace.json
  prompts/
    templates/
      system.base.md
      response.style.md
    packs/
      default.json
      concise.json
  tools/
    descriptors/
      introspect.manifest.json
      workspace.read_bytes.json
      ...
    catalogs/
      default.json
      coding.json
  context/
    docs/
    images/
    snippets/
```

Notes:
- `agent.workspace.json` is the index file for fast deterministic loading.
- `prompts/packs/*.json` compose prompt templates and static context refs.
- `tools/descriptors/*.json` are LLM-facing function/tool schemas.
- `tools/catalogs/*.json` name which descriptors are active per catalog.
- Prompt pack/catalog files should be provider-agnostic JSON blobs consumable by the
  existing host LLM adapter (`message_refs` and `tool_refs` CAS contract).

### 2) Prompt Pack

A prompt pack is a named config bundle in workspace content.

Minimum fields:
- pack id,
- ordered prompt template refs/paths,
- optional static context refs/paths,
- optional default response format/options refs.

Prompt packs are content-only. They do not grant authority.

### 3) Tool Catalog

A tool catalog is a named set of tool descriptors (LLM-visible contract).

Important split:
- **Descriptor** (workspace): name, description, JSON schema.
- **Binding** (runtime authority): how `tool_name` maps to reducer intent/plan/effect path.

A tool call is executable only if:
1. descriptor is present in active catalog,
2. binding exists for that tool name,
3. resulting plan/effect passes caps/policy.

### 4) Binding/Authority Layer

Execution mapping stays in SDK/app AIR + reducer logic, not workspace files.

- Reducer parses normalized tool calls and emits typed DomainIntent(s).
- Triggered plans perform effects (`workspace.*`, `introspect.*`, `llm.generate`, future `exec.shell`, etc.).
- Workspace content can enable/disable visibility to model, but cannot bypass caps/policy.

## Sync + Apply Model (No Autosync)

### Desired behavior

1. External actor updates workspace (UI/CLI/automation) and commits new workspace version.
2. External actor sends explicit event to agent reducer: "refresh workspace config".
3. Agent resolves and validates new snapshot.
4. External actor (or policy) sends explicit apply event for when to adopt.

No refresh/apply event means no change in active agent content.

### Proposed flow

1. `WorkspaceSyncRequested`
- input: workspace name + optional target version + known version.
- reducer emits domain intent for sync plan.

2. `workspace_sync_plan`
- `workspace.resolve`.
- if unchanged vs known version: raise `WorkspaceSyncUnchanged`.
- else read index + referenced catalog/pack assets (`workspace.read_ref` preferred, `workspace.read_bytes` fallback), raise `WorkspaceSnapshotReady`.

3. Reducer validation/adoption staging
- reducer validates snapshot structure deterministically,
- stores as `pending_workspace_snapshot` (not active yet).

4. `WorkspaceApplyRequested`
- reducer applies pending snapshot according to mode:
  - `next_run` (default),
  - `next_step_boundary` (if run active),
  - `immediate_if_idle`.

5. Step materialization
- LLM request for next step uses active prompt pack + tool catalog refs.
- SDK helper path:
  - `materialize_workspace_step_inputs(...)` prepends `prompt_pack_ref` and selects `tool_catalog_ref`.
  - `materialize_llm_generate_params_with_workspace(...)` maps this into `sys/llm.generate` params.
  - Host `LlmAdapter` dereferences and parses the JSON blobs (messages/tools/tool_choice), so reducers do not need to decode workspace files.

## Config Surface (SDK Direction)

Session/run provider-model config remains as-is.

Add workspace-aware fields in next contract rev (name/version TBD):
- session defaults:
  - `workspace_binding` (workspace + version selector),
  - `default_prompt_pack`,
  - `default_tool_catalog`.
- run overrides:
  - optional prompt pack/tool catalog override,
  - optional workspace target version override.

Run boundary behavior:
- provider/model remains immutable per run,
- workspace snapshot adoption is explicit and deterministic,
- if applied mid-run, effect starts at deterministic boundary (`StepBoundary`).

## Workspace Index File

`agent.workspace.json` should provide a stable machine-readable index.

Minimum intent:
- schema/version,
- named prompt packs (id -> file path),
- named tool catalogs (id -> file path),
- optional static context groups,
- defaults.

This avoids expensive directory scans and keeps adoption deterministic.

Compatibility fallback (migration):
- if index missing, allow legacy `tools/*.json` scan path (Demiurge-style) for transition period.

## Security and Governance

1. Workspace content is untrusted input until reducer validation succeeds.
2. Tool descriptor presence does not imply execution permission.
3. All execution remains plan-only under caps/policy.
4. Workspace sync plan must be limited to allowed workspace names/ops.
5. Invalid snapshot never replaces active snapshot; reducer emits explicit failure event.

## MCP and Future Adapters

This split is MCP-ready:
- descriptor layer stays the same (tool name/schema/description),
- binding layer can later target MCP-backed effects/adapters,
- governance surface remains unchanged (plans + caps + policy).

Future tool families from P4 (`workspace.apply_patch`, `workspace.grep`, `exec.shell`, build effects) plug into binding layer without changing prompt-pack/cfg model.

## Migration from Demiurge Prototype

### Current prototype to lift

- tool refs discovered by workspace scan (`tools/*.json`),
- explicit version-aware tool registry refresh behavior,
- reducer-side tool-call interpretation.

### SDK migration slices

1. Introduce SDK workspace contracts + schemas/events for sync/apply.
2. Add reusable `aos.agent/workspace_sync_plan@1` template.
3. Move Demiurge from bespoke tool-registry events to SDK workspace events.
4. Introduce prompt-pack resolution in Demiurge via new workspace snapshot.
5. Keep legacy scan fallback briefly; remove after migration validation.

## Testing

Deterministic coverage should include:

1. Initial sync loads workspace snapshot and populates active prompt/tool config.
2. Workspace updated externally does not affect agent until sync+apply events.
3. Apply at `next_step_boundary` changes subsequent step materialization deterministically.
4. Invalid workspace snapshot yields failure event and retains prior active snapshot.
5. Replay parity for sync/apply flows.
6. Backward compatibility fallback for legacy `tools/*.json` scanning during migration window.

## Definition of Done

1. SDK has a documented and implemented workspace contract for prompt packs and tool catalogs.
2. Agent apps can push workspace updates from external clients and explicitly trigger adoption.
3. Demiurge workspace sync/tool-registry prototype is replaced by reusable SDK flow.
4. Tool execution authority remains in reducer+plan+caps/policy boundaries.
5. At least one smoke fixture demonstrates full external-update -> sync -> apply -> run behavior with replay parity.

## Open Questions

1. Should workspace snapshot apply default to `next_run` only, or support `next_step_boundary` in v0.10?
2. Should prompt template storage prefer markdown bytes, JSON message parts, or both?
3. Do we keep legacy `tools/*.json` fallback through all v0.10, or remove once Demiurge refactor lands?
4. Do we model workspace config events as `SessionEvent` extensions or as parallel `aos.agent/*` config events consumed by the same reducer?
