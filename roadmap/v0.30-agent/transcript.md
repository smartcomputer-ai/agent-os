## Verdict

You are **not fundamentally on the wrong path**. AgentOS already has the right substrate for unattended agents: deterministic worlds, explicit effects, receipts, timers, stream frames, keyed workflows, CAS-backed state, and replay. That is a much better base than a typical “chat session + tools” SDK. The AgentOS overview explicitly frames the system as a deterministic, auditable substrate where I/O is effectful and receipt-backed, not hidden inside application logic , and the architecture already treats workflows as event-reactive state machines that continue from receipts .

But the **current aos-agent roadmap is still too centered on `RunRequested` as the main driver**. P4–P9 fix critical problems, especially the session/run split, context engine, traces, intervention, Fabric execution, and skills. However, by themselves they do not yet define the thing that makes OpenClaw/Hermes-style agents feel alive: a durable agent activity loop with schedules, heartbeats, event hooks, standing orders, work queues, memory maintenance, and post-run reflection.

So my answer is:

**Keep the roadmap, but insert a new P1 layer between P5/P6/P7: “Agent Activity Loop / Agenda / Trigger Runtime.”**
Also split “memory + learning loop” out of P9 and make the minimal version P1, not P2, if the target is long-running unattended agents.

---

## What OpenClaw and Hermes are getting right

The key insight from OpenClaw is not “chat plus tools.” It is that the agent is a **persistent daemon with multiple wake sources**. OpenClaw’s own docs describe a background system made of scheduled jobs, tasks, event hooks, standing instructions, heartbeat, and task flow; the decision guide distinguishes cron for exact/isolated work, heartbeat for context-aware periodic awareness, hooks for lifecycle/tool events, standing orders for persistent authority, and task flow for durable multi-step flows ([OpenClaw][1]). That is the pattern to copy at the primitive level.

OpenClaw’s heartbeat is especially instructive. It is a periodic main-session turn, defaulting to roughly every 30 minutes, with full session context, while cron jobs are precise scheduled tasks that create task records and can deliver output to a channel or webhook ([OpenClaw][1]). This distinction matters: a heartbeat is **ambient awareness**, while cron is **scheduled committed work**. AgentOS currently has `sys/timer.set@1` and deterministic effect mechanics, but aos-agent does not yet expose the agent-level concepts of heartbeat, cron-like schedule, wake reason, or recurrent commitment.

OpenClaw also has “standing orders”: durable instructions that grant the agent scoped ongoing authority, with triggers, approval gates, and escalation rules ([OpenClaw][2]). This is extremely relevant to AgentOS. Your P9 skill model talks about repo-local instructions, context contributions, and tool/profile suggestions, but standing orders are not quite skills. They are **commitments and authorities**. They should become a first-class implementation-layer concept, or at least a normalized context/policy contribution type.

Hermes pushes the same pattern further with an explicit “learning loop”: its docs describe agent-curated memory, autonomous skill creation, skill self-improvement, cross-session recall, scheduled automations, isolated subagents, portable skills, and a gateway that lets the agent live across CLI and messaging platforms ([Hermes Agent][3]). Hermes’ learning path also treats memory, cron, skills, delegation, hooks, batch processing, and sessions as central advanced features, not optional garnish ([GitHub][4]).

Hermes’ recent Kanban work is a strong signal too: it distinguishes `delegate_task` as an in-run function call from Kanban as a durable work queue where handoffs outlive a single API loop and remain visible to humans and other profiles ([GitHub][5]). That is exactly the distinction AgentOS needs: **tool calls and subagent calls are not enough; durable work items are a separate primitive.**

---

## How your current roadmap maps to that

### What is already right

P5 is the most important correction. Today `SessionLifecycle` is doing double duty as both session status and run lifecycle; starting a run clears conversation history; Demiurge still treats one task as one session/run story . P5’s planned model—durable session state with zero or more runs, transcript/history at session scope, active effects and tool batches at run scope—is exactly the right foundation .

P6 is also necessary. The current context model is just prompt refs plus conversation refs; there is no budget model, source metadata, selected/dropped input report, compaction recommendation, session-scoped context state, or skill contribution model . P6’s plan for an inspectable deterministic context engine, normalized source inputs, context plans, reports, and explicit compaction hooks is the right move .

P7 is necessary for unattended work because you cannot operate long-running agents without traceability and intervention. The roadmap already calls out that traces should include context plans, LLM turns, tool batches, effects, receipts, stream/progress frames, interventions, and outcomes, and that steer/follow-up/interrupt/cancel/pause/resume must become distinct semantics . That is the right core.

P4 is directionally right because OpenClaw/Hermes succeed by assembling different surfaces for different agents. Your current default registry accidentally makes every agent look like a local coding agent; P4’s explicit bundles and host target policy are needed to avoid hardening the SDK around one tool shape .

P8 is right because Fabric should remain an execution backend, not an agent primitive. The background notes already say canonical host effects can route through local or Fabric adapters, and Fabric supports sandbox session open, exec, session signal, filesystem operations, and exec progress frames without aos-agent depending on Fabric .

P9 is philosophically right: skills should live above the core session SDK and resolve into explicit context/tool/profile contributions rather than hidden prompt magic . But for a Hermes-like target, the **learning loop** part cannot wait too long.

---

## What is missing

### 1. A first-class `Agent`, not just `SessionWorkflow`

Right now the conceptual center is still “session.” For unattended agents, the center should be a durable **Agent Actor** or **Agent Process**.

A session is one conversation, channel, workspace, or interaction thread. An agent is the thing that owns:

* identity/persona/profile,
* default model/tool/context policy,
* memory namespace,
* standing orders,
* schedules and heartbeats,
* resource attachments,
* active sessions,
* current agenda/work queue,
* bounded run history,
* authority/approval policy.

