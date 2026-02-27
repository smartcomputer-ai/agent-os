# P4: Agent Effects for Coding and Advanced Tooling

**Priority**: P4  
**Effort**: High  
**Risk if deferred**: High (SDK exists but cannot power complete coding/demiurge agent workflows)  
**Status**: Proposed

## Goal

Define and land the additional effects/capabilities needed to support:
- coding-agent-grade tool use,
- Demiurge self-modification/build flows,
- richer headless factory workloads.

This phase extends effect surface area while preserving core AOS boundaries:
- reducer owns logic/state,
- plans execute privileged effects,
- adapters isolate nondeterminism via receipts.

## Context

Current Demiurge already validates the core pattern:
- `llm.generate` + `tool_refs`/`tool_choice`,
- reducer parses tool calls,
- plan-only tool execution,
- workspace/introspect tools,
- traceable execution.

P4 is about filling the missing primitives for complete coding/automation flows.

## Decision Summary

1. Prefer deterministic internal effects for workspace/tree operations.
2. Add adapter effects for external or nondeterministic execution (`shell`, compiler/build).
3. Keep all new effect execution plan-only and cap/policy gated.
4. Keep large payloads in CAS; receipts should return refs where practical.
5. Roll out in slices to preserve existing Demiurge behavior while adding capability.

## Effect Families

## 4.1 Workspace-native deterministic effects (internal)

These should be implemented as kernel internal effects over CAS-backed workspace trees:

1. `workspace.apply_patch@1`
- Purpose: atomic patch-style edits for coding agents.
- Params: `root_hash`, patch ops.
- Receipt: `new_root_hash`, change summary, diagnostics.
- Why: avoids partial multi-write failure modes.

2. `workspace.glob@1`
- Purpose: fast file discovery by pattern.
- Params: `root_hash`, include/exclude glob patterns, limit/cursor.
- Receipt: matched paths + metadata.

3. `workspace.grep@1`
- Purpose: fast content search.
- Params: `root_hash`, query/pattern options, path filters, limit/cursor.
- Receipt: structured matches with path + ranges/snippets.

Optional follow-up:
- `workspace.apply_edits@1` (structured multi-edit batch),
- `workspace.stat@1` (cheap metadata fetch),
- `workspace.exists@1`.

## 4.2 Execution adapter effects (nondeterministic)

1. `exec.shell@1`
- Purpose: run builds/tests/lints/git/project tooling.
- Params: workspace/materialization ref, command, args/env allowlist, timeout, resource limits.
- Receipt: exit code, stdout/stderr refs, timings, runtime metadata.
- Cap type: `exec`.

2. `build.rust_wasm@1` (or `compiler.rust_wasm@1`)
- Purpose: compile reducers/modules inside controlled build environment.
- Params: workspace ref/root, target crate/module, profile/options.
- Receipt: wasm hash ref, diagnostic/log refs, status metadata.
- Cap type: `build`.

Notes:
- Build effect may be layered on top of `exec.shell` later, but should have a stable high-level contract.
- Receipts should avoid large inline payloads; use CAS refs for logs/artifacts.

## 4.3 LLM effect evolution (from P1)

`llm.generate` should evolve to support normalized parsing contracts:
- provider-native raw output ref (`raw_output_ref`),
- normalized output ref in `output_ref` for reducer/tool-runtime consumption.

This keeps reducer logic provider-agnostic while preserving debuggability.

## Capability and Policy Surface

New cap types to introduce:
- `exec`
- `build`

Workspace cap should be extended with new ops:
- `apply_patch`
- `glob`
- `grep`

Policy defaults:
- deny all by default,
- allow only named plans (`aos.agent/tool_call_plan@1`, specialized build plans, etc.),
- keep reducer-origin denial for all high-risk effects.

## Coding-Agent Tool Mapping

Target mapping of SDK tools to effects:
- `read_file` -> `workspace.read_bytes`
- `write_file` -> `workspace.write_bytes`
- `edit_file` -> `workspace.apply_edits@1` (or patch fallback)
- `apply_patch` -> `workspace.apply_patch@1`
- `glob` -> `workspace.glob@1`
- `grep` -> `workspace.grep@1`
- `shell` -> `exec.shell@1`
- `spawn_agent/send_input/wait/close_agent` -> `aos.agent/*` session events (not effects)

## Demiurge Tool Mapping

Demiurge advanced toolset after P4:
- existing introspect/workspace tools,
- governance orchestration tools (existing governance effects),
- compiler/build tool via `build.rust_wasm@1`,
- optional controlled shell tool for diagnostics.

## Phase Plan

### Phase 4.1: Deterministic workspace upgrades
- Land `workspace.apply_patch@1`.
- Land `workspace.glob@1` and `workspace.grep@1`.
- Extend workspace cap/policy schemas and enforcers.

### Phase 4.2: Execution adapter baseline
- Land `exec.shell@1` adapter and cap/policy plumbing.
- Add strict defaults for timeout/env/resource constraints.
- Provide deterministic integration harnesses around receipt contracts.

### Phase 4.3: Compiler/build adapter
- Land `build.rust_wasm@1` adapter.
- Integrate with Demiurge tool flow and/or dedicated build plan.
- Validate artifact hash and diagnostic traceability.

### Phase 4.4: SDK integration and conformance
- Wire new effects into `aos-agent-sdk` tool runtime contracts.
- Add cross-tool e2e tests for coding flow (find/edit/test/build loop).
- Add trace-based assertions for failures/timeouts/policy denials.

## Testing

- Unit tests for new effect param/receipt schema validation.
- Kernel internal effect tests for deterministic workspace behavior.
- Adapter integration tests (mocked + controlled real env for exec/build).
- End-to-end agent tests for:
  - tool fan-out/fan-in,
  - atomic patching,
  - shell/build result ingestion,
  - replay parity.

## Definition of Done

- Required coding-agent tool primitives are available via stable effect contracts.
- Demiurge can exercise at least one build/compile loop through the new effect surface.
- New effects are fully cap/policy gated and traceable.
- Replay behavior remains deterministic for internal effects; adapter nondeterminism is isolated by receipts.

## Open Questions

- Should `workspace.apply_edits@1` ship in v0.10 or defer to v0.11 if `apply_patch` is sufficient?
- Do we expose `exec.shell@1` as a generic effect, or keep it constrained to curated command profiles first?
- Should `build.rust_wasm@1` be a first-class effect or a profile of a more general `build.run@1` family?
