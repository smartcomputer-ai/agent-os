# P3.4: Demiurge Cutover to Agent SDK

**Priority**: P3  
**Effort**: Medium-High  
**Risk if deferred**: High (Demiurge stays on a separate agent stack)  
**Status**: Complete (2026-02-23)

## Goal

Replace `apps/demiurge` with SDK-native agent contracts in one pass.

No compatibility mode, no migration bridge, no dual-stack runtime.

## Implementation Outcome (2026-02-23)

1. Demiurge reducer now uses SDK session contracts end-to-end:
   - reducer ABI: `aos.agent/SessionEvent@1` / `aos.agent/SessionState@1`
   - reducer logic: `apply_session_event_with_catalog_and_limits`
2. Demiurge AIR contracts were reset to SDK-native routing/triggers and bespoke chat/tool registry plans were removed.
3. Workspace sync now goes through imported SDK plans with a thin Demiurge session wrapper:
   - `aos.agent/core_prompt_sync_from_workspace@1`
   - `aos.agent/core_tool_catalog_sync_from_workspace@1`
   - `aos.agent/core_workspace_sync@1`
   - `demiurge/session_workspace_sync_wrapper@1`
4. Tool execution is now explicit plan authority bound to typed intents (`demiurge/tool_execute_from_request@1`) and settles SDK tool-batch lifecycle events.
5. Workspace content conventions were moved to:
   - `agent-ws/prompts/packs/*.json`
   - `agent-ws/tools/catalogs/*.json`
6. Demiurge smoke now validates both execution modes:
   - workspace sync/apply mode
   - direct prompt/tool refs mode
7. SDK core workspace sync plans were hardened for deterministic failure on missing workspaces and composed sync now runs sequentially to avoid duplicate intent-hash wait collisions.

## Target End State

Demiurge becomes an SDK consumer, not a parallel agent framework.

### 1) Reducer contract

1. Reducer ABI uses `aos.agent/SessionEvent@1` and `aos.agent/SessionState@1`.
2. Reducer applies SDK transitions via `apply_session_event_with_catalog_and_limits`.
3. App-specific logic stays in explicit hooks/helpers layered around SDK state, not a replacement state machine.

### 2) Prompt/tool source model

1. Prompt sources follow SDK precedence:
   - run `prompt_refs` > workspace `prompt_pack_ref`.
2. Tool visibility follows SDK precedence:
   - step `tool_refs` > run `tool_refs` > workspace `tool_catalog_ref`.
3. Workspace is optional:
   - direct refs path works with no workspace sync/apply.

### 3) Tool authority model

1. Descriptor visibility may come from workspace or direct refs.
2. Execution authority is reducer/plan-owned only.
3. Tool binding is explicit:
   - `tool_name` -> typed intent payload -> plan trigger -> allowed effects/caps.
4. Unsupported or unauthorized tool intents are rejected deterministically.

### 4) Plan topology

1. Workspace sync uses imported SDK core plans via one thin Demiurge wrapper for `SessionEvent` envelope adaptation.
2. Demiurge-specific plans remain only where app-specific effect behavior is required.
3. No separate Demiurge tool-catalog sync plan remains.

## Work Packages (No Migration Path)

### WP1: AIR contract reset in Demiurge (Completed)

1. Rewrite `apps/demiurge/air/module.air.json` to SDK session reducer ABI.
2. Rewrite `apps/demiurge/air/manifest.air.json` routing/triggers around `aos.agent/SessionEvent@1`.
3. Remove obsolete custom chat/tool schemas from `apps/demiurge/air/schemas.air.json`.
4. Keep only Demiurge-local schemas that are still needed for app-specific intents/results.

### WP2: Reducer replacement (Completed)

1. Replace `apps/demiurge/reducer/src/lib.rs` bespoke chat reducer with SDK session reducer wrapper + hooks.
2. Add deterministic provider/model allowlists (same pattern as SDK live fixture).
3. Keep tool-call interpretation in explicit Demiurge hook code, but drive lifecycle/epochs/tool-batch invariants through SDK state.

### WP3: Workspace sync and config wiring (Completed)

1. Use imported `aos.agent/core_workspace_sync@1` behind a thin session wrapper plan.
2. Remove `apps/demiurge/air/tool_registry_plan.air.json`.
3. Ensure reducer handles `WorkspaceSync*`/`WorkspaceApplyRequested` events through SDK helper path.
4. Ensure direct refs mode (no workspace) is a first-class execution path.

### WP4: Tool execution refactor (Completed)

1. Replace ad hoc `ToolCallRequested` pipeline with SDK-aligned tool batch lifecycle:
   - batch start,
   - per-call settle,
   - batch settled.
2. Keep effect execution plan-only; reducers emit/consume events only.
3. For each supported tool, define one typed intent contract and one bound plan/effect route.

### WP5: Demiurge workspace content conventions (Completed)

1. Move workspace prompt content to `prompts/packs/<pack>.json`.
2. Move workspace tool descriptors to `tools/catalogs/<catalog>.json`.
3. Remove dependence on ad hoc workspace listing/scanning semantics.

### WP6: Smoke and replay hardening (Completed)

1. Replace current Demiurge smoke to drive SDK session events.
2. Cover both modes:
   - workspace sync/apply mode,
   - direct prompt/tool refs mode.
3. Add deterministic failures for unsupported/unauthorized tool and workspace sync paths.
4. Keep replay parity as mandatory gate.

## Definition of Done

1. Demiurge reducer/event/state contracts are SDK-native (`aos.agent/*`).
2. Old custom chat/tool registry control-plane paths are removed.
3. Workspace sync uses imported SDK core plans, not Demiurge-local equivalents.
4. Tool execution authority is explicit and outside workspace descriptors.
5. Demiurge runs end-to-end in both workspace and direct-refs modes.
6. Replay parity passes for migrated flows.

## Out of Scope for This Item

1. New coding-agent tool families beyond current Demiurge capability set.
2. UX redesign of Demiurge shell/client surfaces.
3. Cross-world orchestration features beyond single-session runtime cutover.
