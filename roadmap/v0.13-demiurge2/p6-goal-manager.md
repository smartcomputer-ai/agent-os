# P6: GoalManager Manager-Worker Agent

**Priority**: P1  
**Status**: Proposed  
**Depends on**: `roadmap/v0.13-demiurge2/p1-demiurge2-task-orchestrator.md`, `roadmap/v0.13-demiurge2/p5-workflow-authoring-primitives.md`

## Goal

Replace the thin task bootstrapper with a durable top-level agent that owns a
goal over time and coordinates specialized worker sessions to achieve it.

This is the follow-on step after the current task-driven Demiurge. The core
idea is:

1. keep AOS workflow-native,
2. do not add a high-level DAG DSL,
3. let a root manager agent decide what to do next from observations,
4. keep the manager's control loop explicit and replay-safe.

## Why This Instead of a Workflow DSL

Static graph/DAG authoring assumes the important nodes and transitions are known
up front.

That is the wrong default for agentic work such as coding, debugging, research,
or long-running operational tasks:

1. the agent often cannot know the full path in advance,
2. retries need new steering based on fresh evidence,
3. subtasks appear dynamically when something fails,
4. a second worker may be needed with a different tool profile,
5. "done" is often a judgment call over artifacts and runtime feedback.

In AOS terms, the simplification is not "remove workflow state". The
simplification is:

1. keep a very small deterministic manager state machine,
2. move the open-ended workflow into agent-managed state and decisions.

## Architectural Shape

`GoalManager` should be a keyed workflow module, one cell per root goal.

Recommended world shape:

1. `demiurge/GoalManager@1` owns goal state and coordination
2. `aos.agent/SessionWorkflow@1` remains the reusable worker-session runtime
3. child sessions are addressed by `aos.agent/SessionIngress@1`
4. manager consumes worker lifecycle and worker checkpoint/outcome events

This builds directly on keyed workflow cells:

1. one root goal cell,
2. zero or more child worker session cells,
3. deterministic interleaving by the kernel,
4. receipt continuation handled per cell as usual.

## Scope

### In scope

1. Root goal state and lifecycle.
2. Child worker session roster.
3. Worker spawn, steer, follow-up, cancel, and retry.
4. Goal-level completion / failure / escalation decisions.
5. Operator-readable progress and blocker ownership.

### Out of scope

1. A static workflow graph language.
2. A plan runtime separate from workflows.
3. Autonomous governance/apply semantics in this slice.
4. General multi-tenant scheduler policy.

## Public Ingress Strategy

To avoid churn, the first GoalManager version should keep the current public
task-style ingress and reinterpret it internally as a root goal:

1. continue accepting `demiurge/TaskSubmitted@1`,
2. treat `task_id` as the root goal id,
3. keep CLI and existing task-oriented tooling compatible.

Optional later follow-up:

1. add `demiurge/GoalSubmitted@1` as an alias or clearer public contract.

## Responsibility Split

### GoalManager owns

1. root goal state,
2. current hypotheses / open work items,
3. child worker roster,
4. delegation and retry decisions,
5. finish / fail / cancel decisions,
6. root-level status events.

### SessionWorkflow owns

1. the local LLM/tool loop,
2. tool-batch planning and execution,
3. worker-local lifecycle,
4. worker-local receipts and follow-up turns.

### Kernel and adapters own

1. deterministic stepping,
2. capability and policy enforcement,
3. effect queueing and receipt delivery.

## Goal State Model

The manager state should be explicit and readable. A reasonable initial shape:

1. `goal_id`
2. `status`
3. `objective_ref` or normalized task text
4. `accepted_output_criteria`
5. `open_work_items`
6. `child_workers`
7. `artifacts`
8. `current_blocker`
9. `review_queue`
10. `attempt_counters`
11. `last_progress_at_ns`

The important point is that the state should describe the evolving work, not a
pre-authored list of static workflow nodes.

## Worker Model

Each child worker should carry enough metadata for the manager and operator to
reason about it:

1. `session_id`
2. `worker_role` or specialization
3. `status`
4. `spawn_reason`
5. `workdir` or workspace binding
6. `run_config`
7. `latest_checkpoint_ref`
8. `latest_blocker`
9. `owned_work_items`

