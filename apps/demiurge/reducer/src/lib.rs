#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use aos_wasm_sdk::{aos_reducer, aos_variant, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

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

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum ChatEvent {
        ChatCreated(ChatCreated),
        UserMessage(UserMessage),
        ChatResult(ChatResult),
        ToolResult(ToolResult),
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
    name: String,
    arguments_json: String,
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

    if let Some(tool_calls) = result.tool_calls {
        for call in tool_calls {
            let intent_value = ToolCallRequested {
                chat_id: result.chat_id.clone(),
                request_id: result.request_id,
                tool_call_id: call.id,
                name: call.name,
                arguments_json: call.arguments_json,
            };
            let key = result.request_id.to_be_bytes();
            ctx.intent("demiurge/ToolCallRequested@1")
                .key_bytes(&key)
                .payload(&intent_value)
                .send();
        }
    }
}

fn handle_tool_result(ctx: &mut ReducerCtx<ChatState, ()>, result: ToolResult) {
    if ctx.state.title.is_none() || ctx.state.created_at_ms.is_none() {
        return;
    }

    ctx.state.messages.push(ChatMessage {
        request_id: result.request_id,
        role: ChatRole::Assistant,
        text: None,
        message_ref: Some(result.result_ref),
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

    let (Some(model), Some(provider), Some(max_tokens)) = (
        ctx.state.model.clone(),
        ctx.state.provider.clone(),
        ctx.state.max_tokens,
    ) else {
        return;
    };

    let intent_value = ChatRequest {
        chat_id: result.chat_id,
        request_id: result.request_id,
        message_refs,
        model,
        provider,
        max_tokens,
        tool_refs: ctx.state.tool_refs.clone(),
        tool_choice: ctx.state.tool_choice.clone(),
    };
    let key = result.request_id.to_be_bytes();
    ctx.intent(REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
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
