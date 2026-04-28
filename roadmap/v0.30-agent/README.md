# v0.30 Agent Roadmap

This roadmap updates the older agent plan after the AIR v2 and DX refactors.

Start with `background.md` for the current implementation model that these changes build on.

The current sequence is:

1. `p4-tool-bundle-refactoring.md`
   - make built-in tools explicit bundles,
   - stop treating one coding-agent profile as core,
   - account for Rust-authored AIR effect surfaces and host target policy.
2. `p5-session-run-model.md`
   - split durable session status from per-run lifecycle,
   - make transcript/context state session-scoped and active execution run-scoped.
3. `p6-context-engine.md`
   - add deterministic, inspectable context planning after the session/run split.
4. `p7-run-traces-and-intervention.md`
   - add run traces plus explicit follow-up, steer, interrupt, cancel, pause, and resume semantics.
5. `p8-fabric-hosted-execution.md`
   - prove Fabric-backed hosted execution through canonical host effects without making Fabric core.
6. `p9-skills.md`
   - add skills as an implementation-layer feature after context and traces are stable.

Deferred for later roadmaps:

1. subagent/session-tree supervision,
2. memory/RAG infrastructure,
3. approval policy and permission UX,
4. UI/operator product design,
5. marketplace or package distribution for skills/tools.