Examples of specialization:

1. coder
2. verifier
3. search/research worker
4. migration worker
5. recovery/debug worker

Specialization can start soft through prompt refs and tool profiles. Harder
capability separation can come later through separate modules or tighter
bindings.

## Core Control Loop

The manager loop should be small and explicit.

### 1) Goal start

On root ingress:

1. persist the goal input,
2. create the initial work item,
3. spawn the first worker session.

### 2) Wait for worker signals

The manager consumes:

1. `SessionLifecycleChanged`
2. manager-facing worker checkpoint/outcome events
3. optional timer events for review / retry

### 3) Deliberate at the manager layer

When a worker reaches a useful checkpoint, the manager decides one of:

1. steer the same worker,
2. send a follow-up to the same worker,
3. spawn a second worker,
4. retry with a changed configuration,
5. accept the result and finish,
6. fail or escalate.

### 4) Repeat until terminal

The loop continues until the manager itself decides the goal is complete,
failed, or cancelled.

The worker does not define global completion on its own.

## Important Semantic Change from Current Demiurge

Today the thin orchestrator treats `SessionLifecycle::WaitingInput` as a useful
terminal success signal.

`GoalManager` should change that meaning:

1. `WaitingInput` means the worker has paused at a review point,
2. the manager must inspect the worker checkpoint/output,
3. the manager then decides whether to continue, retry, or finish.

That is the key shift from "task launcher" to "manager agent".

## Contracts Needed

The existing `SessionLifecycleChanged` event is necessary but too thin for
manager orchestration.

GoalManager likely needs a new `aos.agent/*` contract such as
`SessionCheckpoint@1` with:

1. `session_id`
2. `run_id`
3. `checkpoint_kind`
4. `lifecycle_hint`
5. `new_message_refs`
6. `artifact_refs`
7. `summary`
8. `blocker_hint`

This gives the manager a structured observation channel and gives operator UX a
clean basis for diagnosis.

## Operator Story

`GoalManager` should be designed together with `aos task-diagnose`.

Operators need to answer:

1. which root goal is blocked,
2. which child worker owns the current blocker,
3. whether the manager is waiting on a worker or has simply not reviewed a
   worker result yet,
4. whether useful artifacts/workspace changes already exist.

That means `GoalManager` state must make child ownership and review state easy
to inspect.

## Implementation Phases

### Phase 1: preserve public task ingress, add manager semantics internally

1. keep `TaskSubmitted@1` externally,
2. replace "finish on `WaitingInput`" with "manager review pending",
3. add root goal status distinct from worker lifecycle.

### Phase 2: add child roster and checkpoint-driven review

1. record child session metadata in root state,
2. consume worker checkpoint/outcome events,
3. add explicit manager review queue.

### Phase 3: support multi-worker delegation

1. allow more than one child session per goal,
2. assign work items to specialized workers,
3. support follow-up and steering per worker.

### Phase 4: add retry, escalation, and long-running operation

1. timer-backed review loops,
2. retry with updated steering,
3. cancellation / recovery handling,
4. bounded stale-goal policies.

## Acceptance Criteria

1. One root goal cell can coordinate at least one worker session over multiple
   manager review cycles.
2. The manager, not the worker, decides global completion.
3. The manager can respond to worker output by steering the same worker or
   spawning another worker.
4. Goal state remains explicit, durable, and replay-safe.
5. No new high-level workflow DSL or static plan runtime is introduced.
6. Operator tooling can identify blocker ownership at the root and worker
   levels.

## Success Metric

This slice is successful if Demiurge becomes understandable as:

1. a top-level goal manager,
2. coordinating specialized worker agents,
3. iterating until the goal is truly done,
4. with deterministic state and auditable receipts underneath.

If the system still behaves mainly like a one-shot task bootstrapper, this
slice has not landed.

## Follow-Ups

1. Introduce a root goal status/event family (`GoalLifecycleChanged`,
   `GoalFinished`, `GoalCheckpoint`).
2. Add a smoke fixture that exercises multi-worker coordination.
3. Add a minimal UI that shows one goal, its workers, and the current blocker.
