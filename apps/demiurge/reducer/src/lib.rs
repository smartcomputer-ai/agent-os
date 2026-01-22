#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::str;
use aos_wasm_sdk::{aos_event_union, aos_reducer, aos_variant, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

const REQUEST_SCHEMA: &str = "demiurge/ChatRequest@1";

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
    tool_choice: Option<LlmToolChoice>,
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
    expected_ref: String,
}

aos_event_union! {
    #[derive(Debug, Clone, Serialize)]
    enum ChatEvent {
        ChatCreated(ChatCreated),
        UserMessage(UserMessage),
        ChatResult(ChatResult),
        ToolResult(ToolResult),
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
    ctx.state.tool_refs = tool_refs.clone();
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

    let intent_value = ChatRequest {
        chat_id,
        request_id,
        message_refs,
        model,
        provider,
        max_tokens,
        tool_refs,
        tool_choice,
    };
    let key = request_id.to_be_bytes();
    ctx.intent(REQUEST_SCHEMA)
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
    let mut has_assistant = false;
    for message in &ctx.state.messages {
        if message.request_id != result.request_id {
            continue;
        }
        match message.role {
            ChatRole::User => has_user = true,
            ChatRole::Assistant => has_assistant = true,
        }
        if has_user && has_assistant {
            break;
        }
    }

    if !has_user || has_assistant {
        return;
    }

    ctx.state.messages.push(ChatMessage {
        request_id: result.request_id,
        role: ChatRole::Assistant,
        text: None,
        message_ref: Some(result.output_ref),
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
            output_ref: result.output_ref.clone(),
        });
        let params = BlobGetParams {
            blob_ref: result.output_ref,
        };
        ctx.effects().emit_raw("blob.get", &params, Some("blob"));
    }
}

fn emit_chat_request(ctx: &mut ReducerCtx<ChatState, ()>, chat_id: String, request_id: u64) {
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

    let intent_value = ChatRequest {
        chat_id,
        request_id,
        message_refs,
        model,
        provider,
        max_tokens,
        tool_refs: ctx.state.tool_refs.clone(),
        tool_choice: ctx.state.tool_choice.clone(),
    };
    let key = request_id.to_be_bytes();
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

fn handle_blob_put_result(ctx: &mut ReducerCtx<ChatState, ()>, result: BlobPutResult) {
    if result.status != "ok" {
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

    emit_chat_request(ctx, pending.chat_id, pending.request_id);
}

fn handle_blob_get_result(ctx: &mut ReducerCtx<ChatState, ()>, result: BlobGetResult) {
    if result.status != "ok" {
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
        expected_ref: message_ref.clone(),
    });

    let params = BlobPutParams {
        blob_ref: message_ref,
        bytes: message_bytes,
    };
    ctx.effects().emit_raw("blob.put", &params, Some("blob"));
}

fn extract_tool_calls_from_output(bytes: &[u8]) -> Vec<ToolCall> {
    let mut parser = JsonToolCallParser::new(bytes);
    parser.parse();
    parser.calls
}

fn parse_tool_call_params(name: &str, arguments_json: &str) -> Option<ToolCallParams> {
    let args: JsonValue = serde_json::from_str(arguments_json).ok()?;
    let obj = args.as_object()?;
    match name {
        "introspect.manifest" => {
            let consistency = obj
                .get("consistency")
                .and_then(|value| value.as_str())
                .unwrap_or("head")
                .to_string();
            Some(ToolCallParams::IntrospectManifest { consistency })
        }
        "workspace.read" | "workspace.read_bytes" => {
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

fn decode_tool_output(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).to_string(),
    }
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

#[derive(Default)]
struct ObjectCtx {
    expecting_key: bool,
    current_key: Option<String>,
    type_value: Option<String>,
    name: Option<String>,
    arguments_json: Option<String>,
    call_id: Option<String>,
    id: Option<String>,
}

impl ObjectCtx {
    fn new() -> Self {
        Self {
            expecting_key: true,
            ..Self::default()
        }
    }

    fn into_tool_call(self) -> Option<ToolCall> {
        if self.type_value.as_deref() != Some("function_call") {
            return None;
        }
        let name = self.name?;
        if name.is_empty() {
            return None;
        }
        let id = self.call_id.or(self.id)?;
        if id.is_empty() {
            return None;
        }
        let arguments_json = self.arguments_json.unwrap_or_else(|| "{}".into());
        Some(ToolCall {
            id,
            name,
            arguments_json,
        })
    }
}

enum Container {
    Object(ObjectCtx),
    Array,
}

struct JsonToolCallParser<'a> {
    bytes: &'a [u8],
    idx: usize,
    stack: Vec<Container>,
    calls: Vec<ToolCall>,
}

impl<'a> JsonToolCallParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            idx: 0,
            stack: Vec::new(),
            calls: Vec::new(),
        }
    }

    fn parse(&mut self) {
        while self.idx < self.bytes.len() {
            self.skip_ws();
            if self.idx >= self.bytes.len() {
                break;
            }

            if self.try_capture_arguments_object() {
                continue;
            }

            match self.bytes[self.idx] {
                b'{' => {
                    self.stack.push(Container::Object(ObjectCtx::new()));
                    self.idx += 1;
                }
                b'}' => {
                    if let Some(Container::Object(obj)) = self.stack.pop() {
                        if let Some(call) = obj.into_tool_call() {
                            self.calls.push(call);
                        }
                    }
                    self.idx += 1;
                    self.finish_value();
                }
                b'[' => {
                    self.stack.push(Container::Array);
                    self.idx += 1;
                }
                b']' => {
                    self.stack.pop();
                    self.idx += 1;
                    self.finish_value();
                }
                b'"' => {
                    if let Some((value, next)) = parse_json_string(self.bytes, self.idx) {
                        self.idx = next;
                        if let Some(obj) = self.current_object_mut() {
                            if obj.expecting_key {
                                obj.current_key = Some(value);
                                obj.expecting_key = false;
                            } else {
                                self.record_string_value(obj, value);
                                self.finish_value();
                            }
                        } else {
                            self.finish_value();
                        }
                    } else {
                        self.idx += 1;
                    }
                }
                b':' => {
                    self.idx += 1;
                }
                b',' => {
                    self.idx += 1;
                    if let Some(obj) = self.current_object_mut() {
                        obj.expecting_key = true;
                        obj.current_key = None;
                    }
                }
                _ => {
                    if let Some(obj) = self.current_object_mut() {
                        if !obj.expecting_key {
                            let start = self.idx;
                            let end = consume_nonstring_value(self.bytes, start);
                            if let Some(key) = obj.current_key.as_deref() {
                                if key == "arguments" {
                                    if let Ok(raw) = str::from_utf8(&self.bytes[start..end]) {
                                        obj.arguments_json = Some(String::from(raw));
                                    }
                                }
                            }
                            self.idx = end;
                            self.finish_value();
                            continue;
                        }
                    }
                    self.idx += 1;
                }
            }
        }
    }

    fn current_object_mut(&mut self) -> Option<&mut ObjectCtx> {
        match self.stack.last_mut() {
            Some(Container::Object(obj)) => Some(obj),
            _ => None,
        }
    }

    fn finish_value(&mut self) {
        if let Some(obj) = self.current_object_mut() {
            if !obj.expecting_key {
                obj.expecting_key = true;
                obj.current_key = None;
            }
        }
    }

    fn record_string_value(&mut self, obj: &mut ObjectCtx, value: String) {
        if let Some(key) = obj.current_key.as_deref() {
            match key {
                "type" => obj.type_value = Some(value),
                "name" => obj.name = Some(value),
                "arguments" => obj.arguments_json = Some(value),
                "call_id" => obj.call_id = Some(value),
                "id" => obj.id = Some(value),
                _ => {}
            }
        }
    }

    fn try_capture_arguments_object(&mut self) -> bool {
        let Some(obj) = self.current_object_mut() else {
            return false;
        };
        if obj.expecting_key {
            return false;
        }
        let Some(key) = obj.current_key.as_deref() else {
            return false;
        };
        if key != "arguments" {
            return false;
        }
        let byte = self.bytes[self.idx];
        if byte != b'{' && byte != b'[' {
            return false;
        }
        let Some((raw, end)) = extract_raw_json(self.bytes, self.idx) else {
            return false;
        };
        obj.arguments_json = Some(raw);
        self.idx = end;
        self.finish_value();
        true
    }

    fn skip_ws(&mut self) {
        while self.idx < self.bytes.len() && is_ws(self.bytes[self.idx]) {
            self.idx += 1;
        }
    }
}

