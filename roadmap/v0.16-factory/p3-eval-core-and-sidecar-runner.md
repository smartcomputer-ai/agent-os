# P3: Eval Core and Sidecar Factory Runner

**Priority**: P1  
**Effort**: High  
**Risk if deferred**: High (we will have harness scripts and tests but no reusable way to score them, compare them, or run the first real factory loop)  
**Status**: Proposed  
**Depends on**: `roadmap/v0.16-factory/p1-world-harness-core.md`, `roadmap/v0.16-factory/p2-persisted-local-and-python-bindings.md`, `roadmap/v0.16-factory/factory.md`

## Goal

Build the generic evaluation layer above harness execution and the first external sidecar runner that uses it to validate worlds.

Primary outcome:

1. harness scripts and harness runs become reusable evaluation input,
2. deterministic and judged lanes share the same evidence model,
3. `aos-agent-eval` evolves into a consumer of the shared eval core rather than remaining a special-case island,
4. the first factory runner exists outside AOS worlds.

## Problem Statement

Harness execution alone is not enough for the factory vision.

We also need:

1. repeated execution,
2. pass-rate thresholds,
3. evidence bundles,
4. rubric/judge support for fuzzy tasks,
5. cost/performance accounting,
6. holdout separation,
7. a runner that can execute these loops against worlds under test.

Without this layer we will keep building:

1. narrow one-off eval runners,
2. ad hoc scoring rules,
3. hand-inspected transcripts instead of reusable evidence.

## Design Stance

### 1) Treat eval as a layer above harness runs

Harness runs define:

1. what to run,
2. how to perturb it,
3. what immediate evidence to collect.

The eval layer defines:

1. how many runs to execute,
2. what thresholds matter,
3. which evidence is judged or aggregated,
4. how holdouts are kept separate,
5. how outcomes are summarized.

### 2) Keep suite definition minimal and eval-oriented

If eval introduces case or suite definitions, they should exist to support repeated execution and reporting.
They should not become the primary authoring surface for tests.

That means:

1. ordinary Rust and Python harness scripts remain first-class,
2. eval-specific suite config stays minimal,
3. we do not rebuild the harness as a framework-y metadata system here.

### 3) Keep sidecar-first as the implementation rule

The first factory runner is external to the worlds under test.

That runner may later:

1. submit work to AOS worlds,
2. call judge worlds,
3. delegate to planner/governor worlds.

But P3 does not require self-hosting the validator.

### 4) Preserve deterministic and judged lanes together

We need both:

1. deterministic pass/fail lanes,
2. deterministic-plus-judge lanes.

The shared eval core must support both without turning every scenario into a fuzzy LLM-scored task.

## Scope

### [ ] 1) Define the shared eval core

Responsibilities:

1. execute harness runs repeatedly,
2. collect structured evidence,
3. compute pass-rate thresholds,
4. emit per-run and aggregate summaries,
5. support deterministic-only and judged lanes.

Near-term product shape:

1. a shared eval library,
2. narrow runners built on top of it.

### [ ] 2) Standardize evidence bundles

Each eval run should be able to persist:

1. run identity and input summary,
2. harness artifacts,
3. trace summaries,
4. replay results,
5. state snapshots or selected reads,
6. transcripts/tool traces when relevant,
7. cost/performance metadata,
8. judge outputs when present.

The factory needs auditable evidence, not just terminal statuses.

### [ ] 3) Add judged/rubric lanes

Support optional rubric/judge evaluation where deterministic assertions are not sufficient.

Required capabilities:

1. judge input selection from the evidence bundle,
2. rubric attachment,
3. multiple attempts and thresholding,
4. separation between execution evidence and judge output.

Important rule:

- the judge augments the deterministic layer,
- it does not replace it.

### [ ] 4) Add pass-rate, cost, and performance accounting

The eval layer should track:

1. per-run pass/fail,
2. aggregate pass rate,
3. latency and step counts,
4. token/cost usage where applicable,
5. budget or threshold failures.

This is needed for real factory feedback loops, not only correctness checks.

### [ ] 5) Add holdout-suite support

The eval core should support:

1. development suites,
2. release-gate suites,
3. holdout suites.

Holdouts should be runnable through the same substrate while remaining separately managed and reported.

### [ ] 6) Add minimal suite/case orchestration where needed

The eval layer may need lightweight suite or case definitions for repeated execution.
Keep them eval-oriented and minimal.

Required outcome:

1. enough structure to select scripts/runs and apply thresholds,
2. no attempt to turn that structure into the primary test authoring language.

### [ ] 7) Evolve `aos-agent-eval` into a consumer

Near-term direction:

1. keep the existing prompt/tool case model where it fits,
2. factor out the generic execution/reporting pieces,
3. avoid forcing all future world evals into the current `aos-agent-eval` case schema.

Possible end-state:

1. retained `aos-agent-eval` as a narrow prompt/tool runner,
2. broader world/factory runners built on the shared eval core.

### [ ] 8) Build the first sidecar factory runner

The first runner should:

1. load eval suites,
2. execute them through the shared eval core,
3. export evidence bundles,
4. attach judges where configured,
5. produce summary outcomes suitable for iterative agent loops.

It is intentionally external to AOS worlds in this phase.

## Non-Goals

P3 does **not** attempt:

1. a fully AOS-native self-hosted factory,
2. complete planner/judge/governor world orchestration,
3. hosted production fleet scheduling for the factory itself,
4. replacing all human review with judge models,
5. forcing every suite to use rubric scoring.

## Deliverables

1. Shared eval core above harness execution.
2. Standard evidence bundle model.
3. Judge/rubric support for fuzzy tasks.
4. Pass-rate, cost, and performance accounting.
5. Holdout-suite support.
6. Minimal eval-oriented suite/case orchestration.
7. Refactored `aos-agent-eval` consuming the shared eval core.
8. First sidecar factory runner.

## Acceptance Criteria

1. The same harness substrate can power deterministic and judged eval lanes.
2. Eval runs emit structured evidence bundles that are sufficient for later audit and comparison.
3. Pass-rate and budget thresholds can be enforced across repeated scenario runs.
4. `aos-agent-eval` no longer owns unique harness/eval mechanics that future runners must duplicate.
5. The first factory runner exists outside AOS worlds and can evaluate worlds under test end to end.

## Recommended Implementation Order

1. define the evidence bundle and aggregate result model,
2. factor out the shared eval core,
3. add minimal suite/case orchestration,
4. refactor `aos-agent-eval` to consume it,
5. add judged lanes and accounting,
6. add holdout support,
7. build the first sidecar runner.
