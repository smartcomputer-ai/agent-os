# 22-agent-live

Live SDK agent smoke fixture:
- workflow is built on `aos-agent-sdk` session primitives,
- AIR `aos.agent/*` schemas are imported from `crates/aos-agent-sdk/air`,
- dedicated agent workspace content lives under `agent-ws/` (prompt pack + tool catalog),
- runner drives `SessionWorkflowEvent` ingress lifecycle and tool-batch events,
- runner seeds workspace state (`sys/WorkspaceCommit@1`), emits
  `WorkspaceSyncRequested` + `WorkspaceSnapshotReady` +
  `WorkspaceApplyRequested`, then runs the session,
- live LLM issues tool calls over multiple steps,
- harness emulates incremental search traversal,
- verifies final + follow-up answers and replay.
