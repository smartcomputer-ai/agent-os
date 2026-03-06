# P5: Raise the Workflow Authoring Level

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md`, `roadmap/v0.13-demiurge2/p4-operator-ux-stuck-task-diagnosis.md`

## Goal

Reduce the amount of hand-written receipt-driven state-machine code required to
build and evolve agent workflows.

This slice adds a small higher-level primitive layer over:

1. `crates/aos-wasm-sdk/src/workflows.rs`
2. `crates/aos-agent/src/helpers/workflow.rs`

The target is not a new orchestration DSL. The target is to remove repetitive,
error-prone patterns from real workflows such as `SessionWorkflow` and
`worlds/demiurge`.

## Problem Statement

The current runtime model is correct but too manual at the authoring level:

1. authors hand-roll pending-intent bookkeeping,
2. each workflow manually maps effect receipts back into typestate,
3. retries/backoff policies are re-expressed per workflow,
4. lifecycle emission is easy to forget or implement inconsistently,
5. bootstrap/handoff flows between workflows require explicit low-level intent
   plumbing.

The result is a high abstraction tax exactly where new agent capabilities need
to move fastest.

## Design Principle

Do not build a fully declarative agent language.

Instead:

1. keep explicit workflow state,
2. keep explicit events and receipts,
3. add reusable typed helpers for the recurring effect/continuation patterns,
4. prove the layer by refactoring `SessionWorkflow` and Demiurge.

## Proposed Primitive Set

### 1) `request_llm`

Purpose:

1. materialize canonical `llm.generate` params,
2. emit the effect,
3. register the pending intent in workflow state,
4. return a typed handle or correlation token.

Current source material:

1. `crates/aos-agent/src/helpers/llm.rs`
2. `SessionEffectCommand::LlmGenerate`

### 2) `run_tool_batch`

Purpose:

1. take observed tool calls,
2. validate tool availability and policy at the workflow layer,
3. emit the relevant tool effects,
4. record the active batch and expected receipts,
5. produce deterministic batch completion when all receipts settle.

Current source material:

1. `ActiveToolBatch`
2. `ToolBatchPlan`
3. `on_tool_calls_observed` and related helpers in
   `crates/aos-agent/src/helpers/workflow.rs`

### 3) `await_receipt`

Purpose:

1. centralize receipt matching by intent id / params hash / effect kind,
2. decode typed receipt payloads,
3. route success / error / timeout / rejected cases through one helper,
4. reduce per-workflow receipt-envelope boilerplate.

This primitive should cover both:

1. a generic envelope matcher in `aos-wasm-sdk`, and
2. typed convenience adapters in `aos-agent`.

### 4) `retry_with_backoff`

Purpose:

1. standardize attempt counting,
2. standardize terminal retry classification,
3. schedule timer-backed retries deterministically,
4. keep failure ownership explicit.

This should reuse existing failure ownership ideas rather than inventing a
second retry system.

### 5) `emit_lifecycle`

Purpose:

1. centralize lifecycle transition + domain-event emission,
2. eliminate duplicated before/after lifecycle diff logic,
3. ensure session/task workflows publish lifecycle changes uniformly.

Current proof point:

1. `crates/aos-agent/src/bin/session_workflow.rs` already emits
   `aos.agent/SessionLifecycleChanged@1` after reducer transitions.

The new primitive should move this out of ad hoc workflow-local glue.

### 6) `spawn_or_handoff_session`

Purpose:

1. encapsulate the common orchestration path from an outer workflow into
   `aos.agent/SessionWorkflow@1`,
2. emit the necessary ingress events,
3. carry config/tool-profile/workdir/bootstrap parameters coherently,
4. support future delegation to another session or workflow instance without
   rewriting the bootstrap choreography.

This primitive is the most Demiurge-specific and may live in `aos-agent`
instead of the generic WASM SDK.

## Layering Plan

### Layer A: generic workflow helpers in `aos-wasm-sdk`

Keep this layer runtime-generic.

Candidate additions:

1. typed receipt matching helpers,
2. effect emission helpers with cap-slot handling,
3. timer/retry scheduling helpers,
4. lifecycle annotation/event helper patterns.

Suggested file target:

1. `crates/aos-wasm-sdk/src/workflow_primitives.rs`

Keep `workflows.rs` small; re-export the new helpers from `lib.rs`.

### Layer B: agent-specific helpers in `aos-agent`

Keep this layer opinionated around session/task workflows.

Candidate additions:

1. LLM request helper,
2. tool-batch orchestration helper,
3. lifecycle emission helper for session semantics,
4. session spawn/handoff helper.

Suggested file targets:

1. `crates/aos-agent/src/helpers/primitives.rs`
2. optional split files if the surface grows:
   - `helpers/receipts.rs`
   - `helpers/tool_batch.rs`
   - `helpers/spawn.rs`

## Refactor Strategy

### Phase 1: Extract, do not redesign

Lift existing proven code paths into helper functions with minimal behavior
change.

Priority order:

1. `emit_lifecycle`
2. `request_llm`
3. `await_receipt`
4. `run_tool_batch`
5. `retry_with_backoff`
6. `spawn_or_handoff_session`

### Phase 2: Refactor `SessionWorkflow`

Use the new helpers to shrink the main reducer path.

Expected outcomes:

1. fewer direct `SessionEffectCommand::*` construction sites,
2. fewer manual receipt decoding branches,
3. clearer transition boundaries between run phases.

### Phase 3: Refactor Demiurge

Use `spawn_or_handoff_session` plus the generic receipt helpers to simplify the
task bootstrap path.

Expected outcomes:

1. smaller bootstrap state machine,
2. consistent lifecycle/task-finish signaling,
3. less bespoke host-session bootstrap glue.

## Constraints

1. Do not hide durable state mutations behind magic macros.
2. Do not create implicit background schedulers inside the SDK.
3. Do not bypass explicit events/effects/receipts.
4. Keep deterministic replay obvious in the public API.
5. Avoid a “plan runtime in disguise”.

## Candidate API Direction

Illustrative only:

```rust
let llm = agent::request_llm(ctx, &mut state, request)?;
let outcome = workflow::await_receipt(event, llm.intent_id())?;

