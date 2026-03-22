# P1: Harness Core Library

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (smoke/eval/factory work will keep duplicating boot logic and drift across incompatible harness surfaces)  
**Status**: Completed  
**Depends on**: `roadmap/v0.16-factory/factory.md`, `roadmap/v0.14-infra/p3-crate-refactor.md`, `roadmap/v0.15-local/p3-local-state-root-single-world-first-and-fsstore-removal.md`

## Implementation Status

P1 is now landed across `aos-runtime`, `aos-kernel`, `aos-authoring`, `aos-smoke`, and `aos-agent-eval`:

1. shared `HarnessBuilder`, `HarnessCore`, `WorkflowHarness`, and `WorldHarness`,
2. virtual logical-time control in the kernel/runtime seam,
3. explicit quiescence reporting (`kernel-idle`, `runtime-quiescent`),
4. first-class scripted effect control for manual receipt choreography,
5. `Ephemeral` backend,
6. built-in receipt helpers for timer/blob/http/llm settlement,
7. structured artifact export and replay-check surfaces,
8. shared local bootstrap into `WorldHarness` from `aos-authoring`,
9. migration of `aos-smoke` and `aos-agent-eval` onto the shared substrate,
10. focused runtime and harness integration coverage.

## Goal

Extend `aos-runtime` with reusable harness APIs that make deterministic workflow and single-world testing accessible as a library surface rather than as a pile of custom Rust runners.

Primary outcome:

- `TestHost` remains the low-level host-driving primitive in `aos-runtime`,
- higher-level `WorkflowHarness` and `WorldHarness` APIs become the reusable public testing surface,
- `aos-smoke` and `aos-agent-eval` stop carrying separate copies of the same boot/patch logic,
- v1 harness work lands inside the single-world engine boundary rather than in a new permanent harness crate.

## Problem Statement

Today the repository already has the right ingredients, but not the right shape:

1. `TestHost` is a strong execution primitive.
2. `ExampleHost` layers useful workflow-harness behavior on top of it.
3. `EvalHost` duplicates much of that behavior for prompt/tool eval.
4. mock effect harnesses already exist in `aos-effect-adapters`.

What is missing is:

1. one first-class harness library surface in `aos-runtime`,
2. one shared core/builder/config pipeline that higher-level consumers can share,
3. one artifact/evidence model,
4. one explicit scripted effect mode for smoke-style receipt choreography,
5. one explicit virtual-time seam for timer-heavy deterministic tests and future schedule work,
6. one explicit quiescence model instead of overloading "idle".

## Design Stance

### 1) Keep the harness inside `aos-runtime`

We should not introduce a separate permanent harness crate for v1.

Recommended layering:

1. `WorldHost`
   - runtime engine.
2. `TestHost`
   - low-level execution helper,
   - thin over `WorldHost`,
   - useful for crate-local integration tests and advanced control.
3. `WorkflowHarness`
   - deterministic harness surface optimized for effect/receipt/time control.
4. `WorldHarness`
   - higher-level single-world harness surface,
   - state/effect/snapshot/replay/evidence helpers,
   - the main public harness surface for smoke, eval, and factory work.

Design rules:

1. keep `TestHost` small and runtime-oriented,
2. add higher-level harness APIs in `aos-runtime`, not in a new permanent testkit crate,
3. point user-facing harness docs at `WorkflowHarness` and `WorldHarness`, not `TestHost`.

### 2) Keep authored-world bootstrap in `aos-authoring`

The current bootstrap/import/reset helpers already live in `aos-authoring`.
That split should remain.

Implication:

1. `aos-runtime` owns core harness execution surfaces,
2. `aos-authoring` owns local bootstrap/import/reset helpers,
3. P1 should consolidate shared boot/patch logic with that boundary in mind rather than collapsing everything into one crate.

### 3) Define one shared harness core before public APIs harden

The first extraction should not be method names on a new facade.
It should be the shared harness core that owns:

1. module build/patch/bootstrap flow,
2. backend selection and config,
3. runtime open/reopen behavior,
4. artifact/evidence collection.

Today the duplicated shape is visible in `ExampleHost` and `EvalHost`.
P1 should collapse that duplication first so `WorkflowHarness` and `WorldHarness` become thin public wrappers rather than separate accumulation points.

