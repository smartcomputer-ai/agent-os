# 22-agent-live

Live SDK agent smoke fixture:
- reducer is built on `aos-agent-sdk` session primitives,
- runner drives `SessionEvent` lifecycle and tool-batch events,
- live LLM issues tool calls over multiple steps,
- harness emulates incremental search traversal,
- verifies final + follow-up answers and replay.
