# 22-agent-live

Live SDK agent smoke fixture:
- workflow runtime wasm is compiled from `aos-agent-sdk` bin `session_workflow`,
- AIR `aos.agent/*` schemas are imported from `crates/aos-agent-sdk/air`,
- routing binds directly to keyed SDK module `aos.agent/SessionWorkflow@1`,
- dedicated agent workspace content lives under `agent-ws/` (prompt pack + tool catalog),
- runner drives `SessionIngress` lifecycle and tool-batch ingress events,
- runner seeds workspace state (`sys/WorkspaceCommit@1`), emits
  `WorkspaceSyncRequested` + `WorkspaceSnapshotReady` +
  `WorkspaceApplyRequested`, then runs the session,
- live LLM issues tool calls over multiple steps,
- harness emulates incremental search traversal,
- verifies final + follow-up answers and replay.