P5 gets part of this by making sessions durable, but I would not let “session” become the top-level agent primitive. The OpenClaw README itself frames the Gateway as a control plane and the assistant as the product, with multi-channel routing, sessions, tools, events, and skills under that umbrella ([GitHub][6]). AgentOS should mirror that separation: **Agent = durable actor; Session = one interaction scope; Run = one bounded processing attempt.**

### 2. Wake reasons and triggers

Every run should have a cause. Not just `RunRequested`.

Add something like:

```rust
enum RunCause {
    UserInput { input_ref, channel_ref, sender_ref },
    Heartbeat { schedule_id, due_at_ns, missed_count },
    ScheduledJob { job_id, due_at_ns, payload_ref },
    ExternalEvent { source, event_ref },
    WorkItemReady { work_item_id },
    ReceiptContinuation { intent_hash },
    OperatorResume { reason_ref },
    MemoryMaintenance { policy_id },
    Reflection { completed_run_id },
}
```

This changes the mental model. A run is not “the user said something.” A run is “the agent woke up because something happened.”

AgentOS already has the right low-level mechanics: workflows request nondeterministic work through typed effects, the owner records open work durably, executors return stream frames or receipts, and continuations re-enter deterministically . But aos-agent needs to lift that into agent semantics: triggers, wake reasons, and scheduling policy.

### 3. Heartbeat and cron as separate primitives

OpenClaw’s docs make this distinction very clearly: cron is exact scheduled work with task records; heartbeat is a periodic main-session turn with full context and no task record ([OpenClaw][1]).

AgentOS should model both:

```rust
struct AgentSchedule {
    schedule_id: ScheduleId,
    kind: ScheduleKind, // At, Every, CronLike, EventHook
    target: ScheduleTarget, // agent, session, work queue, workflow
    payload_ref: Option<Hash>,
    create_work_item: bool,
    delivery_policy: DeliveryPolicy,
    budget: RunBudget,
}

struct HeartbeatPolicy {
    enabled: bool,
    cadence: Duration,
    active_window: Option<LocalTimeWindow>,
    session_scope: HeartbeatSessionScope,
    context_mode: HeartbeatContextMode, // full, light, isolated
    skip_when_no_due_tasks: bool,
}
```

Do **not** implement heartbeat as “some prompt file the model reads.” Implement it as deterministic schedule state plus timer effects, with optional normalized context inputs. `HEARTBEAT.md` can be one source, not the primitive.

### 4. Durable agenda / task ledger / commitments

This is the biggest missing piece.

P5 gives sessions multiple runs. P7 gives traces. But unattended agents need something between them: durable commitments.

OpenClaw’s task docs say background tasks track detached work, with lifecycle states like queued/running/terminal, and they serve as the activity ledger for ACP runs, subagents, cron jobs, and CLI operations ([OpenClaw][7]). Hermes’ Kanban RFC makes the deeper point: durable work queues are for handoffs that must outlive one API loop, survive restarts, be visible to humans, and potentially be picked up by a different profile ([GitHub][5]).

AgentOS should add a P1 item:

**P6.5: Agent Agenda and Work Items**

Core contract sketch:

```rust
struct WorkItem {
    id: WorkItemId,
    owner_agent_id: AgentId,
    session_id: Option<SessionId>,
    status: WorkItemStatus, // queued, ready, running, blocked, done, failed, cancelled, archived
    cause: WorkItemCause,
    title: String,
    description_ref: Hash,
    dependencies: Vec<WorkItemId>,
    assignee: Option<AgentProfileId>,
    next_wake_at_ns: Option<u64>,
    deadline_ns: Option<u64>,
    retry_policy: Option<RetryPolicy>,
    approval_policy: Option<ApprovalPolicyRef>,
    trace_refs: Vec<Hash>,
    result_ref: Option<Hash>,
}
```

This should not be bolted onto `RunState`. A run attempts work; a work item persists across runs.

### 5. Memory as state transformation, not just context selection

P6 gives you context planning. P9 gives you skills. But long-running agents also need **memory writes**, not just memory reads.

Hermes’ differentiator is its closed learning loop: it creates skills from experience, improves them during use, persists knowledge, and searches past conversations across sessions ([GitHub][8]). Recent research points in the same direction: Memento-Skills treats structured markdown skills as persistent evolving memory, with read/write reflective learning; SkillWeaver reports substantial gains from agents synthesizing reusable skills/APIs from experience ([arXiv][9]).

AgentOS should add an explicit **Memory/Reflection Engine**:

```rust
enum MemoryKind {
    Episodic,     // what happened
    Semantic,     // facts learned
    Procedural,   // how to do something
    Preference,   // user/org preferences
    Commitment,   // things promised or pending
    PolicyNote,   // inferred constraints, requires review
}

struct MemoryWriteProposal {
    run_id: RunId,
    kind: MemoryKind,
    content_ref: Hash,
    provenance: Vec<TraceEntryId>,
    confidence: Decimal,
    expires_at_ns: Option<u64>,
    requires_approval: bool,
}
```

This matters because memory mutation is dangerous. AgentOS has an advantage here: you can make memory writes proposal/receipt-backed, auditable, reversible, and tied to traces.

### 6. Post-run reflection as an automatic run type

After a completed run, the agent should have a chance to:

* summarize what happened,
* update work item status,
* extract durable facts,
* propose memory writes,
* propose skill updates,
* schedule follow-up work,
* detect unresolved commitments,
* report completion or escalate.

This should be a separate bounded run, not hidden inside the final assistant message.

Suggested lifecycle:

```text
event/heartbeat/user input
  → active run
  → outcome
  → reflection run
  → memory proposals / work item updates / skill proposals / next wake
```