### 4) Scripted effect control comes before bigger adapter abstraction

`aos-smoke` shows that the first useful harness surface must support:

1. pulling emitted intents,
2. holding or reordering receipts,
3. injecting synthetic success/error/timeout receipts,
4. doing that without a custom Rust adapter per test.

This is a first-class harness mode, not an afterthought.

Recommended v1 effect modes:

1. `scripted`
   - the harness caller pulls intents and applies receipts manually,
   - best for workflow tests and smoke-style effect choreography.
2. `twin`
   - reusable, stateful simulation of a dependency class,
   - best for more realistic dependency validation later.
3. `live`
   - actual external system integration,
   - best for sparse compatibility checks.

### 5) Build `Ephemeral` first

P1 centers on deterministic harness work.

That means:

1. the default backend is `Ephemeral`,
2. persisted-local realism is left for P2,
3. the interface should reserve a `PersistedLocal` backend from day one so P2 plugs into the same model.

### 6) Make logical time injectable before higher-level API freeze

Future `schedule.*` work will depend on this, but the need already exists for:

1. timer-set workflows,
2. retry/backoff logic,
3. deadline and expiry testing,
4. long-horizon scenarios that should not wait real time.

Required distinction:

1. execution logical time
   - used for workflow semantics and timer delivery,
   - must be controllable by the harness.
2. wall-clock/admin time
   - used for operator-facing metadata and control-plane timestamps,
   - may remain real time.

Current implementation reality matters here:

1. kernel logical time is still derived from elapsed host time,
2. ingress sampling depends on that sampled logical time,
3. therefore `time_set` and `time_advance` are runtime-seam work, not just harness convenience helpers.

P1 does **not** need first-class recurring schedule semantics.
It does need the runtime/harness seam that will let those semantics be tested later.

### 7) Make quiescence explicit

The harness surface should distinguish two different runtime states:

1. `kernel-idle`
   - no more immediate deterministic step work is available in the current kernel loop.
2. `runtime-quiescent`
   - no inbox, effect, receipt, or timer work is pending for the current backend execution path.

Manifest apply should use runtime quiescence as its safety gate rather than introducing a second redundant state label.
These states are related but not interchangeable.
P1 should model them explicitly rather than using one generic `run_to_idle` concept for all of them.

## Scope

### [x] 1) Define a shared harness core, builder, and evidence model

Extract the common harness machinery before the public wrapper types harden.

Required responsibilities:

1. shared world/module preparation and patching,
2. shared backend/config selection,
3. shared runtime open/reopen flow,
4. shared evidence/artifact collection,
5. thin-consumer support for smoke and eval.

Target outcome:

1. `ExampleHost` becomes a thin consumer or disappears,
2. `EvalHost` becomes a thin consumer or disappears,
3. `WorkflowHarness` and `WorldHarness` can layer on one shared substrate.

Completed:

1. `HarnessBuilder`, `HarnessCore`, `HarnessEvidence`, `WorkflowHarness`, and `WorldHarness` landed in `aos-runtime`,
2. the shared substrate owns timer state, cycle accounting, quiescence reads, state/effect/receipt helpers, `send_command`, `reopen`, `replay_check`, and artifact export,
3. `aos-smoke` `ExampleHost` and `aos-agent-eval` `EvalHost` now consume the shared substrate.

### [x] 2) Add virtual logical-time control

Add a clock seam so that deterministic execution time is harness-controlled rather than tied to real elapsed host time.

Required outcomes:

1. the harness can read current logical time,
2. the harness can set or advance logical time in test backends,
3. timer firing can be driven by logical time progression rather than sleeping,
4. long-horizon timer scenarios can run instantly,
5. the same seam can later support `schedule.*` testing.

Design rule:

1. timer and future schedule semantics should depend on logical execution time,
2. observability/admin timestamps may remain wall-clock based.

Current implementation reality:

1. kernel logical time is still derived from elapsed host time,
2. ingress sampling uses that logical time,
3. this means the clock seam belongs at the front of P1 rather than after the public harness shape is fixed.

Illustrative operations:

