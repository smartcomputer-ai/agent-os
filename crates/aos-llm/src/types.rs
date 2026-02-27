//! Core data model types (messages, content parts, requests, responses).
//!
//! Implemented in P02.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ops::{Add, AddAssign};

/// Timestamp encoded as an ISO-8601 string.
pub type Timestamp = String;

/// Who produced a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Developer,
}

/// The content kind discriminator for message parts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Image,
    Audio,
    Document,
    ToolCall,
    ToolResult,
    Thinking,
    RedactedThinking,
}

/// Content kind that allows provider-specific extensions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentKindOrString {
    Known(ContentKind),
    Other(String),
}

impl ContentKindOrString {
    fn is_text(&self) -> bool {
        match self {
            ContentKindOrString::Known(ContentKind::Text) => true,
            ContentKindOrString::Other(value) => value.eq_ignore_ascii_case("text"),
            _ => false,
        }
    }

    fn is_thinking(&self) -> bool {
        match self {
            ContentKindOrString::Known(ContentKind::Thinking)
            | ContentKindOrString::Known(ContentKind::RedactedThinking) => true,
            ContentKindOrString::Other(value) => {
                value.eq_ignore_ascii_case("thinking")
                    || value.eq_ignore_ascii_case("redacted_thinking")
            }
            _ => false,
        }
    }

    fn is_tool_call(&self) -> bool {
        match self {
            ContentKindOrString::Known(ContentKind::ToolCall) => true,
            ContentKindOrString::Other(value) => value.eq_ignore_ascii_case("tool_call"),
            _ => false,
        }
    }
}

impl From<ContentKind> for ContentKindOrString {
    fn from(value: ContentKind) -> Self {
        ContentKindOrString::Known(value)
    }
}

impl From<&str> for ContentKindOrString {
    fn from(value: &str) -> Self {
        ContentKindOrString::Other(value.to_string())
    }
}

impl From<String> for ContentKindOrString {
    fn from(value: String) -> Self {
        ContentKindOrString::Other(value)
    }
}

/// A multimodal content part in a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentPart {
    pub kind: ContentKindOrString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingData>,
}

impl ContentPart {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            kind: ContentKind::Text.into(),
            text: Some(content.into()),
            image: None,
            audio: None,
            document: None,
            tool_call: None,
            tool_result: None,
            thinking: None,
        }
    }

    pub fn image(data: ImageData) -> Self {
        Self {
            kind: ContentKind::Image.into(),
            text: None,
            image: Some(data),
            audio: None,
            document: None,
            tool_call: None,
            tool_result: None,
            thinking: None,
        }
    }

    pub fn audio(data: AudioData) -> Self {
        Self {
            kind: ContentKind::Audio.into(),
            text: None,
            image: None,
            audio: Some(data),
            document: None,
            tool_call: None,
            tool_result: None,
            thinking: None,
        }
    }

    pub fn document(data: DocumentData) -> Self {
        Self {
            kind: ContentKind::Document.into(),
            text: None,
            image: None,
            audio: None,
            document: Some(data),
            tool_call: None,
            tool_result: None,
            thinking: None,
        }
    }

    pub fn tool_call(data: ToolCallData) -> Self {
        Self {
            kind: ContentKind::ToolCall.into(),
            text: None,
            image: None,
            audio: None,
            document: None,
            tool_call: Some(data),
            tool_result: None,
            thinking: None,
        }
    }

    pub fn tool_result(data: ToolResultData) -> Self {
        Self {
            kind: ContentKind::ToolResult.into(),
            text: None,
            image: None,
            audio: None,
            document: None,
            tool_call: None,
            tool_result: Some(data),
            thinking: None,
        }
    }

    pub fn thinking(data: ThinkingData) -> Self {
        Self {
            kind: ContentKind::Thinking.into(),
            text: None,
            image: None,
            audio: None,
            document: None,
            tool_call: None,
            tool_result: None,
            thinking: Some(data),
        }
    }
}

/// The fundamental unit of conversation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentPart::text(content)],
            name: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::text(content)],
            name: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::text(content)],
            name: None,
            tool_call_id: None,
        }
    }

    pub fn developer(content: impl Into<String>) -> Self {
        Self {
            role: Role::Developer,
            content: vec![ContentPart::text(content)],
            name: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<Value>,
        is_error: bool,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        let tool_result = ToolResultData {
            tool_call_id: tool_call_id.clone(),
            content: content.into(),
            is_error,
            image_data: None,
            image_media_type: None,
        };

        Self {
            role: Role::Tool,
            content: vec![ContentPart::tool_result(tool_result)],
            name: None,
            tool_call_id: Some(tool_call_id),
        }
    }

    /// Concatenate all text content parts (empty if none).
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|part| part.kind.is_text())
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Image payload for multimodal input/output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Audio payload for multimodal input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// Document payload for multimodal input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// Model-initiated tool call data within an assistant message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallData {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    #[serde(default = "ToolCallData::default_type")]
    pub r#type: String,
}

impl ToolCallData {
    fn default_type() -> String {
        "function".to_string()
    }
}

/// Tool execution result, linked back to the tool call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultData {
    pub tool_call_id: String,
    pub content: Value,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_data: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_media_type: Option<String>,
}