let retry = workflow::retry_with_backoff(&mut state.retry, outcome, RetryPolicy::default())?;
if let Some(timer) = retry.timer_effect() {
    timer.emit(ctx);
}
```

For handoff:

```rust
agent::spawn_or_handoff_session(
    ctx,
    session_id,
    host_session,
    input_ref,
    run_config,
)?;
```

The API must remain plain Rust over explicit state, not hidden runtime magic.

## Implementation Plan

### WP1: SDK primitive extraction

1. add generic receipt/effect convenience helpers to `aos-wasm-sdk`,
2. add tests for typed receipt matching and retry scheduling,
3. keep public re-export surface small and stable.

### WP2: Agent primitive extraction

1. move LLM request building into a reusable helper,
2. move lifecycle event emission into a reusable helper,
3. move tool-batch emission/settlement into reusable helpers.

### WP3: Refactor `SessionWorkflow`

Targets:

1. `crates/aos-agent/src/helpers/workflow.rs`
2. `crates/aos-agent/src/bin/session_workflow.rs`

Acceptance signal:

1. reducer/event flow gets shorter,
2. no behavior change in existing eval/smoke semantics.

### WP4: Refactor Demiurge

Targets:

1. `worlds/demiurge/workflow/src/lib.rs`

Acceptance signal:

1. bootstrap logic uses shared spawn/handoff and receipt helpers,
2. fewer bespoke continuation branches.

### WP5: Validation

Run existing verification paths after each extraction:

1. `cargo test -p aos-wasm-sdk`
2. `cargo test -p aos-agent`
3. `cargo run -p aos-smoke -- agent-session`
4. `cargo run -p aos-smoke -- agent-live`
5. `cargo run -p aos-agent-eval -- case <id>`
6. `cargo run -p aos-agent-eval -- case <id> --entry demiurge`

## Acceptance Criteria

1. The SDK exposes the primitive set needed for the recurring receipt/effect
   patterns listed above.
2. `SessionWorkflow` is measurably simpler after refactor and remains replay-safe.
3. Demiurge bootstrap/handoff logic is moved onto shared primitives.
4. No new runtime semantics are required; this is authoring-level lift only.
5. Eval and smoke paths continue to pass on both direct and Demiurge entry.

## Success Metric

This work is successful if building a new agent workflow no longer requires
copying large receipt-driven typestate blocks out of `SessionWorkflow` or
Demiurge.

If authors still have to start from those files and duplicate their control
logic, this slice failed even if the helpers compile.

## Follow-Ups

1. Add a small cookbook for common workflow patterns.
2. Add examples for timer-backed retry and session delegation.
3. Add a lint/checklist for workflows that emit effects without lifecycle or
   receipt-handling coverage.
