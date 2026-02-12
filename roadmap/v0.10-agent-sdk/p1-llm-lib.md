# P1: LLM Library Foundation (`aos-llm`)

**Priority**: P1  
**Effort**: Medium/High  
**Risk if deferred**: High (blocks agent SDK velocity and provider quality)  
**Status**: Complete (verified in-repo on February 12, 2026)

## Goal

Create a production-grade AOS-native LLM library by refactoring the code in:

- `roadmap/v0.10-agent-sdk/tmp-llm-lib-code/`

This becomes the foundation for:
- host `llm.generate` adapter behavior,
- streaming surfaces,
- tool-call normalization across providers,
- future coding-agent and factory workloads.

**Very Important**: WE CAN MAKE BREAKING CHANGES. DO NOT, I REPEAT, DO NOT WORRY ABOUT BACKWARD COMPATIBILITY. Only focus on what the best new setup and contracts and schemas would be and agressively refactor towards that goal.


## Why First

Current `aos-host` LLM adapter works, but it is still a narrow bridge focused on one-shot calls.  
Agent workloads need stronger provider abstraction, tool behavior parity, and streaming/event normalization before SDK work can stabilize.

## Inputs (Existing Code to Refactor)

`tmp-llm-lib-code` currently provides:
- multi-provider adapters (OpenAI + Anthropic),
- unified message/tool model,
- streaming event normalization,
- high-level generate/stream APIs,
- cross-provider conformance tests.

It does not fit AOS 1:1 yet and must be reshaped for:
- AOS effect schemas (`sys/LlmGenerateParams@1`, `sys/LlmGenerateReceipt@1`),
- CAS-based message/tool refs,
- deterministic host/testing expectations.

## Decision Summary

1. Introduce a new crate: `crates/aos-llm`.
2. Use `tmp-llm-lib-code` as source material, not a direct copy.
3. Adapt AOS effect contracts as needed.
4. Keep provider/network complexity in `aos-llm`; keep `aos-host` adapter thin.
5. Streaming is supported as runtime telemetry/output events, not reducer-state mutation.
6. Preserve the working Demiurge wiring: CAS message refs in, CAS output ref out, reducer-driven tool loop.

## Non-Goals (P1)

- No kernel changes for agent concepts.
- No new plan opcodes.
- No cross-world orchestration.
- No final long-term provider catalog/governance policy design.

## Proposed Target Architecture

### `crates/aos-llm`

Responsibilities:
- provider adapters (`openai`, `anthropic`, future `gemini`),
- unified request/response/tool/event model,
- stream event normalization,
- retries/backoff (explicit and configurable),
- provider-specific escape hatches via controlled options.
- tests also migrated and passing.

### `crates/aos-host` integration

`crates/aos-host/src/adapters/llm.rs` becomes:
- decode AOS effect params,
- resolve message/tool refs from CAS,
- call `aos-llm`,
- write output blob(s) to CAS,
- return AOS receipt shape.

## Required Refactors from tmp Code

1. Rename/re-scope from `forge-llm` to AOS conventions.
2. Remove ambient/global default client usage in core runtime paths; prefer explicit client wiring.
3. Align message/tool representations with AOS CAS blob patterns.
4. Ensure streaming APIs can emit normalized events usable by shell/SSE surfaces.
5. Harden error mapping to AOS receipt status (`ok/error/timeout`) and deterministic test harnesses.
6. Keep secrets handling compatible with current resolver path (`api_key` resolved upstream).
7. Add a normalized LLM output envelope in CAS so reducers do not parse provider-specific JSON directly.
8. Keep provider-native output available in CAS for debugging/audit (`raw_output_ref`).

## Phase Plan

### Phase 1.1: Crate bootstrap + port
- Add `crates/aos-llm`.
- Port core types/client/provider traits from tmp code.
- Compile with workspace tooling and CI.

### Phase 1.2: Provider parity
- Port OpenAI + Anthropic adapters.
- Keep feature-gated provider modules where practical.
- Add provider contract tests (mocked).

### Phase 1.3: Host bridge
- Refactor `aos-host` LLM adapter to consume `aos-llm`.
- Evolve `llm.generate` request/receipt contracts as needed for normalized output refs.
- Verify tool refs + tool choice mapping parity with current Demiurge behavior.

### Phase 1.4: Streaming + conformance
- Normalize stream events across providers.
- Wire host/shell streaming path (non-deterministic telemetry only).
- Add cross-provider conformance tests (mocked + optional live).

## Testing

- Unit tests in `aos-llm` for types, adapters, stream normalization.
- Mocked provider integration tests (ported from tmp tests).
- Optional live tests (`OPENAI_*`, `ANTHROPIC_*`) ignored by default.
- `aos-host` integration tests to confirm effect receipt compatibility.

## Definition of Done

- `aos-llm` crate exists and is used by `aos-host` LLM adapter.
- OpenAI + Anthropic supported with unified request/tool/stream semantics.
- Existing Demiurge/tool flows keep working on top of the refactored adapter semantics.
- Test suite demonstrates deterministic adapter contracts and provider conformance.
- Reducer-facing parsing can rely on normalized output shape instead of provider-native response formats.

## Open Questions

- Should we split an additional crate (`aos-effects-llm`) later for reusable effect mapping, or keep mapping in `aos-host` until another host needs it?
- Which provider-specific options should be first-class in AOS schemas vs escape hatches?
- How much streaming detail should become API contract vs debug/telemetry only in v0.10?