fn consume_nonstring_value(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t' => break,
            _ => i += 1,
        }
    }
    i
}

fn extract_raw_json(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    i += 1;
                    let raw = String::from(str::from_utf8(&bytes[start..i]).ok()?);
                    return Some((raw, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_json_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return None;
                }
                match bytes[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0c'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if i + 4 >= bytes.len() {
                            return None;
                        }
                        let mut code: u16 = 0;
                        for _ in 0..4 {
                            i += 1;
                            code = (code << 4) | hex_val(bytes[i])?;
                        }
                        if let Some(ch) = core::char::from_u32(code as u32) {
                            out.push(ch);
                        }
                    }
                    _ => {}
                }
            }
            _ => out.push(b as char),
        }
        i += 1;
    }
    None
}

fn hex_val(byte: u8) -> Option<u16> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u16),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u16),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u16),
        _ => None,
    }
}

fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t')
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
        let input = ReducerInput {
            version: ABI_VERSION,
            state: state.map(|s| serde_cbor::to_vec(&s).expect("state bytes")),
            event: DomainEvent::new(
                "demiurge/ChatEvent@1",
                serde_cbor::to_vec(&event).expect("event bytes"),
            ),
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
            tool_choice: None,
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
        assert_eq!(output.domain_events[0].schema, REQUEST_SCHEMA);
        let request: ChatRequest =
            serde_cbor::from_slice(&output.domain_events[0].value).expect("request decode");
        assert_eq!(request.chat_id, "chat-1");
        assert_eq!(request.request_id, 1);
        assert_eq!(request.message_refs, vec![String::from(TEST_HASH)]);
        assert_eq!(request.model, "gpt-mock");
        assert_eq!(request.provider, "mock");
        assert_eq!(request.max_tokens, 128);
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
            tool_choice: None,
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
            tool_choice: None,
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
}
