# v0.30 Agent Roadmap

This roadmap updates the older agent plan after the AIR v2 and DX refactors.

Start with `background.md` for the current implementation model that these changes build on.

Boundary for this roadmap:

1. `aos-agent` remains the reusable LLM session/run SDK, not a factory runtime.
2. Native AOS workflows should own long-running domain processes such as work ledgers, project orchestration, schedules, worker invocation, review, and test flows.
3. Those workflows interact with agent sessions through ordinary domain events and `routing.subscriptions`.
4. v0.30 adds the agent-side seams those workflows need: explicit bundles, durable sessions, run provenance, context inputs, traces, intervention, and host target config.
5. v0.30 does not add policy/capability gating, full scheduler/heartbeat semantics, factory work-item workflows, full skills, or a memory engine to `aos-agent` core.

**Important**: It is okay to focus only on aos-agent for now and assume we'll break downstream users, we can fix them later once the agent sdk reaches its desired shape. This allows us to refactor more agressively, without worrying about backward compatibility.

The current sequence is:

1. `p4-tool-bundle-refactoring.md`
   - make built-in tools explicit bundles,
   - stop treating one coding-agent profile as core,
   - account for Rust-authored AIR effect surfaces and host target config,
   - support generic domain-event tool bundles without baking factory semantics into the SDK.
2. `p5-session-run-model.md`
   - split durable session status from per-run lifecycle,
   - make transcript/context state session-scoped and active execution run-scoped,
   - add extensible run provenance so runs are not implicitly user-turn driven.
3. `p6-context-engine.md`
   - add deterministic, inspectable context planning after the session/run split,
   - accept source-agnostic run-cause, domain, artifact, and instruction inputs.
4. `p7-run-traces-and-intervention.md`
   - add run traces plus explicit follow-up, steer, interrupt, cancel, pause, and resume semantics,
   - carry run-cause and correlation metadata without making traces factory-specific.
5. `p8-fabric-hosted-execution.md`
   - define the host target config shape needed by P4/P5,
   - prove Fabric-backed hosted execution through canonical host effects without making Fabric core.
6. `p9-skills.md`
   - add skills as an implementation-layer feature after context and traces are stable.
7. `p10-agent-sdk-testing.md`
   - make `aos-harness-py` the primary deterministic SDK test harness,
   - keep `aos-agent-eval` as the live provider/tool acceptance lane during migration.

`p10-agent-sdk-testing.md` is numbered last because it is cross-cutting, not because it should wait
until all other work is complete. Start using it as soon as P4 exposes explicit bundle/profile
selection, then expand the Python fixture layer as P5-P9 land.

Deferred for later roadmaps:

1. timer-chain schedules, heartbeats, and external trigger services beyond run provenance,
2. factory/project/work-item workflow families,
3. worker invocation, review, and test orchestration workflows,
4. subagent/session-tree supervision,
5. memory/RAG infrastructure,
6. approval policy, permission UX, and capability gating,
7. UI/operator product design,
8. marketplace or package distribution for skills/tools.