This is how you get “gets better over time” without making the main run state messy.

### 7. Approval and authority boundaries

OpenClaw’s docs emphasize that standing orders need scope, triggers, approval gates, and escalation rules ([OpenClaw][2]). OpenClaw’s own README also warns that host tools may have broad access in the main session and recommends sandboxing non-main sessions ([GitHub][6]). The OpenClaw security paper frames prompt injection, tool chains, context amnesia, and supply-chain contamination as systemic risks in autonomous agents with OS/tool access ([arXiv][10]).

AgentOS has the right structural effect controls, but the public AIR surface currently has no caps/policy language; stronger hosted policy is explicitly layered outside public AIR . That is fine for AIR v2, but the agent SDK needs a user-facing primitive for:

```rust
enum ActionDisposition {
    Allowed,
    RequiresApproval { reason, approver_scope },
    Denied { reason },
    Escalate { reason, channel },
}
```

This should sit above effect admission. Structural effect admission says “may this workflow emit host.exec?” Agent authority policy says “may this agent, under this standing order, run this command now without asking?”

### 8. Event hooks

OpenClaw treats hooks as scripts triggered by lifecycle events, compaction, gateway startup, message flow, and tool calls ([OpenClaw][1]). AgentOS already has the deeper mechanism—event routing and workflows—but aos-agent should expose a clean agent-level hook vocabulary:

```rust
enum AgentHook {
    OnAgentStarted,
    OnSessionOpened,
    OnRunStarted,
    OnRunCompleted,
    OnRunFailed,
    OnToolCallObserved,
    OnToolReceipt,
    OnContextCompacted,
    OnMemoryWritten,
    OnWorkItemBlocked,
    OnHeartbeatSkipped,
}
```

Not every hook should call an LLM. Many should run deterministic maintenance or schedule a run.

### 9. Context plans need agenda and standing-order inputs

P6’s context request should include more than prompts, transcript, and run input. For unattended agents, the context engine must see:

* wake reason,
* standing orders relevant to that wake reason,
* due work items,
* recent unresolved commitments,
* last heartbeat result,
* active schedules,
* pending approvals,
* memory summaries,
* selected skills,
* policy/authority context.

So P6 should add normalized context input types like:

```rust
ContextInputKind::WakeReason
ContextInputKind::StandingOrder
ContextInputKind::WorkItem
ContextInputKind::Schedule
ContextInputKind::PendingApproval
ContextInputKind::MemoryFact
ContextInputKind::SkillContribution
ContextInputKind::TraceSummary
```

Otherwise the context engine will still mostly look like a smarter chat prompt builder.

### 10. Cost-aware unattended execution

OpenClaw’s heartbeat docs explicitly include options like lightweight heartbeat context, isolated sessions, skipping when no due tasks exist, and preserving task intervals to avoid wasted calls ([OpenClaw][11]). Hermes’ Kanban RFC also notes that cron-driven dispatch burned LLM tokens per tick and moved toward a long-lived daemon with first-class runs and structured handoff ([GitHub][5]).

AgentOS should make this a core acceptance criterion for unattended agents:

* cheap deterministic precheck before LLM wake,
* due-only heartbeat sections,
* budget per schedule/work item,
* max autonomous spend per agent/day,
* quiet hours,
* jitter/load smoothing,
* backoff after repeated no-op runs,
* “do not wake model if deterministic state says nothing is due.”

This is not optimization; it is survival for always-on agents.

---

## Roadmap change I would make

I would not throw away P4–P9. I would restructure them like this:

### P4: Tool bundles and execution surfaces

Keep as planned. This prevents the accidental local coding-agent default.

### P5: Agent/session/run model

Expand P5 from “Session and Run Model” to **“Agent, Session, and Run Model.”**

Add:

* `AgentId`,
* `AgentProfile`,
* `AgentStatus`,
* `SessionStatus`,
* `RunLifecycle`,
* `RunCause`,
* `WakeReason`,
* `RunBudget`,
* `AgentResourceAttachment`,
* `AgentMemoryNamespace`.

The key invariant: **run cause is not necessarily user input.**

### P5.5: Activity loop, schedules, and triggers

New P1.

Add:

* heartbeat policy,
* schedule policy,
* event hook registry,
* timer integration,
* wake queue,
* skipped wake trace entries,
* quiet hours,
* deterministic due checks,
* schedule persistence.

Acceptance test: an agent with no user input wakes on a timer, checks due work, optionally runs, records a trace, and schedules the next wake.

### P6: Context engine

Keep P6, but add wake/agenda/standing-order/memory inputs. Context is not just conversation selection.

### P6.5: Agenda and work items

New P1.

Add durable work items, dependencies, retry/blocked states, assignment, claim semantics, and status transitions. This can live above core aos-agent if needed, but the SDK should have normalized contracts because everything else depends on it.

Acceptance test: a work item survives restart, blocks on missing info, wakes later, resumes, and completes across multiple runs.

### P6.7: Memory and reflection

New P1 or P1.5.

Add post-run reflection, memory write proposals, memory observation events, provenance, and approval gates for sensitive memory. Keep embedding/search effects explicit. Do not hide summarization or vector updates in the context engine.

### P7: Run traces and intervention

Keep, but add trace entries for:

* wake received,
* wake skipped,
* schedule evaluated,
* standing order matched,
* work item claimed,
* memory proposal created/applied/rejected,
* approval requested/granted/denied,
* next wake scheduled.

### P8: Fabric-backed hosted execution

Keep. This becomes more valuable once runs can be triggered by schedules/work items, not just user prompts.

### P9: Skills

Split into two levels:

1. **P9a: Static skills as explicit contributions** — current plan.
2. **P9b: Skill learning loop** — generated/updated skills from reflection, with review/provenance.