```rust
pub trait HarnessTimeControl {
    fn logical_now_ns(&self) -> u64;
    fn set_logical_time_ns(&mut self, now_ns: u64) -> Result<()>;
    fn advance_logical_time_ns(&mut self, delta_ns: u64) -> Result<u64>;
    fn advance_to_next_due(&mut self) -> Result<Option<u64>>;
}
```

### [x] 3) Introduce `WorkflowHarness` and `WorldHarness` in `aos-runtime`

Create reusable harness types that expose stable execution control without forcing custom Rust runners.

Recommended surface:

```rust
pub enum HarnessBackend {
    Ephemeral,
    PersistedLocal,
}

pub enum EffectMode {
    Scripted,
    Twin,
    Live,
}

pub struct HarnessCore { /* shared boot/backend/evidence substrate */ }
pub struct WorkflowHarness { /* deterministic workflow-focused harness */ }
pub struct WorldHarness { /* single-world harness */ }
```

Expected operations:

1. `send_event`
2. `send_command`
3. `run_until_kernel_idle`
4. `run_until_runtime_quiescent`
5. `run_cycle_batch`
6. `run_cycle_with_timers`
7. `quiescence_status`
8. `pull_effects`
9. `apply_receipt`
10. `snapshot`
11. `reopen`
12. `state`
13. `state_bytes`
14. `list_cells`
15. `trace_summary`
16. `replay_check`
17. `time_get`
18. `time_set`
19. `time_advance`
20. `time_advance_to_next_due`
21. `export_artifacts`

Important rule:

1. harnesses own reusable effect/receipt/time ergonomics,
2. the underlying runtime semantics still come from the same execution path as `TestHost`,
3. `TestHost` remains the low-level shim rather than growing into the public harness product.

Completed:

1. `WorkflowHarness` and `WorldHarness` are available over the shared `HarnessCore`,
2. the `Ephemeral` backend and `Scripted` / `Twin` / `Live` effect-mode enum surface are present,
3. `send_command`, `reopen`, `replay_check`, and `export_artifacts` are part of the public harness surface.

### [x] 4) Add explicit quiescence status

Add a first-class quiescence surface instead of treating all "idle" states as equivalent.

Required outcomes:

1. the harness can report `kernel-idle`,
2. the harness can report `runtime-quiescent`,
3. blocker details are visible when `runtime-quiescent` is false,
4. `run_until_*` helpers are aligned with those named states.

Illustrative shape:

```rust
pub struct QuiescenceStatus {
    pub kernel_idle: bool,
    pub runtime_quiescent: bool,
    pub blockers: Vec<String>,
}
```

### [x] 5) Add first-class scripted effect control

Add an explicit effect mode where tests/scripts can drive effects manually without authoring custom adapters.

Required capabilities:

1. pull all pending intents,
2. filter intents by kind,
3. hold or reorder receipts,
4. apply synthetic success/error/timeout receipts,
5. keep this flow deterministic and replay-compatible.

This is the core v1 replacement for many custom `aos-smoke` drivers.

### [x] 6) Add built-in receipt helpers

The harness library should make common built-in effect kinds easy to settle without hand-encoding every payload.

Priority helpers:

1. `http.request`
2. `llm.generate`
3. `blob.put`
4. `blob.get`
5. `timer.set`

These helpers should remain conveniences over generic `apply_receipt`, not a rigid DSL.

Completed:

1. generic `receipt_ok` / `receipt_error` / `receipt_timeout` helpers exist on the shared core,
2. built-in helpers cover `timer.set`, `blob.put`, `blob.get`, `http.request`, and `llm.generate`,
3. harness integration tests cover helper encoding and timer settlement flow.

### [x] 7) Implement the `Ephemeral` backend

P1 backend semantics:

1. fast deterministic execution,
2. suitable for workflow tests and tight conformance checks,
3. may use in-memory world persistence, in-memory journals, or temporary local state,
4. does not require the long-lived local daemon.

Allowed uses:

1. workflow logic testing,
2. scripted receipt choreography,
3. replay/snapshot invariants,
4. narrow single-world conformance scenarios.

### [x] 8) Standardize evidence/artifact export

Every harness run should be able to emit a standard artifact bundle:

1. manifest/module patch summary,
2. trace summary,
3. replay check result,
4. selected state reads,
5. optional transcripts/tool traces,
6. cost/perf metadata when applicable.

