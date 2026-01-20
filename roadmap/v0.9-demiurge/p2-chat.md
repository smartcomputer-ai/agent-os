# P2: Demiurge Chat (Vanilla)

**Priority**: P2  
**Effort**: Medium  
**Risk if deferred**: Medium (blocks first-agent UX)  
**Status**: Draft

## Goal

Ship a minimal back-and-forth chatbot in the shell UI that calls `llm.generate`
and persists conversation state in a reducer. No tools, no streaming, just
send -> respond.

## Dependencies

- P1 Vault (secret injection for LLM API keys).

## Non-Goals (v0.9)

- Streaming tokens (see P4).
- Multi-agent orchestration, tools, retrieval, or memory.
- Long-term storage beyond reducer state.
- Remote auth or multi-tenant access.

## Decision Summary

1) **Backend lives as a real app** under `apps/demiurge/`:
   - `apps/demiurge/air/` for schemas, plan, caps, policy, manifest, secrets.
   - `apps/demiurge/reducer/` for the WASM reducer crate.
2) **Frontend lives in the shell** under `apps/shell/src/features/demiurge/`.
3) **Reducer owns chat state**; plans only orchestrate `llm.generate`.
4) **Prompt is stored as a blob by the UI**, and its hash (`input_ref`) is sent
   with the user message event.
5) **Plan emits a result event** with `output_ref` and token usage; reducer
   stores it and the UI reads the blob when rendering.

## Runtime Flow (Happy Path)

1) UI builds a message list (last N turns), stores it via `POST /api/blob`,
   gets `input_ref`.
2) UI posts `demiurge/UserMessage@1` to `POST /api/events`, including
   `input_ref`, model/provider settings, and the user text.
3) Reducer appends the user message and emits `demiurge/ChatRequest@1`
   (intent) with `input_ref` and settings.
4) Plan triggers on `demiurge/ChatRequest@1`, calls `llm.generate` with
   `api_key: SecretRef`, then emits `demiurge/ChatResult@1`.
5) Reducer appends the assistant message (stores `output_ref`, usage).
6) UI renders the assistant text by reading `GET /api/blob/<output_ref>`.

## Schemas (Minimal)

- `demiurge/ChatState@1`: `{ messages: list<Message>, last_request_id: nat }`
- `demiurge/UserMessage@1`: `{ request_id, text, input_ref, model, provider, max_tokens }`
- `demiurge/ChatRequest@1`: `{ request_id, input_ref, model, provider, max_tokens }`
- `demiurge/ChatResult@1`: `{ request_id, output_ref, token_usage }`
- `demiurge/ChatEvent@1`: union for reducer routing.

## Manifest Pieces

- Module: `demiurge/Demiurge@1`
- Plan: `demiurge/chat_plan@1` (single node: `llm.generate`)
- Cap: `demiurge/llm_basic@1` (cap type `sys/llm.basic@1`)
- Policy: allow `llm.generate` only from the plan
- Secret: `demiurge/llm_api@1` with `binding_id = env:LLM_API_KEY`
- Trigger: `demiurge/ChatRequest@1` -> `demiurge/chat_plan@1`
- Routing: `demiurge/ChatEvent@1` -> reducer

## UI Integration

- New route/panel in shell (ex: "Demiurge") and a simple chat layout.
- Use `sdk.eventsPost` to send user events.
- Use `sdk.stateGet` to fetch reducer state, poll `/api/journal` for updates,
  and switch to SSE when P4 lands.

## Tests

- Reducer unit tests for message append, request id, and result handling.
- Plan validation test: `llm.generate` params are normalized and cap-checked.
- Integration test with mock LLM adapter and secret resolver.

## CLI Smoke Test (Pre-UI)

1) Store chat messages JSON (OpenAI format) as a blob:
   - `echo '[{"role":"user","content":"Hello from AOS"}]' | aos blob put @-`
2) Send a user event (use `$tag`/`$value` variant encoding):
   - `aos event send demiurge/ChatEvent@1 '{"$tag":"UserMessage","$value":{"request_id":1,"text":"Hello","input_ref":"sha256:...","model":"gpt-4o-mini","provider":"openai","max_tokens":128}}'`
3) Read reducer state:
   - `aos state get demiurge/Demiurge@1`
4) Fetch assistant output:
   - `aos blob get <output_ref> --raw`

## Open Questions

- Should the reducer store assistant text directly (requires blob read path),
  or keep `output_ref` only?
- Do we want a separate "chat workspace" for ephemeral blobs?
