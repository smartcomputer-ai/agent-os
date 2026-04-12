# v0.16 Factory

This folder turns the broad factory vision in [factory.md](/Users/lukas/dev/aos/roadmap/v0.16-factory/factory.md) into executable roadmap slices.

The focus of v0.16 is not the full self-hosted factory.
The focus is the validation substrate that the factory needs before it can be trusted:

1. deterministic harness library surfaces in Rust and Python,
2. realistic persisted-local world harnesses,
3. generic evaluation above harness execution,
4. the first sidecar factory runner.

## Design Stance

### 1) Separate harness and eval concerns

We should treat these as distinct layers:

1. `WorkflowHarness`
   - deterministic,
   - usually ephemeral,
   - focused on one workflow or a very small world,
   - optimized for deep control of effects, receipts, and time.
2. `WorldHarness`
   - realistic single-world execution,
   - persisted local runtime path when realism matters,
   - snapshots/reopen/control-plane semantics in scope.
3. `FactoryEval`
   - repeated harness execution,
   - evidence collection,
   - deterministic assertions plus optional rubric/judge logic,
   - pass-rate, cost, and holdout tracking.

### 2) Library-first, not framework-first

The first useful product is a harness library, not a scenario framework.

The intended order is:

1. reusable Rust harness APIs,
2. Python bindings as the first-class scripting surface,
3. ordinary Python scripts for authored tests,
4. only thin CLI/debug helpers where they materially help,
5. no large fixture DSL or mandatory metadata model in v0.16.

### 3) Keep the crate boundaries honest

v0.16 should build on the single-world engine boundary that already exists.

That means:

1. core harness types belong in `aos-runtime`,
2. authored-world bootstrap/reset/import helpers belong in `aos-authoring`,
3. a dedicated Python wrapper crate may compose those two,
4. we should not introduce a new permanent harness/testkit crate.

### 4) `PersistedLocal` means the existing local-node path

`PersistedLocal` should not become a second persisted runtime implementation.
It should explicitly mean the local state-root plus local persistence plus `HostedStore` plus `HotWorld` path that v0.15 already established as the canonical realistic local execution model.

P2 should productize that path behind the harness surface.
It should not invent a parallel backend with different semantics.

### 5) Freeze public harness APIs after the shared core exists

The immediate duplication is not just missing method names.
It is duplicated bootstrap, module patching, runtime setup, and evidence collection split across smoke/eval helpers.

That means P1 should first define one shared harness core, builder, and evidence model.
`WorkflowHarness` and `WorldHarness` should then be thin public facades over that shared core rather than the first place where shared behavior is invented.

### 6) Scripted effect control is a first-class v1 mode

`aos-smoke` shows that the immediate need is not a big adapter framework.
The immediate need is being able to:

1. pull emitted intents,
2. hold or reorder receipts,
3. inject synthetic success/error/timeout receipts,
4. advance logical time explicitly,
5. do all of that without writing a custom Rust adapter per test.

`twin` adapters are still important, especially for realistic world tests.
They are not the blocker to the first useful library surface.

### 7) Treat execution time as virtualizable from the start

Future schedule work will depend on this.

The harness/runtime model should distinguish:

1. execution time
   - affects workflow behavior,
   - timer delivery,
   - backoff and deadline progression,
   - future schedule evaluation,
   - must be virtualizable and steerable by the harness.
2. admin/observability wall time
   - operator-facing timestamps,
   - resource freshness and control-plane metadata,
   - may remain real wall-clock time.

This means v0.16 should prepare the clock seam at the front of P1, even though first-class
`schedule.*` semantics are not yet part of the milestone.
`time_set` and `time_advance` are not just harness sugar if the runtime still derives logical time from elapsed host time.

### 8) Quiescence must be explicit

The harness surface should distinguish:

1. kernel-idle,
2. runtime-quiescent.

These are not interchangeable.
Manifest apply uses runtime quiescence as a safety gate, so the harness should expose that runtime state directly instead of collapsing everything into a generic `run_to_idle`.

### 9) Sidecar-first factory

The first factory implementation is sidecar-first.

That means:

1. AOS worlds are the units under test,
2. the harness and first factory runner live outside the worlds they validate,
3. AOS-native planner/judge/governor worlds are follow-on work after the external validator is stable.

## Phase Map

### [P1: Harness Core Library](/Users/lukas/dev/aos/roadmap/v0.16-factory/p1-world-harness-core.md)

Build the reusable harness substrate inside `aos-runtime`:

1. one shared harness core, builder, and evidence model,
2. virtual logical-time control in the runtime seam,
3. `WorkflowHarness` and `WorldHarness` on top of low-level `TestHost`,
4. explicit quiescence surfaces,
5. scripted/manual effect control plus receipt helpers,
6. `Ephemeral` backend,
7. migration target for `aos-smoke` and `aos-agent-eval`.

