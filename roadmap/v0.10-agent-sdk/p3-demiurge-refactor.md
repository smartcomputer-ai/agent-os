# P3: Refactor Demiurge onto Agent SDK

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (Demiurge remains a special-case toy path)  
**Status**: Proposed

## Goal

Refactor `apps/demiurge` to consume `aos-agent-sdk` primitives so Demiurge becomes:
- a real reference implementation for AOS-native agents,
- a proving ground for coding-agent and world-operator flows,
- less bespoke reducer/plan code over time.

This document is intentionally lighter than P1/P2 because SDK contracts will evolve during implementation.

## Current State (Why Refactor)

Demiurge today is a useful prototype:
- chat request/response loop,
- tool call parsing/dispatch,
- workspace/introspect tools,
- debug trace support.

But it is still app-specific and embeds patterns the SDK should own.

## Refactor Direction

1. Keep shipping Demiurge while incrementally swapping internals to SDK primitives.
2. Prefer compatibility shims first, then cleanup/breaking schema changes once SDK stabilizes.
3. Treat Demiurge as the first major integration test for the new SDK.

## Non-Goals (P3)

- Not a full product redesign of shell UX.
- Not the final coding-agent implementation.
- Not final multi-world universe orchestration.

## Proposed Migration Slices

### Slice 3.1: Event/schema alignment
- Map existing Demiurge chat/tool events to SDK run/turn/action concepts.
- Add adapter layer to translate old events to new internals where needed.
- Keep existing UI flows operational.

### Slice 3.2: Reducer refactor
- Replace bespoke loop fields/state transitions with SDK reducer helpers.
- Standardize pending/waiting/error states to SDK conventions.
- Preserve deterministic behavior and request correlation guarantees.

### Slice 3.3: Plan/tool refactor
- Replace custom tool orchestration patterns with SDK plan templates/helpers.
- Keep current introspect/workspace tools functional.
- Add at least one additional tool path that validates SDK extensibility.

### Slice 3.4: Operational hardening
- Align debug surfaces with SDK terminology (run/turn/action lineage).
- Expand e2e tests to cover:
  - tool errors,
  - cap/policy denials,
  - multi-step tool continuation.

## Backward Compatibility Strategy

- Short-term: tolerate mixed mode (legacy Demiurge events + SDK internals).
- Mid-term: introduce versioned schemas for new contracts.
- End of v0.10: deprecate legacy-only paths once shell/API clients are migrated.

## Testing

- Preserve existing Demiurge e2e coverage as baseline.
- Add replay parity tests for migrated flows.
- Add SDK conformance tests that run against Demiurge as a fixture app.

## Definition of Done

- Demiurge core loop runs on SDK primitives rather than bespoke chat-only structures.
- Existing core features (chat + tools + debug trace) remain functional.
- At least one new agent behavior can be added in Demiurge with SDK extension points and minimal bespoke code.
- Legacy pathways are either removed or clearly marked deprecated with migration notes.

## Open Questions

- How aggressively should we rename public Demiurge schemas during v0.10?
- Do we keep one monolithic Demiurge reducer initially, or split by SDK domain boundaries?
- Which coding-agent features should land in Demiurge vs a separate app once SDK is ready?
