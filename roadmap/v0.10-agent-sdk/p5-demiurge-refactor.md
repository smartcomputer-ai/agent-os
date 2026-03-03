# P5: Demiurge Workflow-Native Refactor

**Priority**: P3  
**Effort**: High  
**Risk if deferred**: High (Demiurge remains on legacy plan/event paths)  
**Status**: Complete (2026-02-27)

## Goal

Bring `worlds/demiurge` fully onto the post-plan runtime model:

1. No `defplan`, no `triggers`, no `routing.events`.
2. Keyed workflow subscriptions + receipt-driven continuation.
3. SDK session contracts as the base state machine, with explicit Demiurge workflow hooks for app-local tool execution.

## Completion Update (2026-02-27)

Completed in this pass:

1. Replaced legacy plan/event AIR topology with workflow-native routing:
   - removed `plans`/`triggers` from manifest,
   - switched to `routing.subscriptions`,
   - added keyed subscriptions for:
     - `aos.agent/SessionIngress@1`,
     - `demiurge/ToolCallRequested@1`.
2. Removed legacy plan files:
   - deleted `worlds/demiurge/air/session_workspace_sync_wrapper.air.json`,
   - deleted `worlds/demiurge/air/tool_execute_from_request.air.json`.
3. Replaced reducer ABI/event contract:
   - module now uses `module_kind: "workflow"` + `key_schema`,
   - state schema moved to `demiurge/State@1`,
   - event schema moved to `demiurge/WorkflowEvent@1` (session ingress + receipts + tool request).
4. Reworked Demiurge workflow implementation:
   - migrated off `aos.agent/SessionEvent@1`,
   - now delegates SDK session transitions via `apply_session_workflow_event_with_catalog_and_limits`,
   - emits SDK `llm.generate` effects directly from reducer output,
   - handles typed `demiurge/ToolCallRequested@1` in-workflow,
   - emits `introspect.manifest` / `workspace.resolve` / `workspace.read_bytes` directly,
   - settles tool call and tool batch lifecycle using SDK ingress events (`ToolCallSettled`, `ToolBatchSettled`) after receipts.
5. Updated capability and policy model:
   - capability grants now include `sys/llm.basic@1`, `sys/query@1`, `sys/workspace@1`,
   - module slot bindings added (`llm`, `query`, `workspace`),
   - policy rules moved to `origin_kind: "workflow"` / `origin_name: "demiurge/Demiurge@1"`.
6. Updated smoke script to workflow-native contracts:
   - sends `aos.agent/SessionIngress@1` instead of `SessionEvent@1`,
   - uses `demiurge/ToolCallRequested@1` for typed tool execution requests,
   - removes legacy `RunStarted` / `StepBoundary` events,
   - validates both workspace-applied and direct-refs run paths.

## Work Packages

### WP1: AIR topology reset (Completed)

1. Move manifest to `routing.subscriptions` only.
2. Remove plan/triggers references and plan assets.
3. Keep only workflow-native schemas and module contracts.

### WP2: Workflow runtime migration (Completed)

1. Replace old session event reducer contract with workflow event union.
2. Keep SDK helper as base session state machine.
3. Layer Demiurge tool execution in explicit workflow hooks.

### WP3: Capability/policy realignment (Completed)

1. Bind workflow slots for emitted effects (`llm`, `query`, `workspace`).
2. Convert policy rules to workflow-origin authorization.
3. Keep default-deny fallback.

### WP4: Smoke cutover (Completed)

1. Send workflow-native ingress envelopes.
2. Keep typed Demiurge tool request lane.
3. Validate workspace-applied and direct-refs runs end-to-end.

## Definition of Done

1. Demiurge has no plan/triggers runtime surface.
2. Demiurge routes through keyed workflow subscriptions.
3. Demiurge workflow emits/handles effects + receipts directly.
4. SDK session lifecycle remains the primary state machine.
5. Smoke path validates migrated runtime behavior.
