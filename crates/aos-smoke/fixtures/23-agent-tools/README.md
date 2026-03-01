# 23-agent-tools

Built-in SDK tool end-to-end smoke fixture:
- workflow runtime wasm is compiled from `aos-agent` bin `session_workflow`,
- AIR `aos.agent/*` schemas are imported from `crates/aos-agent/air`,
- runner drives `SessionIngress` and scripted LLM receipts,
- first LLM turn emits multiple built-in host tool calls,
- workflow resolves argument refs, plans parallel groups, maps to host effects,
- tool receipts are translated back into follow-up tool output messages,
- second LLM turn returns final assistant text, then run completes,
- replay parity is verified.