If Hermes-like improvement is a target, P9b should not be treated as marketplace future-work. It is core to “agent gets better over time.”

---

## Concrete redesign target

I would phrase the aos-agent SDK target like this:

> `aos-agent` is a deterministic agent actor runtime. An agent owns durable identity, sessions, memory namespaces, standing orders, schedules, agenda, resources, and policy. A run is a bounded attempt to process a wake reason. User messages are one wake source among timers, receipts, work items, external events, and operator interventions.

That framing prevents “session chat agent” drift.

The minimal state split becomes:

```text
AgentState
  AgentProfile
  AgentStatus
  DefaultPolicies
  ResourceAttachments
  ScheduleState
  AgendaSummary
  MemoryState
  Sessions[]
  CurrentRuns[]
  BoundedHistory

SessionState
  SessionId
  Channel/peer/workspace binding
  Durable transcript refs
  Context state
  Last activity
  Session status

RunState
  RunId
  RunCause / WakeReason
  RunLifecycle
  ContextPlan
  Active effects
  Active tool batch
  Trace
  Outcome
  Reflection status

WorkItemState
  WorkItemId
  Status
  Dependencies
  Assignee/profile
  Next wake/deadline
  Attempts
  Result/blocked reason
```

This is not a radical rewrite of AgentOS. It is a reframing of the agent SDK boundary.

---

## The sharpest risk in the current roadmap

The danger is that P5–P7 land and the system feels clean, but the product still behaves like:

```text
user input → run → tool loop → waiting input
```

That is still a chat agent.

The target should be:

```text
event/timer/user/receipt/work-item → wake → context plan → run → trace → reflection → memory/work updates → next wake
```

That is an unattended agent.

---

## Evaluation changes I would add immediately

Your background notes already say the live eval harness is model-variable and that a deterministic scripted LLM would let tests verify reducer behavior, context planning, tool batching, traces, and replay . I would make that a hard requirement for the new roadmap.

Add deterministic tests for:

1. **heartbeat without user input**: timer fires, run starts, no-op result records `HEARTBEAT_OK`-equivalent trace, next timer scheduled.
2. **cron-like scheduled job**: exact due time creates a work item, run executes, result delivered, task history retained.
3. **work item across runs**: first run blocks, user provides missing info later, second run resumes same item.
4. **post-run reflection**: completed run proposes memory write and next work item.
5. **approval gate**: standing order allows research but requires approval before external send.
6. **restart replay**: schedule/work item/run state replays byte-identically.
7. **cost guard**: heartbeat skips model call when no due tasks exist.
8. **Fabric interruption**: long exec emits progress frames, interrupt signals host session, final state only changes after admitted receipt/rejection.
9. **skill learning**: successful workflow produces a skill proposal with trace provenance.
10. **memory contamination defense**: untrusted tool output cannot silently become durable policy/memory without the right disposition.

---

## Final recommendation

Do **not** radically redesign AgentOS. The deterministic world/effect/receipt architecture is the right foundation and is arguably better suited to unattended agents than OpenClaw/Hermes’ more conventional daemon stacks.

But **do** radically reframe `aos-agent` away from “SessionWorkflow as reusable chat loop” and toward “Agent Actor Runtime.” P4–P9 are necessary cleanup, but the missing P1 primitives are:

1. agent identity/profile above sessions,
2. wake reasons and trigger-driven runs,
3. heartbeat and cron/schedule contracts,
4. durable agenda/work items,
5. standing orders and authority boundaries,
6. memory write/reflection loop,
7. event hooks,
8. cost-aware unattended execution,
9. approval gates,
10. deterministic long-run evals.

With those additions, you are building something materially stronger than another session-based chat agent: a replayable, auditable, long-running agent substrate where chat is just one input channel.

[1]: https://docs.openclaw.ai/automation "Automation & tasks - OpenClaw"
[2]: https://docs.openclaw.ai/automation/standing-orders "Standing Orders - OpenClaw"
[3]: https://hermes-agent.nousresearch.com/docs/ "Hermes Agent Documentation | Hermes Agent"
[4]: https://github.com/NousResearch/hermes-agent/blob/main/website/docs/getting-started/learning-path.md "hermes-agent/website/docs/getting-started/learning-path.md at main · NousResearch/hermes-agent · GitHub"
[5]: https://github.com/NousResearch/hermes-agent/issues/16102 "RFC: review the Kanban — multi-profile collaboration board (PR #16100) · Issue #16102 · NousResearch/hermes-agent · GitHub"
[6]: https://github.com/openclaw/openclaw "GitHub - openclaw/openclaw: Your own personal AI assistant. Any OS. Any Platform. The lobster way.  · GitHub"
[7]: https://docs.openclaw.ai/automation/tasks "Background Tasks - OpenClaw"
[8]: https://github.com/nousresearch/hermes-agent "GitHub - NousResearch/hermes-agent: The agent that grows with you · GitHub"
[9]: https://arxiv.org/abs/2603.18743 "[2603.18743] Memento-Skills: Let Agents Design Agents"
[10]: https://arxiv.org/html/2603.12644v1 "Uncovering Security Threats and Architecting Defenses in Autonomous Agents: A Case Study of OpenClaw"
[11]: https://docs.openclaw.ai/gateway/heartbeat "Heartbeat - OpenClaw"


---

