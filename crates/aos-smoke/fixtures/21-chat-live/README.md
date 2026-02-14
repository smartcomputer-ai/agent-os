# 21-chat-live

Opt-in live chat smoke fixture:
- secret-injected `llm.generate` via `defsecret` aliases for OpenAI/Anthropic,
- tool-call + tool-result roundtrip driven by `aos-smoke chat-live`,
- follow-up user turn,
- replay verification at the end of the run.
