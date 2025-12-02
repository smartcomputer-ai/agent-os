# P3: HTTP + LLM Adapters (path2)

**Goal:** Ship real effect adapters for HTTP and LLM, with sensible defaults and guardrails.

## HTTP Adapter (reqwest)

- Handles `http.request`.
- Config: `timeout`, `max_body_bytes`, `allowed_hosts` (empty = allow all), `user_agent`.
- Steps: decode params (URL/method/headers/body) → host allowlist check → send → cap body size → emit receipt with status/headers/body bytes. Map errors to `ReceiptStatus::Error`.

## LLM Adapter (OpenAI-compatible)

- Handles `llm.generate`.
- Config from env: `OPENAI_API_KEY`, optional `OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_TIMEOUT`.
- Request: chat/completions with messages/model/temperature/max_tokens; response → receipt with content, model, token usage, finish_reason; naive cost cents computed from usage (configurable rate).
- Hard limits: max prompt/response tokens; reject if missing API key.

## Integration into Host

- Adapters stay in `aos-host::adapters` for now; registry wires them in `RuntimeConfig`.
- In daemon mode, register timer+http; register llm only if key present.

## Tasks

1) Add `reqwest`, `url`, `serde_json` deps; wrap in feature flag `adapter-http` (default on).
2) Implement HTTP adapter with host allowlist + body cap tests.
3) Implement LLM adapter with env config + error handling.
4) Add config surface in `RuntimeConfig` (`http`, optional `llm`).
5) Update CLI to expose minimal flags/env hints.
6) Smoke-test `examples/03-fetch-notify` and `examples/07-llm-summarizer` (with key).

## Success Criteria

- HTTP requests succeed and enforce host/size limits; errors return receipts, not panics.
- LLM calls work with OpenAI-compatible endpoints; missing key produces deterministic error receipt.