### [P2: Persisted Local and Python Bindings](/Users/lukas/dev/aos/roadmap/v0.16-factory/p2-persisted-local-and-python-bindings.md)

Build the realistic local-world path and the first-class Python authoring surface:

1. `PersistedLocal` backend over the existing local-node / `HostedStore` / `HotWorld` path,
2. shared bootstrap helpers in `aos-authoring`,
3. dedicated Python bindings package,
4. Python receipt helpers for scripted effect flows,
5. first AI-authorable Python harness scripts,
6. long-horizon time-travel coverage.

### [P3: Eval Core and Sidecar Factory Runner](/Users/lukas/dev/aos/roadmap/v0.16-factory/p3-eval-core-and-sidecar-runner.md)

Build the generic evaluation and first factory execution layer:

1. repeated harness execution,
2. evidence bundles,
3. deterministic and judged lanes,
4. pass-rate and economics,
5. holdout support,
6. first external sidecar factory runner.

## Agent Capability Follow-Ons

The first three slices above establish the validation substrate.
The next required slices are agent-facing refactors that keep the factory-capable
agent stack composable instead of hard-wiring one world-specific implementation
into `aos-agent`.

### [P4: Tool Bundle Refactoring for Agent Core](/Users/lukas/dev/aos/roadmap/v0.16-factory/p4-tool-bundle-refactoring.md)

Make `aos-agent` core tool-surface agnostic and move opinionated tools into
explicit bundles that stay in `aos-agent` for now:

1. split session kernel from tool bundles,
2. treat host and workspace tooling symmetrically as optional bundles,
3. make bundle and per-tool selection explicit,
4. keep bundles easily extendable by embedding worlds,
5. slim the base AIR story accordingly.

### [P5: Overridable Context Engine](/Users/lukas/dev/aos/roadmap/v0.16-factory/p5-context-engine.md)

Add the first-class context assembly API that factory agents will actually need:

1. deterministic run-scoped context planning,
2. overridable engine hooks for worlds that link `aos-agent`,
3. context reports and inspection surfaces,
4. source-agnostic context inputs,
5. explicit compaction/summarization seams.

### [P6: Session Management and Context Scoping](/Users/lukas/dev/aos/roadmap/v0.16-factory/p6-session-management-improvements.md)

Clarify the durable session model so it composes cleanly with the context
engine:

1. separate durable session state from per-run lifecycle,
2. define world/session/run context scopes,
3. support multi-run sessions,
4. improve session telemetry and control surfaces,
5. let Demiurge wrap the library directly when that is the cleaner fit.

### [P7: Skills as an Implementation-Layer Feature](/Users/lukas/dev/aos/roadmap/v0.16-factory/p7-skills.md)

Define skills after the context and session seams are in place:

1. treat skills as optional context/tool bundles,
2. keep skills above the core session SDK,
3. allow workspace, CAS, or static skill sources,
4. route skill resolution through world policy and the context engine,
5. keep repo-local instruction files as one possible source, not the core model.

## Test Taxonomy

### L0) Unit/module tests

Purpose:

1. helper logic,
2. small state transitions,
3. edge conditions.

### L1) Workflow harness tests

Purpose:

1. workflow orchestration correctness,
2. scripted synthetic effect flows,
3. replay/snapshot invariants,
4. keyed-instance routing and cross-talk checks.

Primary surfaces:

1. Rust harness API,
2. Python harness bindings.

Default backend:

1. `Ephemeral`

### L2) World harness tests

Purpose:

1. realistic world bootstrapping,
2. multi-workflow coordination,
3. persisted-local runtime semantics,
4. reopen/snapshot/control operations.

Primary surface:

1. Python harness bindings with persisted-local bootstrap helpers.

Default backend:

1. `PersistedLocal`

### L3) Factory evals

Purpose:

1. end-to-end workload assessment,
2. rubric/judge evaluation,
3. pass-rate and cost tracking,
4. holdout regression detection,
5. live interop where explicitly required.

Default backend:

1. `PersistedLocal`

## v0.16 Exit Condition

v0.16 is successful when:

1. a workflow author can write deep deterministic tests in Rust or Python without building a bespoke Rust runner binary,
2. a world author can run realistic persisted-local tests from Python through the shared bindings and bootstrap helpers,
3. smoke-style receipt choreography works without writing custom adapters for each test,
4. timer-heavy and long-horizon scenarios can advance execution time without waiting real time,
5. `TestHost` remains available as a small low-level shim inside `aos-runtime` without becoming the main public harness product,
6. a shared eval core can run deterministic and judged suites,
7. the first sidecar factory runner can execute and score suites against worlds under test.
