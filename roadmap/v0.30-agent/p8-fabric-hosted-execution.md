# P8: Fabric-Backed Hosted Execution

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (the agent can still improve locally, but hosted/sandbox execution will remain an adapter detail rather than a proven harness mode)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.30-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.30-agent/p7-run-traces-and-intervention.md`

## Goal

Prove that `aos-agent` can run the same host-tool harness against Fabric-backed sandbox sessions without making Fabric part of `aos-agent` core.

Primary outcome:

1. host tools remain canonical AOS effects,
2. Fabric stays in `aos-effect-adapters` and Fabric crates,
3. `aos-agent` host bundle/config can express sandbox target policy,
4. traces and intervention work against Fabric exec/session signals,
5. Demiurge can later choose local or Fabric-backed host execution deliberately.

## Current Fit

Fabric already exists as generic host/session edge infrastructure:

1. `fabric-protocol`, `fabric-client`, `fabric-controller`, `fabric-host`, and `fabric-cli` are separate from AOS agent core.
2. `aos-effect-adapters` has Fabric host adapters for canonical host session, exec, signal, and filesystem effects.
3. adapter routing can map canonical host effect routes to Fabric provider implementations when Fabric config is present.

The missing agent-roadmap piece is not "add Fabric to core."

The missing piece is:

1. explicit host target policy in tool/session config,
2. a fixture proving the same agent harness works with Fabric,
3. trace/intervention coverage for Fabric stream frames and signals,
4. documentation for how Demiurge should select Fabric later.

## Design Stance

### 1) Fabric is an execution backend, not an agent primitive

`aos-agent` should not depend on Fabric crates.

Agent contracts should speak in terms of:

1. host session target policy,
2. host tool effects,
3. stream frames,
4. receipts,
5. run traces.

Fabric-specific controller URLs, runtime classes, host registration, and scheduler details stay below the adapter/config layer.

### 2) Canonical host effects stay canonical

The LLM-facing host tools should continue to map to canonical AOS effects such as:

1. `sys/host.session.open@1`,
2. `sys/host.exec@1`,
3. `sys/host.session.signal@1`,
4. `sys/host.fs.*@1`.

Fabric is selected by adapter routing and host target policy, not by teaching the model a separate Fabric tool family.

### 3) Host target policy must be explicit

The current local default target is not enough for Fabric.

The host bundle/config needs to express:

1. local workdir target,
2. sandbox image/runtime target,
3. network mode,
4. mounts,
5. resource limits,
6. labels/ttl.

This policy can be supplied by the embedding world, Demiurge config, or a host bundle assembly helper.

### 4) Fabric should exercise traces and intervention

Fabric gives us useful proof points:

1. exec progress frames,
2. session signaling,
3. sandbox filesystem operations,
4. hosted execution failure modes.

P8 should reuse P7 trace and intervention contracts rather than invent separate hosted-agent observability.

## Scope

### [ ] 1) Define agent-level host target policy

Add or reuse source-agnostic config that can represent:

1. local host target,
2. sandbox host target,
3. default host session labels,
4. session ttl,
5. run-level target overrides.

Do not expose Fabric controller internals through `aos-agent` contracts.

### [ ] 2) Wire Fabric-ready host bundle assembly

Required outcome:

1. host-local bundle can keep local defaults,
2. host-sandbox bundle can emit a sandbox target for auto-open,
3. explicit pre-attached host sessions still work,
4. hosted agents can disable host auto-open entirely.

### [ ] 3) Add a Fabric-backed agent fixture

Add a focused fixture that proves:

1. session open targets Fabric sandbox,
2. host exec works,
3. filesystem read/write/patch path works,
4. traces record Fabric-backed progress/receipts,
5. failure behavior is typed and deterministic.

This can use a fake controller for deterministic tests and live Fabric tests behind explicit feature/env flags.

### [ ] 4) Verify intervention against Fabric

Required outcome:

1. interrupt/cancel emits `host.session.signal` when an active Fabric session supports it,
2. exec progress stream frames become run trace entries,
3. unsupported or failed signals produce deterministic trace/failure entries.

### [ ] 5) Document Demiurge selection

Document how a future Demiurge version chooses:

1. local host execution,
2. Fabric sandbox execution,
3. workspace-only execution,
4. no-host chat/debug execution.

The selection should be explicit in task/session config, not inferred from adapter availability alone.

## Non-Goals

P8 does **not** attempt:

1. moving Fabric crates under `aos-agent`,
2. final hosted fleet scheduling,
3. multi-tenant product API design,
4. Fabric marketplace or image management,
5. replacing workspace tools with Fabric filesystem tools.

## Acceptance Criteria

1. `aos-agent` has no Fabric crate dependency.
2. A Fabric-backed fixture runs canonical host tools through adapter routing.
3. Host target policy can express local and sandbox targets explicitly.
4. Run traces show Fabric-backed exec progress and receipts.
5. Interrupt/cancel can signal a Fabric-backed host session where supported.
6. Demiurge has a documented path to choose Fabric later without changing core session semantics.