> Keep in mind that AOS agent is just one more flow inside agentOS. It's by far the most sophisticated one right now, I sense, and the changes you want to make are to the existing flow system. I think you need to look at the proposal from the perspective of agentOS itself. So what it can do as an event source, depend log, deterministic, reflow system with common effects. So a few points. The common effect system, we actually had until recently, but we just ripped it out because we want to rebuild it from scratch. So in the current agent perspective, the whole policy and capability stuff I want to leave away because we do want to focus on that and spend a lot of time on that, but not right now. Right now in agentOS, we also only have timers, but heartbeats and or schedules are planned and can be implemented as part of this to make to make room for different wake conditions, kind of Chrome-like wake conditions to not be too much work. And in terms of product direction, The goal is to build a software factory agent or a dark factory of various agents coordinating to write and test software, and they will drive CLI level coding agents as well. But the agents you're building here, although they can use, um, sandboxed sessions, their goal is to be mostly um Mostly the coordinator, the kind of workflow engine basically on top of fully agentic and agent-based. So I want you to look at the suggestions you've made now, which are excellent, from an AgentOS perspective and what I added. What is really needed and what is really essential here? For example, what should live in the workspace system of AgentOS, which is kind of its own basic file system, but native, and each item in the workspace is part of the content addressable storage and versioned and so on. And yeah, basically, your whole proposal for this to be added while extremely valuable should be now really considered from the perspective of the foundations of AgentOS, which you have also access to here as all the specifications and schemas and and build things that are part of AgentOS, so you can look at these what how it looks and how it works. Also while I appreciate all the ideas that you've extracted from open clone or MISAgent, think broadly and deeply what is the essence that we need to implement, not every possible thing to make it equivalent. We can work towards that, but that's gonna take time. But part of this current roadmap, which should be implemented in like one major push to get the first full version of agents working, how should, what should be part of that? And what could be left for later?

---

You’re right to push this back down into AgentOS. My previous “Agent Actor Runtime” framing is useful as a **product mental model**, but it would be wrong if it led us to add a separate agent runtime above or beside the AgentOS workflow system.

The better framing is:

> **AgentOS already is the long-running agent substrate. `aos-agent` should remain one sophisticated workflow family inside it, not become a second runtime.**

The thing we need is not “add agent primitives everywhere.” It is: make sure the AgentOS workflow/effect/workspace substrate has enough **wake, work, context, trace, and artifact structure** that `aos-agent`, Demiurge, and future factory workflows can compose into a dark software factory.

## Revised thesis

Do **not** radically redesign AgentOS.

Do **not** make `Agent` a kernel primitive.

Do **not** make `aos-agent` the top-level owner of all scheduling, memory, work queues, and factory state.

Instead:

1. Keep the kernel/AIR/effect model minimal.
2. Keep `aos-agent` focused on LLM session/run orchestration.
3. Add a small number of reusable workflow-level structures around it:

   * wake causes,
   * timer-driven schedules,
   * durable work items,
   * workspace-backed artifacts,
   * context sources,
   * run traces.
4. Let the software factory be a composition of workflows, not a monolithic agent session.

That aligns much better with AgentOS itself: worlds are deterministic event logs; workflows are state machines; effects are explicit and receipt-backed; workspaces are versioned CAS-backed artifact trees; routing and keyed workflows give us long-running entities already  .

---

# The essential distinction

The system you want should not look like this:

```text
user prompt
  → aos-agent session
  → model loop
  → tools
  → final answer
```

It should look like this:

```text
task/event/timer/receipt/workspace change
  → factory workflow
  → work item state
  → coordinator session run if needed
  → worker invocation / CLI agent / tests / patch
  → receipts and traces
  → workspace commit / work item update
  → next scheduled wake or downstream event
```

In that model, `aos-agent` is important, but it is not the factory. It is the **LLM reasoning loop** used by coordinators, reviewers, planners, or supervisors.

The factory itself should mostly be normal AgentOS workflows.

---

# What AgentOS already gives us

AgentOS already has most of the hard substrate:

| Need                          | Existing AgentOS primitive                   |
| ----------------------------- | -------------------------------------------- |
| Long-running durable state    | Workflow state and keyed workflow cells      |
| Event-driven execution        | Domain events and routing subscriptions      |
| Async external work           | Effects, open work, receipts, stream frames  |
| Replayability                 | Journal, snapshots, canonical CBOR, receipts |
| Timers                        | `sys/timer.set@1`                            |
| Large artifacts               | CAS blobs                                    |
| Versioned code/artifact trees | `sys/Workspace@1`                            |
| Sandboxed external execution  | Canonical host/session effects               |
| Introspection                 | `sys/introspect.*@1`                         |
| Self-modification             | AIR patches, shadow, approve, apply          |

The workflow spec already says workflows own business state, transition logic, retry, compensation, and effect requests, while the kernel/effect runtime owns admission, open-work tracking, and continuation delivery . That is exactly the right foundation for a dark factory.

So the first rule is:

> If something is a long-running business process, it should probably be a workflow, not a hidden feature inside `aos-agent`.

---

# What should live where

## 1. Kernel / AIR / core AgentOS

Keep this small.

For the current push, I would **not** add an `Agent` primitive, policy/capability language, standing-order enforcement, memory engine, or global cron DSL to AIR.

The public AIR/effect model is intentionally structural: workflows declare emitted effects, params are schema-normalized, open work is recorded, and receipts re-enter deterministically  . That is enough for now.

Core AgentOS should only need:

* stable effect/receipt/stream-frame mechanics,
* `sys/timer.set@1`,
* workspace effects,
* host effects,
* LLM effects,
* introspection effects,
* keyed workflow/cell support,
* replay and snapshots.

The common effect system can be rebuilt, but I would keep the first rebuilt version boring: typed params, typed receipts, stream frames, issuer/correlation refs, adapter routing, and no policy/capability system yet.

## 2. `aos-agent`

`aos-agent` should own the LLM session/run loop.

It should include:

* session/run split,
* run lifecycle,
* context planning,
* LLM turn orchestration,
* tool-call planning,
* tool-bundle execution seams,
* run traces,
* intervention semantics,
* explicit run cause / wake reason.

It should **not** own the whole factory queue, global scheduling, workspace memory, cross-agent assignment, or product-level task lifecycle.

