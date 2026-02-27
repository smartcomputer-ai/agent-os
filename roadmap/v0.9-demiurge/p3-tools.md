# P3: Demiurge Tools (CAS Specs + Introspect/Workspace)

**Priority**: P3  
**Effort**: Medium  
**Risk if deferred**: Medium (limits agent usefulness)  
**Status**: Complete

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
2) **`sys/LlmGenerateParams@1` gains `tool_refs` + `tool_choice`**:
   - `tool_refs: option<list<hash>>` points to CAS blobs with provider tool JSON.
   - `tool_choice` uses a small generic schema; adapter maps to provider shape.
3) **LLM adapter targets the Responses API** (`POST /v1/responses`).
4) **LLM adapter loads `tool_refs` and injects into provider request**.
5) **LLM adapter returns `output_ref` with full Responses `output` array**.
6) **Reducer interprets tool calls**:
   - Validates/normalizes parameters, applies allowlists.
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
  tool_refs?: list<hash>
  tool_choice?: sys/LlmToolChoice@1
  api_key?: sys/TextOrSecretRef@1
}
```

### Update `sys/LlmGenerateReceipt@1`
```
record {
  output_ref: hash
  token_usage: { prompt: nat, completion: nat }
  cost_cents: nat
  provider_id: text
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
- Cache by hash; plans can re-use the same `tool_refs`.
- For `openai-responses`, tool entries should use the Responses shape
  (`{ "type": "function", "name": "...", "parameters": ... }`). The adapter
  will normalize Chat-style function wrappers if present.

## Runtime Flow (Happy Path)

0) UI stores tool spec JSON in CAS and receives `tool_refs`.
1) UI includes `tool_refs` + `tool_choice` on `demiurge/UserMessage@1`.
2) Reducer emits `demiurge/ChatRequest@1` with `tool_refs` + `tool_choice`.
3) Plan calls `llm.generate` (Responses API) with `tool_refs` and `tool_choice`.
4) LLM adapter loads tool spec from CAS and sends it to the provider.
5) Receipt returns `output_ref`; plan emits `demiurge/ChatResult@1`.
6) Reducer appends the assistant message and, when tool parsing is enabled, validates/normalizes
   parameters and emits `demiurge/ToolCallRequested@1` for approved calls.
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
- Update LLM policy to allow tool usage via `tool_refs`.

## Tests

- Adapter: tool spec load from CAS, tool_choice mapping.
- Plan: tool dispatch for introspect/workspace from `ToolCallRequested`.
- Reducer: tool call validation/allowlist + result append + re-request flow.
- Integration: end-to-end tool call with mock adapter.

## Open Questions

- Do we want to allow per-message tool overrides or only per-chat toolset?
- Should tool result messages be stored as JSON or normalized text?
- Do we need `tool_choice` on the reducer state to keep continuity?
- Do we persist Responses `previous_response_id` for optional stateful chaining?
