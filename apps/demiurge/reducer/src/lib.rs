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
}

aos_variant! {
    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    enum ChatRole {
        User,
        Assistant,
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
    input_ref: Option<String>,
    output_ref: Option<String>,
    token_usage: Option<TokenUsage>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum ChatEvent {
        UserMessage(UserMessage),
        ChatResult(ChatResult),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserMessage {
    request_id: u64,
    text: String,
    input_ref: String,
    model: String,
    provider: String,
    max_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatRequest {
    request_id: u64,
    input_ref: String,
    model: String,
    provider: String,
    max_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatResult {
    request_id: u64,
    output_ref: String,
    token_usage: TokenUsage,
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
            ChatEvent::UserMessage(message) => handle_user_message(ctx, message),
            ChatEvent::ChatResult(result) => handle_chat_result(ctx, result),
        }
        Ok(())
    }
}

fn handle_user_message(ctx: &mut ReducerCtx<ChatState, ()>, message: UserMessage) {
    let UserMessage {
        request_id,
        text,
        input_ref,
        model,
        provider,
        max_tokens,
    } = message;

    if request_id <= ctx.state.last_request_id {
        return;
    }

    ctx.state.last_request_id = request_id;
    ctx.state.messages.push(ChatMessage {
        request_id,
        role: ChatRole::User,
        text: Some(text),
        input_ref: Some(input_ref.clone()),
        output_ref: None,
        token_usage: None,
    });

    let intent_value = ChatRequest {
        request_id,
        input_ref,
        model,
        provider,
        max_tokens,
    };
    let key = request_id.to_be_bytes();
    ctx.intent(REQUEST_SCHEMA)
        .key_bytes(&key)
        .payload(&intent_value)
        .send();
}

fn handle_chat_result(ctx: &mut ReducerCtx<ChatState, ()>, result: ChatResult) {
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
        input_ref: None,
        output_ref: Some(result.output_ref),
        token_usage: Some(result.token_usage),
    });
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
    fn user_message_appends_and_emits_request() {
        let event = ChatEvent::UserMessage(UserMessage {
            request_id: 1,
            text: "hello".into(),
            input_ref: TEST_HASH.into(),
            model: "gpt-mock".into(),
            provider: "mock".into(),
            max_tokens: 128,
        });
        let output = run_with_state(None, event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(state.last_request_id, 1);
        assert_eq!(state.messages.len(), 1);
        let message = &state.messages[0];
        assert!(matches!(message.role, ChatRole::User));
        assert_eq!(message.text.as_deref(), Some("hello"));
        assert_eq!(message.input_ref.as_deref(), Some(TEST_HASH));
        assert!(message.output_ref.is_none());

        assert_eq!(output.domain_events.len(), 1);
        assert_eq!(output.domain_events[0].schema, REQUEST_SCHEMA);
        let request: ChatRequest =
            serde_cbor::from_slice(&output.domain_events[0].value).expect("request decode");
        assert_eq!(request.request_id, 1);
        assert_eq!(request.input_ref, TEST_HASH);
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
                input_ref: Some(TEST_HASH.into()),
                output_ref: None,
                token_usage: None,
            }],
            last_request_id: 2,
        };
        let event = ChatEvent::UserMessage(UserMessage {
            request_id: 1,
            text: "late".into(),
            input_ref: TEST_HASH.into(),
            model: "gpt-mock".into(),
            provider: "mock".into(),
            max_tokens: 64,
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
                input_ref: Some(TEST_HASH.into()),
                output_ref: None,
                token_usage: None,
            }],
            last_request_id: 1,
        };
        let event = ChatEvent::ChatResult(ChatResult {
            request_id: 1,
            output_ref: TEST_HASH.into(),
            token_usage: TokenUsage {
                prompt: 10,
                completion: 20,
            },
        });
        let output = run_with_state(Some(state), event);
        let state: ChatState =
            serde_cbor::from_slice(output.state.as_ref().expect("state")).expect("state decode");

        assert_eq!(state.messages.len(), 2);
        let message = &state.messages[1];
        assert!(matches!(message.role, ChatRole::Assistant));
        assert_eq!(message.output_ref.as_deref(), Some(TEST_HASH));
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
