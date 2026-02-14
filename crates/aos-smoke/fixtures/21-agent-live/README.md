# 21-agent-live

Opt-in live Agent SDK smoke fixture:
- secret-injected `llm.generate` via `defsecret` aliases for OpenAI/Anthropic,
- tool-call + tool-result roundtrip driven by `aos-smoke agent-live`,
- follow-up user turn,
- replay verification at the end of the run.
