# P2 Closure Report: Agent SDK on AOS Primitives

**Stage**: P2 (umbrella close)  
**Status**: Closed on 2026-02-14

## Outcome

P2 is closed with staged SDK delivery completed across `p2.1` through `p2.6`, including deterministic conformance and opt-in live smoke coverage in `crates/aos-smoke`.

## Stage Completion Snapshot

1. `p2.1-session-contracts.md`: complete.
2. `p2.2-llm-contract-direct-provider-model.md`: complete.
3. `p2.3-tool-loop-safety-context-bounds.md`: complete.
4. `p2.4-events-observability-contract.md`: complete.
5. `p2.5-failure-retry-cancel.md`: complete.
6. `p2.6-sdk-conformance-live-smoke.md`: complete.

## Exit-Gate Mapping

1. Deterministic SDK conformance lane (`20+`) exists with replay parity checks (`20-agent-session`).
2. Core fixture lane (`00`-`19`) remains separated from SDK lane.
3. Core trace failure-classification fixture remains in core lane (`10-trace-failure-classification`).
4. Live-provider SDK lane exists and is opt-in (`chat-live`, `agent-live`) with OpenAI/Anthropic provider routing.
5. Unknown provider/model run-start rejection is covered deterministically in SDK conformance fixture assertions.
6. Active-run config immutability and subsequent-run provider/model updates are covered deterministically.
7. Repository-level boundary guard for `sys/Llm*` ownership was explicitly deferred as non-blocking follow-up for this close.
8. Closure documentation is published in this report and linked from stage docs.

## Verification Snapshot (2026-02-14)

1. `cargo check -p aos-smoke`
2. `cargo run -p aos-smoke -- chat-live`
3. `cargo run -p aos-smoke -- agent-live`

Note: live-provider executions are operational/interop checks and remain non-gating for deterministic CI replay requirements.

## Deferred Follow-Up

1. Add repository-level automated boundary guard to prevent SDK-local ownership of `sys/Llm*` schema definitions.