Today `aos-agent` is already close to AOS primitives: sessions are keyed workflow state, LLM calls are `sys/llm.generate@1`, tools map to effects/domain events, receipts and stream frames re-enter the workflow, and large payloads live behind blob refs . The roadmap should preserve that shape.

## 3. Factory / Demiurge / orchestration workflows

This is where the software factory should live.

Add or evolve workflows such as:

```text
FactoryProjectWorkflow
WorkItemWorkflow
WorkerInvocationWorkflow
ReviewWorkflow
TestWorkflow
CoordinatorSessionWorkflow wrapper
```

Demiurge should probably stop being “one task maps to one session/run” and become the first real factory orchestration layer.

The background doc says Demiurge currently submits a task, opens a host session, sends `SessionIngress`, and treats `WaitingInput` as task completion . That is fine for the old one-shot model, but the factory needs a separate durable work ledger.

## 4. Workspace system

The workspace should be the factory’s **artifact substrate**, not its only state machine.

Use workspaces for:

* source code trees,
* generated patches,
* test logs,
* build artifacts,
* run summaries,
* task briefs,
* design docs,
* review notes,
* repo-local instructions,
* reusable playbooks,
* context packs,
* skill-like files,
* versioned memory documents.

Do **not** use workspace files as the only source of truth for active lifecycle state such as:

* whether a work item is queued/running/blocked/done,
* which effect is in flight,
* which timer is pending,
* which run is active,
* whether a worker has claimed a task.

Those should be workflow state. Workspace documents can mirror or explain them, but workflow state is what routes events, resumes receipts, and drives deterministic transitions.

This matches the workspace design: workspaces are named, versioned trees stored through CAS and manipulated by deterministic workspace effects; `sys/Workspace@1` records committed roots and metadata  .

---

# What is truly essential for the first full agent/factory version

I would revise the current roadmap into **core P1 essentials** and **deferable enrichments**.

## Essential 1: P4 tool bundles and execution surfaces

Keep P4 as P1.

This is still foundational. The current default registry makes every agent look like a broad local coding agent. P4 correctly moves toward explicit inspect, host, workspace, and sandbox-ready bundles .

But from the AgentOS perspective, the key reason is not only “different agents need different tools.” The deeper reason is:

> Tool bundles are the bridge from LLM tool calls to common AgentOS effects.

So P4 should optimize for canonical effect mapping:

```text
LLM tool call
  → tool mapper
  → canonical effect intent or domain event
  → receipt
  → tool result
  → trace
```

For the factory, I would make these bundle categories first-class:

1. `inspect` bundle,
2. `workspace` bundle,
3. `host-local` bundle,
4. `host-sandbox-ready` bundle,
5. `factory-control` bundle, probably domain-event tools such as “create work item,” “update work item,” “request worker run.”

The last one is important: the coordinator should not always directly mutate files or run shell commands. Often it should emit factory events.

## Essential 2: P5 session/run split, plus `RunCause`

Keep P5 as P1, but add one small required change: every run needs a cause.

P5 already separates durable session state from per-run execution state, which is mandatory because the current lifecycle mixes session status and run lifecycle, and run start clears conversation history .

But for unattended factory work, `RunRequested { input_ref }` is too chat-shaped.

Add something like:

```rust
pub enum RunCause {
    UserInput { input_ref: Hash },
    WorkItemReady { work_item_id: WorkItemId, input_ref: Option<Hash> },
    TimerFired { timer_key: String },
    ScheduledWake { schedule_id: ScheduleId },
    ReceiptContinuation { intent_hash: Hash },
    OperatorSteer { input_ref: Hash },
    FactoryEvent { event_ref: Hash },
}
```

This does not need to be a kernel concept. It can be an `aos-agent` contract.

But it must be in the run model, because context, traces, scheduling, and work-item updates all need to know why the model woke up.

## Essential 3: Timer-chain wake support, not a full scheduler yet

You do not need a full cron system in the first push.

You do need a minimal, deterministic way for workflows to wake themselves without user input.

AgentOS already has `sys/timer.set@1`, with `deliver_at_ns` and an optional key . That is enough for a first heartbeat/schedule implementation.

The minimal pattern is:

```text
workflow stores schedule state
  → emits sys/timer.set@1
  → timer receipt returns
  → workflow handles receipt as wake
  → workflow decides whether to run LLM / update work / reschedule
  → emits next sys/timer.set@1
```

This keeps scheduling inside deterministic workflow state and avoids an external cron daemon.

For the first version, I would support:

* one-shot `At`,
* recurring `Every`,
* maybe `Backoff`,
* maybe `RunSoon`.

I would defer:

* full cron syntax,
* timezone-heavy schedules,
* calendar semantics,
* global scheduler service,
* schedule marketplace,
* complex missed-run policies.

A heartbeat should also not automatically mean “call the LLM.” It should mean “wake the workflow.” The workflow can perform a cheap deterministic precheck and skip the model if nothing is due.

## Essential 4: Durable work items / agenda, but outside `aos-agent`

This is the most important addition for the software factory.

Do not bury the agenda inside `SessionWorkflow`.

Add a separate workflow family, probably keyed by work item id:

```rust
pub struct WorkItemState {
    pub id: WorkItemId,
    pub project_id: ProjectId,
    pub status: WorkItemStatus,
    pub kind: WorkItemKind,
    pub parent: Option<WorkItemId>,
    pub dependencies: Vec<WorkItemId>,
    pub assigned_profile: Option<WorkerProfileId>,
    pub input_ref: Hash,
    pub result_ref: Option<Hash>,
    pub workspace_ref: Option<WorkspaceRef>,
    pub current_run_id: Option<RunId>,
    pub attempts: Vec<AttemptSummary>,
    pub next_wake_at_ns: Option<u64>,
}
```

