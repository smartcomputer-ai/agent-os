# v0.10 Agent SDK

This phase moves AOS from a toy chat app toward a reusable agent runtime stack:

1. **P1**: LLM library foundation (`aos-llm`) refactored from `tmp-llm-lib-code/`.
2. **P2**: Agent SDK built on AOS primitives (reducers/plans/effects/workspaces).
3. **P3**: Refactor Demiurge to consume the Agent SDK (light spec, iterative).
4. **P4**: New effects/capabilities for coding and advanced agent functionality.

## Documents

- `roadmap/v0.10-agent-sdk/p1-llm-lib.md`
- `roadmap/v0.10-agent-sdk/p2-agent-sdk.md`
- `roadmap/v0.10-agent-sdk/p3-demiurge-refactor.md`
- `roadmap/v0.10-agent-sdk/p4-agent-effects.md`

## Scope Notes

- Breaking changes are acceptable in v0.10.
- Keep AOS kernel/spec boundaries intact: reducers own state/business logic, plans orchestrate effects.
- Agent semantics live above core AOS, not inside the kernel.
- SDK schema/plan/module naming uses the `aos.agent/*` namespace.
