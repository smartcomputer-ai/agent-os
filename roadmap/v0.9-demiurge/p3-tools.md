# P3: Demiurge Tools (CAS Specs + Introspect/Workspace)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (limits agent usefulness)  
**Status**: Proposed

## Goal

Add tool usage to the Demiurge chat agent by:
- Storing large tool specs in CAS and referencing them from `llm.generate`.
- Returning tool calls from the LLM adapter as CAS refs.
- Interpreting tool calls in the reducer; plans only execute approved tool intents.
- Shipping initial tools: **Introspect** and **Workspace**.

## Non-Goals (v0.9)

- Tool streaming (see P4).
- Arbitrary "function" execution in reducers.
- Multi-agent tool sharing.
- Full OpenAI/Anthropic parity for every tool feature.

## Decision Summary

1) **Tool specs live in CAS** as provider-specific JSON payloads.
2) **`sys/LlmGenerateParams@1` gains `tools_ref` + `tool_choice`**:
   - `tools_ref: option<hash>` points to a CAS blob with provider tool JSON.
   - `tool_choice` uses a small generic schema; adapter maps to provider shape.
3) **LLM adapter targets the Responses API** (`POST /v1/responses`).
4) **LLM adapter loads `tools_ref` and injects into provider request**.
5) **LLM adapter stores tool calls in CAS and returns `tool_call_refs`** (optionally `output_items_ref`).
6) **Reducer interprets tool calls**:
   - Loads tool call blobs, validates/normalizes parameters, applies allowlists.
   - Emits `demiurge/ToolCallRequested@1` intents for approved calls.
7) **Tool execution is plan-only**:
   - Plan executes tool effects from `ToolCallRequested`, writes tool result message blobs,
     emits `demiurge/ToolResult@1` for the reducer.
8) **Reducer owns state**:
   - Appends tool call/result message refs.
   - Emits the next `ChatRequest` when tool results are ready.

## Proposed Schemas (additions)

### `sys/LlmToolChoice@1`
```
union {
  Auto { }
  None { }
  Required { }
  Tool { name: text }
}
```

### Update `sys/LlmGenerateParams@1`
```
record {
  provider: text
  model: text
  temperature: dec128
  max_tokens: nat
  message_refs: list<hash>
  tools_ref?: hash
  tool_choice?: sys/LlmToolChoice@1
  api_key?: sys/TextOrSecretRef@1
  response_format?: sys/LlmTextFormat@1
}
```

### Update `sys/LlmGenerateReceipt@1`
```
record {
  output_ref: hash
  token_usage: { prompt: nat, completion: nat }
  cost_cents: nat
  provider_id: text
  tool_call_refs?: list<hash>
  output_items_ref?: hash
}
```

### `sys/LlmToolCall@1`
```
record { id: text, name: text, arguments_json: text }
```

Notes:
- `sys/LlmToolCall@1` blobs are stored in CAS; receipts only reference them.
- `arguments_json` is raw JSON string from the provider for determinism.
- Provider mapping rules live in the adapter, not the schema.
- `id` should map to Responses `call_id` where available.

### `sys/LlmOutputItem@1`
```
union {
  Message { role: text, content: list<sys/LlmContentPart@1> }
  ToolCall { id: text, name: text, arguments_json: text }
  ToolOutput { id: text, content: text }
  Reasoning { summary?: list<text> }
}
```

Notes:
- `output_items_ref` points to a CAS blob holding `list<sys/LlmOutputItem@1>`.

### `sys/LlmTextFormat@1`
```
union {
  JsonSchema { name: text, schema_json: text, strict: bool }
}
```

## Tool Spec Blob (CAS)

Store a single JSON object that the adapter can inject directly:

```
{
  "provider": "openai-responses",
  "tools": [ ...provider schema... ],
  "tool_choice": { ...provider schema... }
}
```

Guidelines:
- Keep tool specs provider-specific.
- Use one blob per toolset to allow reuse across chats.
- Cache by hash; plans can re-use the same `tools_ref`.

## Tool Call Blob (CAS)

Each tool call is stored as a blob using `sys/LlmToolCall@1`:

```
{ "id": "...", "name": "...", "arguments_json": "{...raw json...}" }
```

This keeps large tool calls out of receipts while preserving deterministic replay.

## Runtime Flow (Happy Path)

0) UI stores tool spec JSON in CAS and receives `tools_ref`.
1) UI includes `tools_ref` + `tool_choice` on `demiurge/UserMessage@1`.
2) Reducer emits `demiurge/ChatRequest@1` with `tools_ref` + `tool_choice`.
3) Plan calls `llm.generate` (Responses API) with `tools_ref` and `tool_choice`.
4) LLM adapter loads tool spec from CAS and sends it to the provider.
5) Receipt returns `output_ref` + `tool_call_refs` (and optional `output_items_ref`);
   plan emits `demiurge/ChatResult@1` containing the refs.
6) Reducer appends the assistant message and, if `tool_call_refs` exist, loads each
   tool call blob, validates/normalizes parameters, and emits
   `demiurge/ToolCallRequested@1` for approved calls.
7) Tool plan executes introspect/workspace effects, writes a tool-result message
   blob (`role=tool`, `tool_call_id`, `content`), emits `demiurge/ToolResult@1`.
8) Reducer appends tool result and emits a new `ChatRequest` to continue.

## Tool Set v1

### Introspect
- Uses `sys/Introspect` effects.
- Tools:
  - `introspect.schemas`
  - `introspect.module`
  - `introspect.state`
  - `introspect.manifest`
- Result is stored as a tool message blob, content is JSON text.

### Workspace
- Uses `workspace.*` plan-only internal effects.
- Tools:
  - `workspace.list`
  - `workspace.read`
  - `workspace.write`
- Cap-gated by `sys/workspace` caps and policies.

## Manifest Updates

- New plan: `demiurge/tool_plan@1` (dispatch by `ToolCallRequested` event).
- New schemas for `ToolCallRequested` and `ToolResult`.
- Update `demiurge/ChatResult@1` to include `tool_call_refs` + `output_items_ref`.
- Update LLM policy to allow tool usage via `tools_ref`.

## Tests

- Adapter: tool spec load from CAS, tool_choice mapping, tool_call_refs decoding.
- Adapter: Responses output items parsing (message/tool_call/tool_output).
- Plan: tool dispatch for introspect/workspace from `ToolCallRequested`.
- Reducer: tool call validation/allowlist + result append + re-request flow.
- Integration: end-to-end tool call with mock adapter.

## Open Questions

- Do we want to allow per-message tool overrides or only per-chat toolset?
- Should tool result messages be stored as JSON or normalized text?
- Do we need `tool_choice` on the reducer state to keep continuity?
- Do we persist Responses `previous_response_id` for optional stateful chaining?