/// Model reasoning/thinking content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThinkingData {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default)]
    pub redacted: bool,
}

/// Tool definition for requests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Tool choice policy for a request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolChoice {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Request input for complete() and stream().
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<Value>,
}

/// Response from a provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub model: String,
    pub provider: String,
    pub message: Message,
    pub finish_reason: FinishReason,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitInfo>,
}

impl Response {
    pub fn text(&self) -> String {
        self.message.text()
    }

    pub fn reasoning(&self) -> Option<String> {
        let mut output = String::new();
        for part in &self.message.content {
            if part.kind.is_thinking() {
                if let Some(thinking) = &part.thinking {
                    output.push_str(&thinking.text);
                }
            }
        }
        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }

    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.message
            .content
            .iter()
            .filter(|part| part.kind.is_tool_call())
            .filter_map(|part| part.tool_call.as_ref())
            .map(ToolCall::from)
            .collect()
    }
}

/// Unified finish reason with provider-specific raw detail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinishReason {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// Token usage summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl Usage {
    fn sum_optional(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0) + right.unwrap_or(0)),
        }
    }
}

impl Add for Usage {
    type Output = Usage;

    fn add(self, rhs: Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            total_tokens: self.total_tokens + rhs.total_tokens,
            reasoning_tokens: Usage::sum_optional(self.reasoning_tokens, rhs.reasoning_tokens),
            cache_read_tokens: Usage::sum_optional(self.cache_read_tokens, rhs.cache_read_tokens),
            cache_write_tokens: Usage::sum_optional(
                self.cache_write_tokens,
                rhs.cache_write_tokens,
            ),
            raw: None,
        }
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Usage) {
        *self = self.clone() + rhs;
    }
}

/// Response format configuration for structured outputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseFormat {
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<Value>,
    #[serde(default)]
    pub strict: bool,
}

/// Non-fatal warning returned by a provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Rate limit metadata from response headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateLimitInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_remaining: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_remaining: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<Timestamp>,
}

/// Canonical tool call extracted from a response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_arguments: Option<String>,
}

impl From<&ToolCallData> for ToolCall {
    fn from(value: &ToolCallData) -> Self {
        match &value.arguments {
            Value::String(raw) => ToolCall {
                id: value.id.clone(),
                name: value.name.clone(),
                arguments: Value::Object(Default::default()),
                raw_arguments: Some(raw.clone()),
            },
            other => ToolCall {
                id: value.id.clone(),
                name: value.name.clone(),
                arguments: other.clone(),
                raw_arguments: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_system_constructor_sets_role_and_text() {
        let msg = Message::system("hello");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.text(), "hello");
    }

    #[test]
    fn message_text_concatenates_text_only() {
        let msg = Message {
            role: Role::User,
            content: vec![
                ContentPart::text("a"),
                ContentPart::image(ImageData {
                    url: Some("https://example.com".to_string()),
                    data: None,
                    media_type: None,
                    detail: None,
                }),
                ContentPart::text("b"),
            ],
            name: None,
            tool_call_id: None,
        };

        assert_eq!(msg.text(), "ab");
    }

    #[test]
    fn response_text_uses_message_text() {
        let response = Response {
            id: "resp".to_string(),
            model: "model".to_string(),
            provider: "provider".to_string(),
            message: Message::assistant("hello"),
            finish_reason: FinishReason {
                reason: "stop".to_string(),
                raw: None,
            },
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
                reasoning_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                raw: None,
            },
            raw: None,
            warnings: vec![],
            rate_limit: None,
        };

        assert_eq!(response.text(), "hello");
    }

    #[test]
    fn usage_addition_sums_optional_fields() {
        let usage_a = Usage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            reasoning_tokens: Some(4),
            cache_read_tokens: None,
            cache_write_tokens: Some(1),
            raw: None,
        };
        let usage_b = Usage {
            input_tokens: 5,
            output_tokens: 6,
            total_tokens: 11,
            reasoning_tokens: Some(2),
            cache_read_tokens: Some(3),
            cache_write_tokens: None,
            raw: None,
        };

        let combined = usage_a + usage_b;
        assert_eq!(combined.input_tokens, 6);
        assert_eq!(combined.output_tokens, 8);
        assert_eq!(combined.total_tokens, 14);
        assert_eq!(combined.reasoning_tokens, Some(6));
        assert_eq!(combined.cache_read_tokens, Some(3));
        assert_eq!(combined.cache_write_tokens, Some(1));
    }

    #[test]
    fn usage_addition_keeps_optional_none() {
        let usage_a = Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            raw: None,
        };
        let usage_b = Usage {
            input_tokens: 2,
            output_tokens: 3,
            total_tokens: 5,
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            raw: None,
        };

        let combined = usage_a + usage_b;
        assert_eq!(combined.reasoning_tokens, None);
        assert_eq!(combined.cache_read_tokens, None);
        assert_eq!(combined.cache_write_tokens, None);
    }

    #[test]
    fn message_tool_result_sets_role_and_content() {
        let msg = Message::tool_result("call_1", "ok", false);
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(msg.content.len(), 1);
        let part = &msg.content[0];
        assert!(matches!(part.tool_result, Some(_)));
    }
}
