#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::str;
use aos_wasm_sdk::{aos_event_union, aos_reducer, aos_variant, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

const REQUEST_SCHEMA: &str = "demiurge/ChatRequest@1";
const TOOL_REGISTRY_SCAN_SCHEMA: &str = "demiurge/ToolRegistryScanRequested@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct ChatState {
    messages: Vec<ChatMessage>,
    last_request_id: u64,
    title: Option<String>,
    created_at_ms: Option<u64>,
    model: Option<String>,
    provider: Option<String>,
    max_tokens: Option<u64>,
    tool_refs: Option<Vec<String>>,
    tool_registry_version: Option<u64>,
    tool_choice: Option<LlmToolChoice>,
    #[serde(default)]
    pending_chat_requests: Vec<PendingChatRequest>,
    #[serde(default)]
    pending_outputs: Vec<PendingOutput>,
    #[serde(default)]
    pending_tool_outputs: Vec<PendingToolOutput>,
    #[serde(default)]
    pending_tool_messages: Vec<PendingToolMessage>,
}

aos_variant! {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    enum ChatRole {
        User,
        Assistant,
    }
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    enum LlmToolChoice {
        Auto,
        #[serde(rename = "None")]
        NoneChoice,
        Required,
        Tool { name: String },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TokenUsage {
    prompt: u64,
    completion: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChatMessage {
    request_id: u64,
    role: ChatRole,
    text: Option<String>,
    message_ref: Option<String>,
    token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingOutput {
    chat_id: String,
    request_id: u64,
    output_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingToolOutput {
    chat_id: String,
    request_id: u64,
    tool_call_id: String,
    output_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingToolMessage {
    chat_id: String,
    request_id: u64,
    tool_call_id: String,
    expected_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingChatRequest {
    chat_id: String,
    request_id: u64,
    message_refs: Vec<String>,
    model: String,
    provider: String,
    max_tokens: u64,
    tool_choice: Option<LlmToolChoice>,
}

aos_event_union! {
    #[derive(Debug, Clone, Serialize)]
    enum ChatEvent {
        ChatCreated(ChatCreated),
        UserMessage(UserMessage),
        ChatResult(ChatResult),
        ToolResult(ToolResult),
        ToolRegistrySnapshot(ToolRegistrySnapshot),
        ToolRegistryUnchanged(ToolRegistryUnchanged),
        ToolError(ToolError),
        BlobGetResult(BlobGetResult),
        BlobPutResult(BlobPutResult),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatCreated {
    chat_id: String,
    title: String,
    created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserMessage {
    chat_id: String,
    request_id: u64,
    text: String,
    message_ref: String,
    model: String,
    provider: String,
    max_tokens: u64,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatRequest {
    chat_id: String,
    request_id: u64,
    message_refs: Vec<String>,
    model: String,
    provider: String,
    max_tokens: u64,
    tool_refs: Option<Vec<String>>,
    tool_choice: Option<LlmToolChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatResult {
    chat_id: String,
    request_id: u64,
    output_ref: String,
    token_usage: TokenUsage,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ToolCall {
    id: String,
    name: String,
    arguments_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolRegistryScanRequested {
    chat_id: String,
    request_id: u64,
    known_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolRegistrySnapshot {
    chat_id: String,
    request_id: u64,
    version: Option<u64>,
    entries: Vec<WorkspaceListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolRegistryUnchanged {
    chat_id: String,
    request_id: u64,
    version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolError {
    chat_id: String,
    request_id: u64,
    #[serde(default)]
    tool_call_id: Option<String>,
    stage: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceListEntry {
    path: String,
    kind: String,
    hash: Option<String>,
    size: Option<u64>,
    mode: Option<u64>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    enum ToolCallParams {
        IntrospectManifest { consistency: String },
        WorkspaceReadBytes { workspace: String, version: Option<u64>, path: String },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobGetParams {
    blob_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobGetReceipt {
    blob_ref: String,
    size: u64,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobGetResult {
    status: String,
    requested: BlobGetParams,
    receipt: BlobGetReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobPutParams {
    blob_ref: String,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobPutReceipt {
    blob_ref: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobPutResult {
    status: String,
    requested: BlobPutParams,
    receipt: BlobPutReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolResult {
    chat_id: String,
    request_id: u64,
    tool_call_id: String,
    result_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCallRequested {
    chat_id: String,
    request_id: u64,
    tool_call_id: String,
    params: ToolCallParams,
}

aos_reducer!(DemiurgeReducer);

#[derive(Default)]
struct DemiurgeReducer;

impl Reducer for DemiurgeReducer {
    type State = ChatState;
    type Event = ChatEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        match event {
            ChatEvent::ChatCreated(created) => handle_chat_created(ctx, created),
            ChatEvent::UserMessage(message) => handle_user_message(ctx, message),
            ChatEvent::ChatResult(result) => handle_chat_result(ctx, result),
            ChatEvent::ToolResult(result) => handle_tool_result(ctx, result),
            ChatEvent::ToolRegistrySnapshot(snapshot) => {
                handle_tool_registry_snapshot(ctx, snapshot)
            }
            ChatEvent::ToolRegistryUnchanged(unchanged) => {
                handle_tool_registry_unchanged(ctx, unchanged)
            }
            ChatEvent::ToolError(error) => handle_tool_error(ctx, error),
            ChatEvent::BlobGetResult(result) => handle_blob_get_result(ctx, result),
            ChatEvent::BlobPutResult(result) => handle_blob_put_result(ctx, result),
        }
        Ok(())
    }
}

fn handle_chat_created(ctx: &mut ReducerCtx<ChatState, ()>, created: ChatCreated) {
    if ctx.state.title.is_some() {
        return;
    }

    ctx.state.title = Some(created.title);
    ctx.state.created_at_ms = Some(created.created_at_ms);
}

fn handle_user_message(ctx: &mut ReducerCtx<ChatState, ()>, message: UserMessage) {
    let UserMessage {
        chat_id,
        request_id,
        text,
        message_ref,
        model,
        provider,
        max_tokens,
        tool_refs,
        tool_choice,
    } = message;

    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    if request_id <= ctx.state.last_request_id {
        return;
    }

    ctx.state.last_request_id = request_id;
    ctx.state.model = Some(model.clone());
    ctx.state.provider = Some(provider.clone());
    ctx.state.max_tokens = Some(max_tokens);
    ctx.state.tool_choice = tool_choice.clone();
    ctx.state.messages.push(ChatMessage {
        request_id,
        role: ChatRole::User,
        text: Some(text),
        message_ref: Some(message_ref.clone()),
        token_usage: None,
    });

    let mut message_refs: Vec<String> = ctx
        .state
        .messages
        .iter()
        .filter_map(|msg| msg.message_ref.clone())
        .collect();
    const MAX_MESSAGE_REFS: usize = 32;
    if message_refs.len() > MAX_MESSAGE_REFS {
        let start = message_refs.len() - MAX_MESSAGE_REFS;
        message_refs = message_refs.split_off(start);
    }

    let pending_request = PendingChatRequest {
        chat_id: chat_id.clone(),
        request_id,
        message_refs,
        model,
        provider,
        max_tokens,
        tool_choice,
    };

    if let Some(explicit_refs) = tool_refs {
        ctx.state.tool_refs = Some(explicit_refs.clone());
        ctx.state.tool_registry_version = None;
        emit_chat_request_with_refs(ctx, pending_request, Some(explicit_refs));
        return;
    }

    ctx.state.pending_chat_requests.push(pending_request);
    if ctx.state.pending_chat_requests.len() > 1 {
        return;
    }

    let intent_value = ToolRegistryScanRequested {
        chat_id,
        request_id,
        known_version: ctx.state.tool_registry_version,
    };
    let key = request_id.to_be_bytes();
    ctx.intent(TOOL_REGISTRY_SCAN_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_chat_result(ctx: &mut ReducerCtx<ChatState, ()>, result: ChatResult) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    if result.request_id > ctx.state.last_request_id {
        return;
    }

    let mut has_user = false;
    let mut has_output = false;
    for message in &ctx.state.messages {
        if message.request_id != result.request_id {
            continue;
        }
        match message.role {
            ChatRole::User => has_user = true,
            ChatRole::Assistant => {
                if message.message_ref.as_deref() == Some(result.output_ref.as_str()) {
                    has_output = true;
                }
            }
        }
        if has_user && has_output {
            break;
        }
    }

    if !has_user || has_output {
        return;
    }

    let output_ref = result.output_ref.clone();
    ctx.state.messages.push(ChatMessage {
        request_id: result.request_id,
        role: ChatRole::Assistant,
        text: None,
        message_ref: Some(output_ref.clone()),
        token_usage: Some(result.token_usage),
    });

    let should_parse = ctx
        .state
        .tool_refs
        .as_ref()
        .map(|refs| !refs.is_empty())
        .unwrap_or(false);
    if should_parse {
        ctx.state.pending_outputs.push(PendingOutput {
            chat_id: result.chat_id,
            request_id: result.request_id,
            output_ref: output_ref.clone(),
        });
        let params = BlobGetParams {
            blob_ref: output_ref,
        };
        ctx.effects().emit_raw("blob.get", &params, Some("blob"));
    }
}

fn emit_chat_request(
    ctx: &mut ReducerCtx<ChatState, ()>,
    chat_id: String,
    request_id: u64,
    tool_choice_override: Option<LlmToolChoice>,
) {
    let mut message_refs: Vec<String> = ctx
        .state
        .messages
        .iter()
        .filter_map(|msg| msg.message_ref.clone())
        .collect();
    const MAX_MESSAGE_REFS: usize = 32;
    if message_refs.len() > MAX_MESSAGE_REFS {
        let start = message_refs.len() - MAX_MESSAGE_REFS;
        message_refs = message_refs.split_off(start);
    }

    let (Some(model), Some(provider), Some(max_tokens)) = (
        ctx.state.model.clone(),
        ctx.state.provider.clone(),
        ctx.state.max_tokens,
    ) else {
        return;
    };

    let tool_choice = tool_choice_override.or_else(|| ctx.state.tool_choice.clone());
    let pending = PendingChatRequest {
        chat_id,
        request_id,
        message_refs,
        model,
        provider,
        max_tokens,
        tool_choice,
    };
    emit_chat_request_with_refs(ctx, pending, ctx.state.tool_refs.clone());
}

fn emit_chat_request_with_refs(
    ctx: &mut ReducerCtx<ChatState, ()>,
    pending: PendingChatRequest,
    tool_refs: Option<Vec<String>>,
) {
    ctx.state.tool_refs = tool_refs.clone();
    let intent_value = ChatRequest {
        chat_id: pending.chat_id,
        request_id: pending.request_id,
        message_refs: pending.message_refs,
        model: pending.model,
        provider: pending.provider,
        max_tokens: pending.max_tokens,
        tool_refs,
        tool_choice: pending.tool_choice,
    };
    let key = intent_value.request_id.to_be_bytes();
    ctx.intent(REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_tool_result(ctx: &mut ReducerCtx<ChatState, ()>, result: ToolResult) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    ctx.state.pending_tool_outputs.push(PendingToolOutput {
        chat_id: result.chat_id,
        request_id: result.request_id,
        tool_call_id: result.tool_call_id,
        output_ref: result.result_ref.clone(),
    });

    let params = BlobGetParams {
        blob_ref: result.result_ref,
    };
    ctx.effects().emit_raw("blob.get", &params, Some("blob"));
}

fn handle_tool_registry_snapshot(
    ctx: &mut ReducerCtx<ChatState, ()>,
    snapshot: ToolRegistrySnapshot,
) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    let tool_refs = extract_tool_refs(&snapshot.entries);
    ctx.state.tool_refs = Some(tool_refs.clone());
    ctx.state.tool_registry_version = snapshot.version;

    if ctx.state.pending_chat_requests.is_empty() {
        return;
    }

    let pending = core::mem::take(&mut ctx.state.pending_chat_requests);
    for request in pending {
        emit_chat_request_with_refs(ctx, request, Some(tool_refs.clone()));
    }
}

fn handle_tool_registry_unchanged(
    ctx: &mut ReducerCtx<ChatState, ()>,
    unchanged: ToolRegistryUnchanged,
) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    if let Some(version) = unchanged.version {
        ctx.state.tool_registry_version = Some(version);
    }

    let Some(tool_refs) = ctx.state.tool_refs.clone() else {
        return;
    };

    if ctx.state.pending_chat_requests.is_empty() {
        return;
    }

    let pending = core::mem::take(&mut ctx.state.pending_chat_requests);
    for request in pending {
        emit_chat_request_with_refs(ctx, request, Some(tool_refs.clone()));
    }
}

fn handle_tool_error(ctx: &mut ReducerCtx<ChatState, ()>, error: ToolError) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    let text = format!("Tool error ({}): {}", error.stage, error.detail);
    ctx.state.messages.push(ChatMessage {
        request_id: error.request_id,
        role: ChatRole::Assistant,
        text: Some(text),
        message_ref: None,
        token_usage: None,
    });
}

fn extract_tool_refs(entries: &[WorkspaceListEntry]) -> Vec<String> {
    let mut refs = Vec::new();
    for entry in entries {
        if entry.kind != "file" {
            continue;
        }
        if !entry.path.starts_with("tools/") {
            continue;
        }
        if !entry.path.ends_with(".json") {
            continue;
        }
        if let Some(hash) = &entry.hash {
            refs.push(hash.clone());
        }
    }
    refs
}

#[derive(Serialize)]
struct TaggedEvent<'a, T: Serialize> {
    #[serde(rename = "$tag")]
    tag: &'static str,
    #[serde(rename = "$value")]
    value: &'a T,
}

fn emit_tool_error(
    ctx: &mut ReducerCtx<ChatState, ()>,
    chat_id: String,
    request_id: u64,
    tool_call_id: Option<String>,
    stage: &str,
    detail: String,
) {
    let error = ToolError {
        chat_id,
        request_id,
        tool_call_id,
        stage: stage.to_string(),
        detail,
    };
    let tagged = TaggedEvent {
        tag: "ToolError",
        value: &error,
    };
    ctx.intent("demiurge/ChatEvent@1").payload(&tagged).send();
}

fn handle_blob_put_result(ctx: &mut ReducerCtx<ChatState, ()>, result: BlobPutResult) {
    if result.status != "ok" {
        if let Some(index) = ctx
            .state
            .pending_tool_messages
            .iter()
            .position(|pending| pending.expected_ref == result.requested.blob_ref)
        {
            let pending = ctx.state.pending_tool_messages.remove(index);
            emit_tool_error(
                ctx,
                pending.chat_id,
                pending.request_id,
                Some(pending.tool_call_id),
                "tool_output_blob_put",
                result.status,
            );
        }
        return;
    }

    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    let Some(index) = ctx
        .state
        .pending_tool_messages
        .iter()
        .position(|pending| pending.expected_ref == result.receipt.blob_ref)
    else {
        return;
    };
    let pending = ctx.state.pending_tool_messages.remove(index);

    ctx.state.messages.push(ChatMessage {
        request_id: pending.request_id,
        role: ChatRole::Assistant,
        text: None,
        message_ref: Some(result.receipt.blob_ref),
        token_usage: None,
    });

    emit_chat_request(
        ctx,
        pending.chat_id,
        pending.request_id,
        Some(LlmToolChoice::Auto),
    );
}

fn handle_blob_get_result(ctx: &mut ReducerCtx<ChatState, ()>, result: BlobGetResult) {
    if result.status != "ok" {
        if let Some(index) = ctx
            .state
            .pending_outputs
            .iter()
            .position(|pending| pending.output_ref == result.requested.blob_ref)
        {
            let pending = ctx.state.pending_outputs.remove(index);
            emit_tool_error(
                ctx,
                pending.chat_id,
                pending.request_id,
                None,
                "llm_output_blob_get",
                result.status.clone(),
            );
            return;
        }
        if let Some(index) = ctx
            .state
            .pending_tool_outputs
            .iter()
            .position(|pending| pending.output_ref == result.requested.blob_ref)
        {
            let pending = ctx.state.pending_tool_outputs.remove(index);
            emit_tool_error(
                ctx,
                pending.chat_id,
                pending.request_id,
                Some(pending.tool_call_id),
                "tool_output_blob_get",
                result.status.clone(),
            );
        }
        return;
    }

    if let Some(index) = ctx
        .state
        .pending_outputs
        .iter()
        .position(|pending| pending.output_ref == result.requested.blob_ref)
    {
        let pending = ctx.state.pending_outputs.remove(index);

        let tool_calls = extract_tool_calls_from_output(&result.receipt.bytes);
        if tool_calls.is_empty() {
            return;
        }

        for call in tool_calls {
            if let Some(params) = parse_tool_call_params(&call.name, &call.arguments_json) {
                let intent_value = ToolCallRequested {
                    chat_id: pending.chat_id.clone(),
                    request_id: pending.request_id,
                    tool_call_id: call.id,
                    params,
                };
                let key = pending.request_id.to_be_bytes();
                ctx.intent("demiurge/ToolCallRequested@1")
                    .key_bytes(&key)
                    .payload(&intent_value)
                    .send();
            } else {
                emit_tool_error(
                    ctx,
                    pending.chat_id.clone(),
                    pending.request_id,
                    Some(call.id),
                    "unsupported_tool",
                    format!("Unsupported tool '{}'", call.name),
                );
            }
        }
        return;
    }

    let Some(index) = ctx
        .state
        .pending_tool_outputs
        .iter()
        .position(|pending| pending.output_ref == result.requested.blob_ref)
    else {
        return;
    };
    let pending = ctx.state.pending_tool_outputs.remove(index);

    let output_text = decode_tool_output(&result.receipt.bytes);
    let message_bytes = build_tool_message_bytes(&pending.tool_call_id, &output_text);
    let message_ref = hash_bytes(&message_bytes);
    ctx.state.pending_tool_messages.push(PendingToolMessage {
        chat_id: pending.chat_id,
        request_id: pending.request_id,
        tool_call_id: pending.tool_call_id,
        expected_ref: message_ref.clone(),
    });

    let params = BlobPutParams {
        blob_ref: message_ref,
        bytes: message_bytes,
    };
    ctx.effects().emit_raw("blob.put", &params, Some("blob"));
}

fn extract_tool_calls_from_output(bytes: &[u8]) -> Vec<ToolCall> {
    let value: JsonValue = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut calls = Vec::new();
    collect_tool_calls(&value, &mut calls);
    calls
}

fn collect_tool_calls(value: &JsonValue, calls: &mut Vec<ToolCall>) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_tool_calls(item, calls);
            }
        }
        JsonValue::Object(obj) => {
            if let Some(call) = parse_tool_call_item(value) {
                calls.push(call);
            }
            for key in ["output", "tool_calls", "content"] {
                if let Some(JsonValue::Array(items)) = obj.get(key) {
                    for item in items {
                        collect_tool_calls(item, calls);
                    }
                }
            }
        }
        _ => {}
    }
}

fn parse_tool_call_item(item: &JsonValue) -> Option<ToolCall> {
    let obj = item.as_object()?;
    let type_tag = obj.get("type").and_then(|value| value.as_str());

    if matches!(type_tag, Some("function_call") | Some("tool_call")) {
        let name = obj.get("name").and_then(|value| value.as_str())?;
        let id = obj
            .get("call_id")
            .and_then(|value| value.as_str())
            .or_else(|| obj.get("id").and_then(|value| value.as_str()))?;
        let arguments_json = arguments_to_json(obj.get("arguments_json").or_else(|| obj.get("arguments")));
        return Some(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json,
        });
    }

    if let Some(function_obj) = obj.get("function").and_then(|value| value.as_object()) {
        let name = function_obj.get("name").and_then(|value| value.as_str())?;
        let id = obj
            .get("call_id")
            .and_then(|value| value.as_str())
            .or_else(|| obj.get("id").and_then(|value| value.as_str()))?;
        let arguments_json =
            arguments_to_json(function_obj.get("arguments").or_else(|| obj.get("arguments")));
        return Some(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json,
        });
    }

    if let (Some(name), Some(id)) = (
        obj.get("name").and_then(|value| value.as_str()),
        obj.get("id")
            .and_then(|value| value.as_str())
            .or_else(|| obj.get("call_id").and_then(|value| value.as_str())),
    ) {
        if obj.contains_key("arguments") || obj.contains_key("arguments_json") {
            let arguments_json =
                arguments_to_json(obj.get("arguments_json").or_else(|| obj.get("arguments")));
            return Some(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments_json,
            });
        }
    }

    None
}

fn arguments_to_json(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".into()),
        None => "{}".into(),
    }
}

fn parse_tool_call_params(name: &str, arguments_json: &str) -> Option<ToolCallParams> {
    let args: JsonValue = serde_json::from_str(arguments_json).ok()?;
    let obj = args.as_object()?;
    match name {
        "introspect.manifest" | "introspect_manifest" => {
            let consistency = obj
                .get("consistency")
                .and_then(|value| value.as_str())
                .unwrap_or("head")
                .to_string();
            Some(ToolCallParams::IntrospectManifest { consistency })
        }
        "workspace.read" | "workspace.read_bytes" | "workspace_read_bytes" => {
            let workspace = obj.get("workspace").and_then(|value| value.as_str())?;
            let path = obj.get("path").and_then(|value| value.as_str())?;
            let version = obj.get("version").and_then(|value| value.as_u64());
            Some(ToolCallParams::WorkspaceReadBytes {
                workspace: workspace.to_string(),
                version,
                path: path.to_string(),
            })
        }
        _ => None,
    }
}

const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;

fn decode_tool_output(bytes: &[u8]) -> String {
    let (slice, truncated) = if bytes.len() > MAX_TOOL_OUTPUT_BYTES {
        (&bytes[..MAX_TOOL_OUTPUT_BYTES], true)
    } else {
        (bytes, false)
    };
    let mut text = match core::str::from_utf8(slice) {
        Ok(text) => text.to_string(),
        Err(_) => String::from_utf8_lossy(slice).to_string(),
    };
    if truncated {
        text.push_str("\n...[truncated]");
    }
    text
}

fn build_tool_message_bytes(tool_call_id: &str, output: &str) -> Vec<u8> {
    let message = serde_json::json!([
        {
            "type": "function_call_output",
            "call_id": tool_call_id,
            "output": output,
        }
    ]);
    serde_json::to_vec(&message).unwrap_or_else(|_| Vec::new())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use aos_wasm_abi::{DomainEvent, ReducerContext, ReducerInput, ReducerOutput, ABI_VERSION};
    use aos_wasm_sdk::step_bytes;

    const TEST_HASH: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000001";

    fn context_bytes(reducer: &str) -> Vec<u8> {
        let ctx = ReducerContext {
            now_ns: 1,
            logical_now_ns: 2,
            journal_height: 3,
            entropy: vec![0x11; 64],
            event_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            manifest_hash: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                .into(),
            reducer: reducer.into(),
            key: None,
            cell_mode: false,
        };
        serde_cbor::to_vec(&ctx).expect("context bytes")
    }

    fn run_with_state(state: Option<ChatState>, event: ChatEvent) -> ReducerOutput {
        fn encode_event_payload(event: ChatEvent) -> Vec<u8> {
            match event {
                ChatEvent::ChatCreated(value) => {
                    serde_cbor::to_vec(&value).expect("event bytes")
                }
                ChatEvent::UserMessage(value) => serde_cbor::to_vec(&value).expect("event bytes"),
                ChatEvent::ChatResult(value) => serde_cbor::to_vec(&value).expect("event bytes"),
                ChatEvent::ToolResult(value) => serde_cbor::to_vec(&value).expect("event bytes"),
                ChatEvent::ToolRegistrySnapshot(value) => {
                    serde_cbor::to_vec(&value).expect("event bytes")
                }
                ChatEvent::ToolRegistryUnchanged(value) => {
                    serde_cbor::to_vec(&value).expect("event bytes")
                }
                ChatEvent::ToolError(value) => serde_cbor::to_vec(&value).expect("event bytes"),
                ChatEvent::BlobGetResult(value) => serde_cbor::to_vec(&value).expect("event bytes"),
                ChatEvent::BlobPutResult(value) => serde_cbor::to_vec(&value).expect("event bytes"),
            }
        }
        let input = ReducerInput {
            version: ABI_VERSION,
            state: state.map(|s| serde_cbor::to_vec(&s).expect("state bytes")),
            event: DomainEvent::new("demiurge/ChatEvent@1", encode_event_payload(event)),
            ctx: Some(context_bytes("demiurge/Demiurge@1")),
        };
        let bytes = input.encode().expect("input bytes");
        let output = step_bytes::<DemiurgeReducer>(&bytes).expect("step");
        ReducerOutput::decode(&output).expect("decode")
    }

    #[test]
    fn chat_created_sets_title_and_created_at() {
        let event = ChatEvent::ChatCreated(ChatCreated {
            chat_id: "chat-1".into(),
            title: "First chat".into(),
            created_at_ms: 1234,
        });
        let output = run_with_state(None, event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(state.title.as_deref(), Some("First chat"));
        assert_eq!(state.created_at_ms, Some(1234));
    }

    #[test]
    fn user_message_appends_and_emits_request() {
        let state = ChatState {
            messages: vec![],
            last_request_id: 0,
            title: Some("First chat".into()),
            created_at_ms: Some(1234),
            model: None,
            provider: None,
            max_tokens: None,
            tool_refs: None,
            tool_registry_version: None,
            tool_choice: None,
            pending_chat_requests: vec![],
            pending_outputs: vec![],
            pending_tool_outputs: vec![],
            pending_tool_messages: vec![],
        };
        let event = ChatEvent::UserMessage(UserMessage {
            chat_id: "chat-1".into(),
            request_id: 1,
            text: "hello".into(),
            message_ref: TEST_HASH.into(),
            model: "gpt-mock".into(),
            provider: "mock".into(),
            max_tokens: 128,
            tool_refs: None,
            tool_choice: None,
        });
        let output = run_with_state(Some(state), event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(state.last_request_id, 1);
        assert_eq!(state.messages.len(), 1);
        let message = &state.messages[0];
        assert!(matches!(message.role, ChatRole::User));
        assert_eq!(message.text.as_deref(), Some("hello"));
        assert_eq!(message.message_ref.as_deref(), Some(TEST_HASH));

        assert_eq!(output.domain_events.len(), 1);
        assert_eq!(output.domain_events[0].schema, TOOL_REGISTRY_SCAN_SCHEMA);
        let scan: ToolRegistryScanRequested =
            serde_cbor::from_slice(&output.domain_events[0].value).expect("scan decode");
        assert_eq!(scan.chat_id, "chat-1");
        assert_eq!(scan.request_id, 1);
        assert_eq!(scan.known_version, None);

        assert_eq!(state.pending_chat_requests.len(), 1);
        let pending = &state.pending_chat_requests[0];
        assert_eq!(pending.chat_id, "chat-1");
        assert_eq!(pending.request_id, 1);
        assert_eq!(pending.message_refs, vec![String::from(TEST_HASH)]);
        assert_eq!(pending.model, "gpt-mock");
        assert_eq!(pending.provider, "mock");
        assert_eq!(pending.max_tokens, 128);
    }

    #[test]
    fn user_message_ignores_stale_request_id() {
        let state = ChatState {
            messages: vec![ChatMessage {
                request_id: 2,
                role: ChatRole::User,
                text: Some("hi".into()),
                message_ref: Some(TEST_HASH.into()),
                token_usage: None,
            }],
            last_request_id: 2,
            title: Some("First chat".into()),
            created_at_ms: Some(1234),
            model: None,
            provider: None,
            max_tokens: None,
            tool_refs: None,
            tool_registry_version: None,
            tool_choice: None,
            pending_chat_requests: vec![],
            pending_outputs: vec![],
            pending_tool_outputs: vec![],
            pending_tool_messages: vec![],
        };
        let event = ChatEvent::UserMessage(UserMessage {
            chat_id: "chat-1".into(),
            request_id: 1,
            text: "late".into(),
            message_ref: TEST_HASH.into(),
            model: "gpt-mock".into(),
            provider: "mock".into(),
            max_tokens: 64,
            tool_refs: None,
            tool_choice: None,
        });
        let output = run_with_state(Some(state.clone()), event);
        let next: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(next, state);
        assert!(output.domain_events.is_empty());
    }

    #[test]
    fn chat_result_appends_assistant_message() {
        let state = ChatState {
            messages: vec![ChatMessage {
                request_id: 1,
                role: ChatRole::User,
                text: Some("ping".into()),
                message_ref: Some(TEST_HASH.into()),
                token_usage: None,
            }],
            last_request_id: 1,
            title: Some("First chat".into()),
            created_at_ms: Some(1234),
            model: None,
            provider: None,
            max_tokens: None,
            tool_refs: None,
            tool_registry_version: None,
            tool_choice: None,
            pending_chat_requests: vec![],
            pending_outputs: vec![],
            pending_tool_outputs: vec![],
            pending_tool_messages: vec![],
        };
        let event = ChatEvent::ChatResult(ChatResult {
            chat_id: "chat-1".into(),
            request_id: 1,
            output_ref: TEST_HASH.into(),
            token_usage: TokenUsage {
                prompt: 10,
                completion: 20,
            },
            tool_calls: None,
        });
        let output = run_with_state(Some(state), event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(state.messages.len(), 2);
        let message = &state.messages[1];
        assert!(matches!(message.role, ChatRole::Assistant));
        assert_eq!(message.message_ref.as_deref(), Some(TEST_HASH));
        assert_eq!(
            message.token_usage,
            Some(TokenUsage {
                prompt: 10,
                completion: 20
            })
        );
        assert!(output.domain_events.is_empty());
    }

    #[test]
    fn tool_registry_snapshot_emits_chat_request() {
        let state = ChatState {
            messages: vec![ChatMessage {
                request_id: 1,
                role: ChatRole::User,
                text: Some("ping".into()),
                message_ref: Some(TEST_HASH.into()),
                token_usage: None,
            }],
            last_request_id: 1,
            title: Some("First chat".into()),
            created_at_ms: Some(1234),
            model: Some("gpt-mock".into()),
            provider: Some("mock".into()),
            max_tokens: Some(128),
            tool_refs: None,
            tool_registry_version: None,
            tool_choice: None,
            pending_chat_requests: vec![PendingChatRequest {
                chat_id: "chat-1".into(),
                request_id: 1,
                message_refs: vec![TEST_HASH.into()],
                model: "gpt-mock".into(),
                provider: "mock".into(),
                max_tokens: 128,
                tool_choice: None,
            }],
            pending_outputs: vec![],
            pending_tool_outputs: vec![],
            pending_tool_messages: vec![],
        };
        let event = ChatEvent::ToolRegistrySnapshot(ToolRegistrySnapshot {
            chat_id: "chat-1".into(),
            request_id: 1,
            version: Some(2),
            entries: vec![WorkspaceListEntry {
                path: "introspect.manifest.json".into(),
                kind: "file".into(),
                hash: Some(TEST_HASH.into()),
                size: None,
                mode: None,
            }],
        });
        let output = run_with_state(Some(state), event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert!(state.pending_chat_requests.is_empty());
        assert_eq!(state.tool_refs, Some(vec![TEST_HASH.into()]));
        assert_eq!(state.tool_registry_version, Some(2));

        assert_eq!(output.domain_events.len(), 1);
        assert_eq!(output.domain_events[0].schema, REQUEST_SCHEMA);
        let request: ChatRequest =
            serde_cbor::from_slice(&output.domain_events[0].value).expect("request decode");
        assert_eq!(request.chat_id, "chat-1");
        assert_eq!(request.request_id, 1);
        assert_eq!(request.message_refs, vec![String::from(TEST_HASH)]);
        assert_eq!(request.model, "gpt-mock");
        assert_eq!(request.provider, "mock");
        assert_eq!(request.max_tokens, 128);
        assert_eq!(request.tool_refs, Some(vec![String::from(TEST_HASH)]));
    }
}
