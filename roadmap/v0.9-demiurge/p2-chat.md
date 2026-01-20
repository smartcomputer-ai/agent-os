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
4) **Each message is a CAS blob**, and the reducer stores ordered `message_refs`.
   User events carry the new `message_ref`; reducer emits `message_refs` in the intent.
5) **Plan emits a result event** with `output_ref` (assistant message blob) and
   token usage; reducer stores it and the UI reads the blob when rendering.

## Runtime Flow (Happy Path)

1) UI writes the user message to CAS as a message blob and gets `message_ref`.
2) UI posts `demiurge/UserMessage@1` to `POST /api/events`, including
   `message_ref`, model/provider settings, and the user text (for state display).
3) Reducer appends the user message and emits `demiurge/ChatRequest@1`
   (intent) with ordered `message_refs` (last N turns) and settings.
4) Plan triggers on `demiurge/ChatRequest@1`, calls `llm.generate` with
   `message_refs` and `api_key: SecretRef`, then emits `demiurge/ChatResult@1`.
5) LLM adapter stores assistant message as a CAS message blob and returns
   `output_ref` in the receipt; reducer appends assistant message metadata.
6) UI renders the assistant message by reading `GET /api/blob/<output_ref>`.

## Schemas (Minimal)

- `demiurge/ChatState@1`: `{ messages: list<Message>, last_request_id: nat }`
- `demiurge/UserMessage@1`: `{ request_id, text, message_ref, model, provider, max_tokens }`
- `demiurge/ChatRequest@1`: `{ request_id, message_refs, model, provider, max_tokens }`
- `demiurge/ChatResult@1`: `{ request_id, output_ref, token_usage }`
- `demiurge/ChatEvent@1`: union for reducer routing.

Message blob (CAS):
- Store one message per blob. Use a stable JSON shape such as:
  - `{ "role": "user|assistant|system|tool", "content": [ContentPart...], "tool_calls"?: [...] }`
- `ContentPart` supports typed parts:
  - text: `{ "type": "text", "text": "..." }`
  - image: `{ "type": "image", "mime": "image/png", "bytes_ref": "sha256:..." }`
  - audio: `{ "type": "audio", "mime": "audio/wav", "bytes_ref": "sha256:..." }`
- This keeps large binaries in CAS while the message blob only references them.

LLM effect schema update (global):
- Update `sys/LlmGenerateParams@1` to use `message_refs: list<hash>`.
- Adapter assembles provider messages by loading each message blob and
  expanding referenced attachments.

## Manifest Pieces

- Module: `demiurge/Demiurge@1`
- Plan: `demiurge/chat_plan@1` (single node: `llm.generate`)
- Cap: `demiurge/llm_basic@1` (cap type `sys/llm.basic@1`)
- Policy: allow `llm.generate` only from the plan
- Secret: `llm/api@1` with `binding_id = env:LLM_API_KEY`
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
- Adapter tests for `message_refs` expansion and attachment handling.
- Integration test with real LLM adapter + env secret resolver (smoke).

## CLI Smoke Test (Pre-UI)

1) Store a user message blob:
   - `echo '{"role":"user","content":[{"type":"text","text":"Hello from AOS"}]}' | aos blob put @-`
2) Send a user event (use `$tag`/`$value` variant encoding):
   - `aos event send demiurge/ChatEvent@1 '{"$tag":"UserMessage","$value":{"request_id":1,"text":"Hello","message_ref":"sha256:...","model":"gpt-4o-mini","provider":"openai","max_tokens":128}}'`
3) Read reducer state:
   - `aos state get demiurge/Demiurge@1`
4) Fetch assistant output:
   - `aos blob get <output_ref> --raw`

## Open Questions

- Should the reducer store assistant text directly (requires blob read path),
  or keep `output_ref` only?
- Do we want adapter-side message windowing based on token budget, or keep
  it reducer-side (truncate `message_refs` before emit)?
- Do we want a separate "chat workspace" for ephemeral blobs?
