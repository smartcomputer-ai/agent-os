# v0.10 Agent SDK

This phase moves AOS from a toy chat app toward a reusable agent runtime stack:

1. **P1**: LLM library foundation (`aos-llm`) refactored from `tmp-llm-lib-code/`.
2. **P2**: Agent SDK built on AOS primitives (reducers/plans/effects/workspaces).
3. **P3**: Refactor Demiurge to consume the Agent SDK (light spec, iterative).
4. **P4**: New effects/capabilities for coding and advanced agent functionality.

## Documents

- `roadmap/v0.10-agent-sdk/p1-llm-lib.md`
- `roadmap/v0.10-agent-sdk/p2-agent-sdk.md`
- `roadmap/v0.10-agent-sdk/p2.1-session-contracts.md`
- `roadmap/v0.10-agent-sdk/p2.2-llm-contract-direct-provider-model.md`
- `roadmap/v0.10-agent-sdk/p2.2-llm-boundary-issue.md`
- `roadmap/v0.10-agent-sdk/p2.3-tool-loop-safety-context-bounds.md`
- `roadmap/v0.10-agent-sdk/p2.4-events-observability-contract.md`
- `roadmap/v0.10-agent-sdk/p2.5-failure-retry-cancel.md`
- `roadmap/v0.10-agent-sdk/p2.6-sdk-conformance-live-smoke.md`
- `roadmap/v0.10-agent-sdk/p2-closure-report.md`
- `roadmap/v0.10-agent-sdk/p2-agent-sdk-concerns.md`
- `roadmap/v0.10-agent-sdk/p3-demiurge-refactor.md`
- `roadmap/v0.10-agent-sdk/p4-agent-effects.md`

## Scope Notes

- Breaking changes are acceptable in v0.10.
- Keep AOS kernel/spec boundaries intact: reducers own state/business logic, plans orchestrate effects.
- Agent semantics live above core AOS, not inside the kernel.
- Built-in `sys/*` contracts are core-owned (`spec/defs`, `aos-effects`, `aos-host`, `aos-sys`) and must not be defined in SDK AIR assets.
- SDK schema/plan/module naming uses the `aos.agent/*` namespace.
- Canonical SDK implementation lives in `crates/aos-agent-sdk` (shared Rust types/helpers, SDK WASM modules, reusable AIR assets).
- `crates/aos-sys` remains reserved for built-in `sys/*` runtime primitives and enforcers.
- End-to-end SDK scenarios run through `crates/aos-smoke` fixtures; do not create a separate e2e runner in `aos-agent-sdk`.
- Real-provider LLM smoke checks are opt-in and separate from deterministic replay-parity gates.
- Session/run config is provider/model-first; no provider-profile registry is required in v0.10.
