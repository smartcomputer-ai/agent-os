# 22-agent-live

Live SDK agent smoke fixture:
- reducer is built on `aos-agent-sdk` session primitives,
- AIR session schemas are imported from `aos-agent-sdk` defs-only export,
- fixture-local `demo/session_workspace_sync_plan@1` bridges
  `WorkspaceSyncRequested` session events into `workspace.*` plan effects,
- dedicated agent workspace content lives under `agent-ws/` (prompt pack + tool catalog),
- runner drives `SessionEvent` lifecycle and tool-batch events,
- runner seeds workspace state (`sys/WorkspaceCommit@1`), emits
  `WorkspaceSyncRequested` + `WorkspaceApplyRequested`, then runs the session,
- live LLM issues tool calls over multiple steps,
- harness emulates incremental search traversal,
- verifies final + follow-up answers and replay.