Lifecycle can stay simple:

```text
queued
ready
running
blocked
done
failed
cancelled
archived
```

This should be a normal AgentOS workflow using keyed cells. Keyed workflows are already designed for many independent durable instances with isolated state, mailboxes, receipt routing, and per-cell scheduling .

For a dark software factory, this is more important than “memory.” The work ledger is the active memory of what needs doing.

## Essential 5: P6 context engine, but with factory inputs

Keep P6 as P1.

The current context model is just prompt refs and conversation refs; P6 correctly introduces deterministic context requests, context plans, reports, selected/dropped inputs, compaction hooks, and source-agnostic inputs .

For the factory version, make sure the context engine can consume these normalized input kinds:

```text
RunCause
WorkItemBrief
ProjectStateSummary
WorkspaceRoot
WorkspaceDiff
RepoInstructionFile
FactoryPlaybook
RecentRunSummary
RelevantTraceSummary
Tool/BundleProfile
PendingDecision
```

This still does not require a full skill system.

The immediate need is not “skills.” The immediate need is that the context engine can assemble a run from workspace artifacts, work-item state, previous summaries, and repo instructions without becoming hidden I/O.

## Essential 6: P7 traces and intervention

Keep P7 as P1.

A dark factory without traces is not operable.

P7 already asks for deterministic traces covering context plans, LLM turns, tool batches, effects, receipts, stream frames, interventions, and outcomes .

For the factory, trace entries should also include:

```text
run_cause
work_item_id
project_id
workspace_root_before
workspace_root_after
worker_invocation_id
host_session_id
patch_ref
test_result_ref
next_wake_at_ns
```

The trace should not become a giant blob in workflow state. Keep bounded summaries in state and large payloads in CAS/workspace refs.

Intervention also matters because CLI-level coding agents and host execs can run long. The P7 stance is right: interrupt/cancel must be effect-aware and cannot pretend external work stopped until a receipt or rejection is admitted .

## Essential 7: A worker invocation workflow for CLI agents

Because your product direction is a software factory that drives CLI-level coding agents, I would add one more workflow concept now:

```text
WorkerInvocationWorkflow
```

This can be thin at first.

Its job:

1. receive `WorkerRunRequested`,
2. open or use a host/sandbox session,
3. run the CLI coding agent or test command via canonical host effects,
4. collect stream frames,
5. store logs/results as blob/workspace refs,
6. emit `WorkerRunCompleted` / `WorkerRunFailed`,
7. update the work item.

This keeps the coordinator LLM from directly owning every host exec detail.

You can still expose host tools to some coordinator sessions, but the factory should have a structured way to say:

```text
delegate this work to a worker
```

rather than always:

```text
run this shell command yourself
```

The host effect catalog already has session open, exec, session signal, and filesystem effects, with local and sandbox target schemas already represented in built-in host schemas  .

---

# What I would change in the roadmap

## P4 stays P1

No change, except I would explicitly add a **factory-control/domain-event bundle**.

P4 should not only separate host/workspace/inspect. It should also let an LLM session emit structured factory events:

```text
create_work_item
mark_blocked
request_worker_run
request_review
attach_artifact
schedule_followup
```

These should map to domain events, not host effects.

## P5 stays P1, with `RunCause`

P5 should not add a global `Agent` abstraction.

Instead, it should add:

```text
SessionStatus
RunLifecycle
RunState
RunCause
RunOutcome
```

That is enough.

The earlier idea of `AgentState` should become either:

* a factory/project workflow concept, or
* a session/profile config concept,

not a core `aos-agent` requirement.

## Add P5.5: Wake and timer-chain support

Small P1.

Scope:

* `RunCause::TimerFired` / `ScheduledWake`,
* session/workflow timer state,
* recurring timer helper,
* skipped-wake trace entry,
* deterministic no-op wake handling,
* next timer rescheduling.

Do not build full cron yet.

## Add P5.6 or P6.5: Work item / agenda workflow

This is P1 for the software factory, but it should probably be outside `aos-agent`.

Call it something like:

```text
aos-factory / WorkItemWorkflow
```

or make Demiurge evolve into this.

Acceptance test:

```text
TaskSubmitted
  → WorkItemCreated
  → WorkItemReady
  → coordinator run starts with RunCause::WorkItemReady
  → worker invocation runs
  → workspace artifact produced
  → work item completes
  → replay reproduces state
```

## P6 stays P1, with workspace/factory context sources

P6 should explicitly support:

* workspace-root context inputs,
* repo instruction files,
* work item briefs,
* run summaries,
* trace summaries,
* factory playbooks.

This gives you most of the practical value of “memory” without building a full memory subsystem yet.

## P7 stays P1, with factory correlation

P7 should add trace correlation fields for:

* work item id,
* project id,
* workspace roots,
* worker invocation id,
* run cause,
* schedule id.

## P8 should be split

P8 has two parts:

1. **Host target policy shape** — P1.
2. **Full Fabric-backed execution fixture/product proof** — P2.

For the first full agent version, you need local vs sandbox target shape to avoid baking local assumptions into the SDK. But you do not need full Fabric product integration before the rest of the agent model works.

The P8 doc already has the right stance: Fabric is an execution backend below canonical host effects, not an `aos-agent` dependency .

## P9 should be reduced for now

Full skills can wait.

But a P9-lite should be folded into P6:

```text
repo-local instruction files
workspace context packs
factory playbooks
```

These should resolve into explicit context inputs.

The full P9 skill system—activation metadata, tool/profile suggestions, marketplace-ish sources, versioning, learning loop—can wait. P9 itself already says skills should live above the core session SDK and feed context/tools explicitly rather than become hidden prompt magic .

---

