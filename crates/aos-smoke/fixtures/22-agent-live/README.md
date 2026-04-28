# 22-agent-live

Live SDK agent smoke fixture:
- workflow runtime wasm is compiled from `aos-agent` bin `session_workflow`,
- AIR `aos.agent/*` schemas are imported from `crates/aos-agent/air`,
- routing binds directly to keyed SDK module `aos.agent/SessionWorkflow@1`,
- fixture prompt/tool assets are embedded in the runner (no external asset dir),
- runner drives `SessionInput` lifecycle and tool-batch observe/settle input events,
- runner uploads the default prompt pack blob and passes it as `default_prompt_refs`,
- live LLM issues tool calls over multiple steps,
- harness emulates incremental search traversal,
- verifies final + follow-up answers and replay.