The harness needs to return more than booleans.

Completed:

1. `HarnessEvidence`, `HarnessArtifacts`, and `HarnessReplayReport` provide structured evidence,
2. exported artifacts include evidence, trace summary, and journal entries,
3. replay-check is part of the shared surface and is covered by integration tests.

## Required Refactors

### 1) `aos-runtime`

Keep the layering explicit inside the single-world engine crate.

Required result:

1. `TestHost` remains the low-level runtime test utility,
2. shared harness core/builder/evidence types land in `aos-runtime`,
3. `WorkflowHarness` and `WorldHarness` land in `aos-runtime`,
4. scripted effect control and receipt helpers do not accumulate directly on `TestHost` by default.

### 2) `aos-authoring`

Keep bootstrap/import/reset helpers in the authoring crate and extract shared helpers where smoke/eval currently duplicate them.

Required result:

1. local bootstrap helpers stay out of `aos-runtime` to avoid dependency inversion,
2. the harness layer can rely on `aos-authoring` for local-world preparation without bespoke helper code per consumer.

### 3) `aos-smoke`

Move smoke runners onto the shared harness substrate.

Required result:

1. no private host boot pipeline in smoke,
2. smoke stays focused on scenario logic and assertions,
3. smoke consumes the shared harness core rather than carrying its own boot/evidence layer.

Completed:

1. `ExampleHost` now wraps `WorldHarness`,
2. smoke bootstrapping uses the shared `aos-authoring` local harness bootstrap path.

### 4) `aos-agent-eval`

Move eval host setup onto the shared harness substrate.

Required result:

1. eval stays focused on prompt/tool execution and scoring,
2. harness boot and evidence behavior are shared with smoke and future factory runners,
3. eval consumes the shared harness core rather than carrying its own boot/evidence layer.

Completed:

1. `EvalHost` now wraps `WorldHarness`,
2. eval setup uses the shared placeholder-resolution and local harness bootstrap flow.

## Non-Goals

P1 does **not** attempt:

1. persisted-local runtime semantics in full,
2. Python bindings,
3. a framework-y metadata model or fixture DSL,
4. the generic eval core,
5. the sidecar factory runner,
6. complete twin implementations for every effect kind,
7. full `schedule.upsert` / cron / timezone / DST semantics,
8. turning `TestHost` into the full public harness product,
9. introducing a new permanent harness crate.

## Deliverables

1. Shared harness core/builder/evidence model in `aos-runtime`.
2. `WorkflowHarness` and `WorldHarness` APIs in `aos-runtime`.
3. Explicit quiescence status model with `kernel-idle` and `runtime-quiescent`.
4. Scripted effect control and built-in receipt helpers.
5. `Ephemeral` backend.
6. Virtual logical-time control in the harness/runtime seam.
7. Standard artifact/evidence export model.
8. Migration of smoke/eval host bootstrapping onto the shared substrate.
9. Clear layering: `WorldHost -> TestHost -> HarnessCore -> WorkflowHarness/WorldHarness`.

## Acceptance Criteria

1. A workflow author can write a deterministic harness test in Rust without building a bespoke runner binary.
2. The shared harness core owns boot/backend/evidence behavior used by both smoke and eval.
3. The shared harness can inject events, inspect effects, apply receipts, snapshot, reopen, and verify replay.
4. Scripted smoke-style receipt choreography works without custom adapters for each test.
5. The shared harness can advance logical execution time without waiting real time.
6. Long-horizon timer/backoff scenarios can be tested in seconds rather than elapsed schedule time.
7. The harness can distinguish `kernel-idle` and `runtime-quiescent`.
8. Smoke and eval use the same shared boot/patch/evidence substrate.
9. The harness returns structured evidence, not only pass/fail.
10. `TestHost` remains a small runtime-level shim instead of absorbing the public harness responsibilities.

## Recommended Implementation Order

1. extract the shared harness core/builder/evidence model,
2. add virtual logical-time control,
3. introduce `WorkflowHarness` and `WorldHarness` on top of the shared core,
4. add explicit quiescence surfaces,
5. add scripted effect control and receipt helpers,
6. wire the `Ephemeral` backend,
7. add artifact/evidence export,
8. migrate smoke/eval consumers.