# What should be left for later

I would explicitly defer these:

1. **Policy/capability/authority system**
   Keep structural effect admission only. You already said this is not the current focus, and the AIR docs intentionally keep public caps/policies out of v2 .

2. **Full standing orders**
   Store factory playbooks and instructions as context files for now. Do not build enforcement semantics yet.

3. **Full semantic memory**
   Use event log + traces + workspace summaries + work item state first. Add vector/semantic memory later.

4. **Autonomous skill creation / self-improving skills**
   Valuable later, but not necessary for the first working software factory.

5. **Global cron service**
   Timer chains are enough for first version.

6. **Complex schedule semantics**
   Avoid timezone/DST-heavy cron until the deterministic model is clear.

7. **Multi-world agent mesh**
   Keep one world / one factory first. Portals and cross-world coordination can wait unless immediately needed.

8. **Sophisticated subagent hierarchy**
   Use work items and worker invocation workflows first.

9. **Full Fabric hosted product flow**
   Keep host target policy sandbox-ready, but do not block the core roadmap on Fabric.

10. **UI-heavy operator surfaces**
    P7 traces should support later UI, but text/CLI inspection is enough for now.

---

# The workspace answer in more detail

For this factory direction, I would define workspace conventions early.

Example:

```text
workspace: project
  /src/...
  /tests/...
  /AGENTS.md
  /FACTORY.md
  /plans/
  /patches/
  /runs/
  /reviews/
  /artifacts/
  /summaries/
```

Possible layout:

```text
/runs/<run_id>/summary.md
/runs/<run_id>/trace.json.ref
/runs/<run_id>/stdout.log
/runs/<run_id>/stderr.log
/runs/<run_id>/tool-results/
/work-items/<work_item_id>/brief.md
/work-items/<work_item_id>/result.md
/work-items/<work_item_id>/attempts/<attempt_id>.md
/patches/<work_item_id>/<attempt_id>.patch
/reviews/<review_id>.md
/factory/playbooks/*.md
/factory/context/*.md
```

The actual active status remains in workflow state:

```text
WorkItemState.status = running
WorkItemState.current_run_id = ...
WorkItemState.result_ref = ...
WorkItemState.workspace_root_after = ...
```

Workspace gives you durable, versioned, human-readable artifacts. Workflow state gives you routing and lifecycle.

That split is essential.

If you put too much into workspace files, the factory becomes a document store with no deterministic process semantics.

If you put too much into workflow state, the factory becomes opaque and hard for humans/agents to inspect.

The right model is:

```text
workflow state = authority
workspace/CAS = artifacts and history
trace = causal explanation
journal = replay source of truth
```

---

# The minimum “not just chat” acceptance tests

For the first full version, I would require these tests before calling it a real agent/factory foundation:

1. **Timer wake without user input**
   A workflow schedules a timer, receives the timer receipt, records a wake, and either starts a run or deterministically skips.

2. **Work item drives agent run**
   A `WorkItemReady` event starts an `aos-agent` run with `RunCause::WorkItemReady`, not `UserInput`.

3. **Multi-run durable session**
   One session handles multiple runs without losing transcript/context state.

4. **Workspace-backed context**
   A run context plan includes a work item brief, repo instruction file, previous summary, and workspace root/diff.

5. **CLI worker invocation**
   A factory workflow invokes a CLI-level coding agent or test command through host effects, records stream frames, stores logs, and emits a structured completion event.

6. **Patch/test artifact path**
   Worker output produces a patch or workspace commit, test result artifact, and work item update.

7. **Trace correlation**
   Operator can inspect one work item and see: triggering event, coordinator run, worker invocation, host effects, receipts, workspace changes, and final status.

8. **Interrupt/cancel**
   A long host exec can be interrupted through the P7 model without pretending the external process stopped before a receipt confirms it.

9. **Replay equivalence**
   Replaying the journal and receipts reproduces the same work item states, run states, workspace refs, and traces.

10. **No model call on empty heartbeat**
    A scheduled wake can skip the LLM when no work is due.

These tests capture the essence. If they pass, you are no longer building a session-based chat agent.

---

# The major architectural correction

My previous answer over-indexed on making the agent itself the durable actor.

For AgentOS, the better decomposition is:

```text
World
  ├─ Workspace workflow / workspace roots
  ├─ Factory project workflow
  ├─ Work item cells
  ├─ Worker invocation cells
  ├─ aos-agent session cells
  ├─ Review/test workflows
  └─ Effects / receipts / timers / traces
```

Not:

```text
World
  └─ Giant Agent
      ├─ sessions
      ├─ schedules
      ├─ work queue
      ├─ memory
      ├─ skills
      └─ tools
```

The first design is AgentOS-native.

The second design recreates a monolithic agent daemon inside AgentOS.

---

# My revised recommendation

The current P4–P7 roadmap is still basically right, but it needs two additions and two scope cuts.

## Add now

1. **Wake/run cause support**

   * Small addition to P5.
   * Required to avoid chat-session semantics.

2. **Factory work item / agenda workflow**

   * Separate from `aos-agent`.
   * Required for the software factory product.

## Keep now

1. P4 tool bundles.
2. P5 session/run split.
3. P6 context engine.
4. P7 traces/intervention.
5. Host target policy shape from P8.
6. Repo/workspace instruction loading as P6 input.

## Defer

1. Full skills.
2. Full memory engine.
3. Full standing orders.
4. Policy/capability system.
5. Global cron.
6. Full Fabric productization.
7. Multi-world agent mesh.

The essence is:

> Build the first version around **workflow-driven work items, timer/event wakes, workspace artifacts, and `aos-agent` runs as one reasoning step inside that larger deterministic process**.

That gives you a dark software factory foundation without prematurely building every OpenClaw/Hermes feature.
