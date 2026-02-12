use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aos_llm::{
    Client, GenerateObjectOptions, GenerateOptions, Message, Request, Response, SDKError, Tool,
    generate, generate_object,
};
use aos_llm::{
    FinishReason, StreamEvent, StreamEventStream, StreamEventType, StreamEventTypeOrString,
};
use aos_llm::{ProviderAdapter, ToolCallData, Usage};
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use serde_json::{Value, json};

#[derive(Clone)]
struct ScriptedAdapter {
    name: String,
    responses: Arc<Mutex<Vec<Response>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl ScriptedAdapter {
    fn new(name: &str, responses: Vec<Response>) -> Self {
        Self {
            name: name.to_string(),
            responses: Arc::new(Mutex::new(responses)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen_requests(&self) -> Vec<Request> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ProviderAdapter for ScriptedAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, request: Request) -> Result<Response, SDKError> {
        self.requests.lock().expect("requests").push(request);
        let mut responses = self.responses.lock().expect("responses");
        if responses.is_empty() {
            return Err(SDKError::Configuration(aos_llm::ConfigurationError::new(
                "no scripted response available",
            )));
        }
        Ok(responses.remove(0))
    }

    async fn stream(&self, _request: Request) -> Result<StreamEventStream, SDKError> {
        let response = response_text(&self.name, "stream final", "stop");
        let events = vec![
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::StreamStart),
                delta: None,
                text_id: None,
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::TextStart),
                delta: None,
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::TextDelta),
                delta: Some("stream ".to_string()),
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::TextDelta),
                delta: Some("final".to_string()),
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::TextEnd),
                delta: None,
                text_id: Some("text_0".to_string()),
                reasoning_delta: None,
                tool_call: None,
                finish_reason: None,
                usage: None,
                response: None,
                error: None,
                raw: None,
            }),
            Ok(StreamEvent {
                event_type: StreamEventTypeOrString::Known(StreamEventType::Finish),
                delta: None,
                text_id: None,
                reasoning_delta: None,
                tool_call: None,
                finish_reason: Some(response.finish_reason.clone()),
                usage: Some(response.usage.clone()),
                response: Some(response),
                error: None,
                raw: None,
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}

fn response_tool_call(provider: &str, id: &str) -> Response {
    Response {
        id: format!("{}_tool", provider),
        model: "test-model".to_string(),
        provider: provider.to_string(),
        message: aos_llm::Message {
            role: aos_llm::Role::Assistant,
            content: vec![aos_llm::ContentPart::tool_call(ToolCallData {
                id: id.to_string(),
                name: "echo_payload".to_string(),
                arguments: json!({"value":"live"}),
                r#type: "function".to_string(),
            })],
            name: None,
            tool_call_id: None,
        },
        finish_reason: FinishReason {
            reason: "tool_calls".to_string(),
            raw: Some("tool_use".to_string()),
        },
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            raw: None,
        },
        raw: None,
        warnings: vec![],
        rate_limit: None,
    }
}

fn response_text(provider: &str, text: &str, reason: &str) -> Response {
    Response {
        id: format!("{}_text", provider),
        model: "test-model".to_string(),
        provider: provider.to_string(),
        message: aos_llm::Message::assistant(text),
        finish_reason: FinishReason {
            reason: reason.to_string(),
            raw: Some(reason.to_string()),
        },
        usage: Usage {
            input_tokens: 7,
            output_tokens: 3,
            total_tokens: 10,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            raw: None,
        },
        raw: None,
        warnings: vec![],
        rate_limit: None,
    }
}

fn client_for_adapter(adapter: Arc<dyn ProviderAdapter>, provider: &str) -> Arc<Client> {
    let mut providers: HashMap<String, Arc<dyn ProviderAdapter>> = HashMap::new();
    providers.insert(provider.to_string(), adapter);
    Arc::new(Client::new(providers, Some(provider.to_string()), vec![]))
}

#[tokio::test(flavor = "current_thread")]
async fn tool_loop_round_trips_tool_results_for_openai_and_anthropic() {
    for provider in ["openai", "anthropic"] {
        let adapter = Arc::new(ScriptedAdapter::new(
            provider,
            vec![
                response_tool_call(provider, "call_1"),
                response_text(provider, "done", "stop"),
            ],
        ));
        let client = client_for_adapter(adapter.clone(), provider);

        let tool = Tool::with_execute(
            "echo_payload",
            "Echo payload",
            json!({"type":"object"}),
            |_args| async { Ok(json!({"ok": true})) },
        );

        let mut options = GenerateOptions::new("test-model");
        options.prompt = Some("trigger tool".to_string());
        options.provider = Some(provider.to_string());
        options.client = Some(client);
        options.max_tool_rounds = 2;
        options.tools = vec![tool];

        let result = generate(options).await.expect("generate result");
        assert_eq!(result.text, "done");
        assert_eq!(result.finish_reason.reason, "stop");

        let seen = adapter.seen_requests();
        assert_eq!(seen.len(), 2, "expected two complete() calls");
        let second = &seen[1].messages;
        assert!(
            second
                .iter()
                .any(|message| message.role == aos_llm::Role::Tool),
            "expected tool result message in second request for {provider}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn generate_object_conformance_matches_schema_for_openai_and_anthropic() {
    for provider in ["openai", "anthropic"] {
        let adapter = Arc::new(ScriptedAdapter::new(
            provider,
            vec![response_text(
                provider,
                "{\"name\":\"alice\",\"age\":30}",
                "stop",
            )],
        ));
        let client = client_for_adapter(adapter, provider);

        let mut generate_options = GenerateOptions::new("test-model");
        generate_options.prompt = Some("return object".to_string());
        generate_options.provider = Some(provider.to_string());
        generate_options.client = Some(client);

        let result = generate_object(GenerateObjectOptions {
            generate: generate_options,
            schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                },
                "required": ["name", "age"]
            }),
            strict: false,
        })
        .await
        .expect("generate_object result");

        let object = result.output.expect("output object");
        assert_eq!(
            object.get("name").and_then(Value::as_str),
            Some("alice"),
            "name field mismatch for {provider}"
        );
        assert_eq!(
            object.get("age").and_then(Value::as_i64),
            Some(30),
            "age field mismatch for {provider}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_event_order_conformance_for_openai_and_anthropic() {
    for provider in ["openai", "anthropic"] {
        let adapter = Arc::new(ScriptedAdapter::new(provider, vec![]));
        let client = client_for_adapter(adapter, provider);

        let mut stream = client
            .stream(Request {
                model: "test-model".to_string(),
                messages: vec![Message::user("hello")],
                provider: Some(provider.to_string()),
                tools: None,
                tool_choice: None,
                response_format: None,
                temperature: None,
                top_p: None,
                max_tokens: Some(64),
                stop_sequences: None,
                reasoning_effort: None,
                metadata: None,
                provider_options: None,
            })
            .await
            .expect("stream");

        let mut event_types = Vec::new();
        while let Some(event) = stream.next().await {
            let event = event.expect("stream event");
            event_types.push(event.event_type);
        }

        assert_eq!(
            event_types,
            vec![
                StreamEventTypeOrString::Known(StreamEventType::StreamStart),
                StreamEventTypeOrString::Known(StreamEventType::TextStart),
                StreamEventTypeOrString::Known(StreamEventType::TextDelta),
                StreamEventTypeOrString::Known(StreamEventType::TextDelta),
                StreamEventTypeOrString::Known(StreamEventType::TextEnd),
                StreamEventTypeOrString::Known(StreamEventType::Finish),
            ],
            "event order mismatch for {provider}"
        );
    }
}
