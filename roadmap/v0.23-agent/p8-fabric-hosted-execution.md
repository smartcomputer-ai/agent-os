# P8: Host Target Config and Fabric-Backed Hosted Execution

**Priority**: Split (host target config shape is P1; Fabric-backed proof is P2)  
**Effort**: Medium  
**Risk if deferred**: Medium (the agent can still improve locally, but hosted/sandbox execution will remain an adapter detail rather than a proven harness mode)  
**Status**: Complete for v0.30 SDK scope; Fabric-backed fixtures, intervention signaling, and Demiurge selection are deferred proof/product work  
**Depends on**: `roadmap/v0.23-agent/p4-tool-bundle-refactoring.md`, `roadmap/v0.23-agent/p7-run-traces-and-intervention.md`

## Goal

Define the agent-level host target config shape needed by P4/P5, then prove that `aos-agent` can run
the same host-tool harness against Fabric-backed sandbox sessions without making Fabric part of
`aos-agent` core.

Primary outcome:

1. host tools remain canonical AOS effects,
2. Fabric stays in `aos-effect-adapters` and Fabric crates,
3. `aos-agent` host bundle/config can express sandbox target config,
4. traces and intervention work against Fabric exec/session signals,
5. Demiurge can later choose local or Fabric-backed host execution deliberately.

The host target config shape is required for the v0.30 core roadmap so local-host assumptions do not
leak into `aos-agent`. The full Fabric-backed fixture/product proof can remain P2.

## Current Fit

Fabric already exists as generic host/session edge infrastructure:

1. `fabric-protocol`, `fabric-client`, `fabric-controller`, `fabric-host`, and `fabric-cli` are separate from AOS agent core.
2. `aos-effect-adapters` has Fabric host adapters for canonical host session, exec, signal, and filesystem effects.
3. adapter routing can map canonical host effect routes to Fabric provider implementations when Fabric config is present.

The missing agent-roadmap piece is not "add Fabric to core."

The missing piece is:

1. explicit host target config in tool/session config,
2. a fixture proving the same agent harness works with Fabric,
3. trace/intervention coverage for Fabric stream frames and signals,
4. documentation for how Demiurge should select Fabric later.

## Design Stance

### 1) Fabric is an execution backend, not an agent primitive

`aos-agent` should not depend on Fabric crates.

Agent contracts should speak in terms of:

1. host session target config,
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

Fabric is selected by adapter routing and host target config, not by teaching the model a separate Fabric tool family.

### 3) Host target config must be explicit

The current local default target is not enough for Fabric.

The host bundle/config needs to express:

1. local workdir target,
2. sandbox image/runtime target,
3. network mode,
4. mounts,
5. resource limits,
6. labels/ttl.

This config can be supplied by the embedding world, Demiurge config, or a host bundle assembly helper.

### 4) Fabric should exercise traces and intervention

Fabric gives us useful proof points:

1. exec progress frames,
2. session signaling,
3. sandbox filesystem operations,
4. hosted execution failure modes.

P8 should reuse P7 trace and intervention contracts rather than invent separate hosted-agent observability.

## Scope

### [x] 1) Define agent-level host target config (P1)

Add or reuse source-agnostic config that can represent:

1. local host target,
2. sandbox host target,
3. default host session labels,
4. session ttl,
5. run-level target overrides.

Do not expose Fabric controller internals through `aos-agent` contracts.

This is the only P8 part that should block the core P4/P5 cleanup.

Done:

1. `HostTargetConfig` represents local and sandbox targets without exposing Fabric controller internals.
2. `HostSessionOpenConfig` carries target, ttl, and labels through session defaults and run overrides.
3. sandbox targets map to canonical `sys/host.session.open@1` params.

### [x] 2) Wire sandbox-ready host bundle assembly (P1)

Required outcome:

1. host-local bundle can keep local defaults,
2. host-sandbox bundle can emit a sandbox target for auto-open,
3. explicit pre-attached host sessions still work,
4. hosted agents can disable host auto-open entirely.

This should stay backend-neutral. Fabric is one implementation of the sandbox target config.

Done:

1. local and sandbox host tool bundles are explicit and backend-neutral.
2. host auto-open is opt-in through `HostSessionOpenConfig`.
3. pre-attached sessions still flow through `HostSessionUpdated`.
4. workspace-only and no-host modes remain representable by omitting host tools and/or host auto-open config.

### [x] 3) Defer Fabric-backed agent fixture (P2)

Add a focused fixture that proves:

1. session open targets Fabric sandbox,
2. host exec works,
3. filesystem read/write/patch path works,
4. traces record Fabric-backed progress/receipts,
5. failure behavior is typed and deterministic.

This can use a fake controller for deterministic tests and live Fabric tests behind explicit feature/env flags.
The deterministic path should be expressed through `aos-harness-py` as described in
`roadmap/v0.23-agent/p10-agent-sdk-testing.md`; live Fabric coverage should remain an explicit
gated acceptance lane.

Deferred:

1. `aos-agent` already speaks canonical host effects and traces stream frames/receipts.
2. adapter-level Fabric coverage belongs in `aos-effect-adapters` and Fabric crates.
3. cross-crate deterministic harness coverage belongs with P10.

### [x] 4) Defer intervention against Fabric (P2)

Required outcome:

1. interrupt/cancel emits `host.session.signal` when an active Fabric session supports it,
2. exec progress stream frames become run trace entries,
3. unsupported or failed signals produce deterministic trace/failure entries.

Deferred:

1. P7 defines deterministic run interruption at the agent level.
2. emitting `sys/host.session.signal@1` on interrupt/cancel is a later host/Fabric policy layer.
3. adapter signal receipts and stream frames can re-enter through the existing trace path.

### [x] 5) Defer Demiurge selection docs (P2)

Document how a future Demiurge version chooses:

1. local host execution,
2. Fabric sandbox execution,
3. workspace-only execution,
4. no-host chat/debug execution.

The selection should be explicit in task/session config, not inferred from adapter availability alone.

Deferred:

1. the SDK exposes the config shape Demiurge needs.
2. the actual selection policy belongs to Demiurge/task configuration, not `aos-agent` core.

## Non-Goals

P8 does **not** attempt:

1. moving Fabric crates under `aos-agent`,
2. final hosted fleet scheduling,
3. multi-tenant product API design,
4. Fabric marketplace or image management,
5. replacing workspace tools with Fabric filesystem tools,
6. policy/capability gating for host execution.

## Acceptance Criteria

1. [x] `aos-agent` has no Fabric crate dependency.
2. [x] Host target config can express local and sandbox targets explicitly.
3. [x] Host target config is source-agnostic and does not expose Fabric controller internals.
4. [x] Host-local, host-sandbox, pre-attached-host, workspace-only, and no-host modes are representable.
5. [x] Fabric-backed fixtures are deferred; the SDK already emits canonical host tools through adapter routing.
6. [x] Fabric-backed exec progress and receipts are deferred; P7 traces already cover stream frames and receipts generically.
7. [x] Fabric host-session interrupt/cancel signaling is deferred to host/Fabric policy work.
8. [x] Deterministic Fabric adapter behavior through `aos-harness-py` is deferred to P10/live-gated harness work.
9. [x] Demiurge can choose Fabric later through explicit session/run host target config without changing core session semantics.
