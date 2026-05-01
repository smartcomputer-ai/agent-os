# P4: Tool Bundles and Execution Surfaces

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (`aos-agent` will keep hardening around one accidental coding-agent tool surface, making session, context, hosted execution, and Demiurge integration harder to clean up later)  
**Status**: Complete  
**Depends on**: `roadmap/v0.21-air-v2/`, `roadmap/v0.22-dx/p2-migrate-rust-authored-agent-demiurge.md`

## Goal

Make `aos-agent` core tool-surface agnostic and move opinionated tools into explicit bundles.

Primary outcome:

1. session and run contracts remain generic,
2. host, workspace, inspect, and future domain tools become optional bundles,
3. bundle selection is explicit in library composition and evented `SessionWorkflow` composition,
4. AIR v2 emitted-effect surfaces are explicit and do not imply accidental host or workspace access,
5. domain-event tools can be assembled without baking any factory-specific API into the core SDK,
6. Demiurge and eval fixtures stop depending on the current implicit coding-agent preset.

## Current Fit

This item still maps directly onto the current codebase.

Current `aos-agent` has improved since the first roadmap draft:

1. AIR is Rust-authored and generated from `aos-agent` source.
2. Demiurge imports `aos-agent` through Rust-authored package metadata.
3. prompt refs are hash refs rather than workspace paths.
4. session bootstrap can be driven through library helpers.

Before this P4 slice, the tool boundary was too broad:

1. `default_tool_registry()` mixes host, inspect, and workspace-facing tools.
2. `default_tool_profiles()` expose a coding-agent shaped default.
3. `SessionState::default()` installs that registry and `openai` profile by default.
4. `SessionWorkflow` declares all optional host, inspect, and workspace effects in one workflow allowlist.
5. workspace composite tools are special-cased inside the generic tool runner.
6. host-session auto-open assumes one host target story instead of explicit local/sandbox config.

P4 addressed the SDK default, explicit bundle assembly, composite-tool seam, host-target config,
AIR/package documentation, and representative consumer migration. Follow-on roadmap items still
own session/run lifecycle changes, context, tracing/intervention, hosted execution product flows,
and skills.

The core session kernel should not care whether a world chooses:

1. no tools,
2. inspect-only tools,
3. local host tools,
4. Fabric-backed sandbox host tools,
5. workspace tools,
6. a world-specific domain-event bundle.

## Design Stance

### 1) Keep the session kernel tool-surface agnostic

`aos-agent` core should own:

1. session and run contracts,
2. transcript and tool-batch state,
3. LLM turn orchestration,
4. generic tool planning and receipt settlement,
5. extension seams for bundle-specific execution.

It should not assume that host tools, workspace tools, or any other bundle are present.

Domain-event tools are part of the same generic seam. The core runner can support tools that emit
typed domain events, but it should not know whether those events represent factory work items,
review requests, chat handoff, or another world-specific process.

### 2) Keep built-in bundles inside `aos-agent` for now

The shipped bundles should remain in `aos-agent` until the extension seams stabilize.

Recommended internal split:

1. core session kernel,
2. built-in tool bundles,
3. bundle-specific runners/mappers,
4. thin assembly helpers for embedding worlds.

Do not split these into separate crates in this slice.

### 3) Make bundle selection explicit

The first API should optimize for clear assembly, not a general plugin framework.

Illustrative shape:

```rust
let registry = ToolRegistryBuilder::new()
    .with_bundle(tool_bundle_inspect())
    .with_bundle(tool_bundle_workspace())
    .with_tool(custom_tool_spec())
    .without_tool("workspace.commit")
    .build()?;
```

Equivalent map-based helpers are acceptable if they keep the same properties:

1. bundle constructors return ordinary tool definitions,
2. embedding worlds can add, remove, or override individual tools,
3. profiles are assembled explicitly by the caller,
4. no hidden preset is required to get useful behavior.

### 4) Treat local host, Fabric host, and workspace as distinct choices

Host tools are not a single target model.

At minimum the roadmap needs to distinguish:

1. local host tools against a local workdir,
2. Fabric-backed sandbox host tools against a controller-selected session,
3. workspace tools against versioned AOS workspace trees.

The same LLM-facing tool names may be reused, but session/run configuration must make the target
explicit. The current auto-open path that emits a default local target is not enough for
Fabric-backed agents.

### 5) Keep AIR v2 effect surfaces honest

Because `SessionWorkflow` is Rust-authored AIR, bundle selection is not only a registry issue.

We need a deliberate answer for emitted effects:

1. keep one broad reusable `SessionWorkflow` for now if needed,
2. document that broad workflow as the "full adapter" surface,
3. support slimmer wrapper workflows through direct library composition or generated variants later,
4. avoid implying that importing `aos-agent` grants host/workspace access.

The immediate slice can keep one broad workflow, but docs and builders must stop treating broad access as the base model.

## Scope

### [x] 1) Define the code boundary

Document and implement the internal split:

1. session kernel contracts and reducer helpers,
2. built-in bundles,
3. bundle-specific execution hooks,
4. adapter/workflow assembly helpers.

The public API should expose the assembly pieces intentionally instead of hiding them under `#[doc(hidden)]` if downstream worlds are expected to use them.

Done:

1. `SessionState::default()` now starts with an empty tool registry, empty profiles, and no selected profile.
2. the previous broad coding-agent surface is available only through explicit `local_coding_agent_*` helpers.
3. built-in tool assembly is exposed through public bundle constructors and builder APIs in `aos-agent`.
4. generic workflow helpers no longer depend on provider defaults to select a hidden tool surface.

### [x] 2) Add explicit bundle constructors and registry builders

Add bundle constructors for at least:

1. inspect,
2. host-local,
3. host-sandbox/Fabric-ready,
4. workspace.

Also make sure custom/domain-event bundles can be assembled through the same registry/profile API.
This does not require shipping a factory bundle in `aos-agent`.

Add registry/profile builders that support:

1. bundle merge,
2. per-tool include/exclude,
3. deterministic conflict handling,
4. validation of canonical `tool_id` values and LLM-facing `tool_name` uniqueness.

Done:

1. added `tool_bundle_inspect()`, `tool_bundle_host_local()`, `tool_bundle_host_sandbox()`, and `tool_bundle_workspace()`.
2. added smaller host pieces, `tool_bundle_host_session()` and `tool_bundle_host_fs()`, for custom assembly.
3. added `ToolRegistryBuilder` with bundle merge, single-tool override, per-tool removal, and final registry validation.
4. added `ToolProfileBuilder` so profiles can be assembled independently and validated against a chosen registry.
5. kept the local coding profile as an explicit compatibility helper instead of an SDK default.

### [x] 3) Move composite execution behind bundle seams

The generic tool runner should stop hardcoding workspace internals.

Required outcome:

1. workspace composite tools run behind a workspace runner/hook,
2. future domain composite tools can use the same seam,
3. host-specific behavior such as auto-open is expressed through host bundle config,
4. domain-event tools can emit typed domain events without becoming host or workspace effects,
5. core planning remains generic over accepted tool calls, parallelism hints, domain events, and receipts.

Done:

1. added a generic composite-tool seam: `is_composite_tool_mapper`, `start_composite_tool`, `continue_composite_tool`, and `resume_composite_tool`.
2. moved workspace composite execution behind that seam; workspace remains the first backend, not a special case in the tool-batch runner.
3. kept tool planning and receipt settlement generic over mapper, executor, parallelism hints, domain events, and LLM results.
4. preserved workspace composite coverage with deterministic reducer tests.

### [x] 4) Make host target config explicit

Represent host-session target config at assembly/config time.

Required outcome:

1. local host auto-open can still target a local workdir,
2. Fabric-backed host auto-open can target a sandbox target,
3. hosted agents can choose workspace-only with no host auto-open,
4. a chat-only agent can have no tool registry at all.

This slice does not need to finish all Fabric product behavior; it only needs to avoid baking local-host assumptions into `aos-agent` core.

Done:

1. added `HostSessionOpenConfig`, `HostTargetConfig::Local`, `HostTargetConfig::Sandbox`, and `HostMountConfig` to session/run config contracts.
2. auto-open now requires explicit session or run host-open config; there is no implicit local-host default.
3. run-level host-open config overrides the session default.
4. local and sandbox host-open configs are converted into `sys/host.session.open@1` params through the same core path.
5. reducer tests cover local auto-open, sandbox auto-open, host-tools-without-auto-open, and chat-only no-tool sessions.

### [x] 5) Update Rust-authored AIR and generated package docs

Keep generated AIR aligned with the new boundary.

Required outcome:

1. generated AIR remains checked against Rust source,
2. emitted effects are documented as the full adapter surface,
3. optional bundle/effect surfaces are described in `aos-agent` docs,
4. consumers understand when they are using evented broad `SessionWorkflow` versus direct wrapper composition.

Done:

1. crate docs now state that `SessionState::default()` is chat/no-tools and that built-in bundles are opt-in.
2. `crates/aos-agent/air/README.md` documents the broad evented `SessionWorkflow` as the full adapter surface.
3. docs describe chat-only, inspect-only, local coding, sandbox host, and workspace-only assembly shapes.
4. `aos air check` verifies checked-in generated AIR remains aligned with the Rust-authored source.

### [x] 6) Migrate representative consumers

Update:

1. Demiurge to select bundles explicitly,
2. `aos-agent-eval` cases to install explicit bundles/profiles for live acceptance,
3. `aos-harness-py` agent fixtures to install explicit bundles/profiles for deterministic SDK tests,
4. one local-coding fixture proving host plus inspect,
5. one hosted-style fixture proving workspace without host.

Done:

1. Demiurge validates allowed tools against the explicit local-coding registry instead of the empty SDK default.
2. `aos-agent-eval` installs the explicit local-coding registry/profiles for live acceptance cases.
3. `aos-smoke` agent-tools assembles host, inspect, and workspace bundles through `ToolRegistryBuilder`.
4. direct `SessionConfig` consumers in smoke fixtures set `default_host_session_open: None` explicitly.
5. `aos-harness-py` has no current agent fixture references to migrate.

## Non-Goals

P4 does **not** attempt:

1. the session/run lifecycle split,
2. the turn planner,
3. final interrupt/steer semantics,
4. final Fabric hosted-agent product flow,
5. subagent/session-tree semantics,
6. splitting bundles into separate crates,
7. factory work-item or worker-invocation workflows,
8. skill packaging or marketplace design.

## Acceptance Criteria

1. `aos-agent` can build an empty/no-tool registry.
2. Built-in host, workspace, and inspect bundles can be selected independently.
3. Existing local coding eval behavior survives through explicit bundle selection.
4. Deterministic Python harness fixtures can select the same explicit bundles without live provider credentials.
5. Workspace composite behavior is no longer hardwired into generic tool-batch planning.
6. A custom/domain-event bundle can map tool calls to typed domain events without changing core session semantics.
7. Host target config is explicit enough that local and Fabric-backed sessions do not require different core agent semantics.
8. Generated AIR remains deterministic and checked in sync with Rust source.
