//! Streaming event types and helpers.
//!
//! Implemented in P02 and P04.

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::errors::SDKError;
use crate::types::{FinishReason, Response, ToolCall, Usage};

/// Stream of unified events returned by provider adapters.
pub type StreamEventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, SDKError>> + Send>>;

/// Stream event type discriminator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventType {
    StreamStart,
    TextStart,
    TextDelta,
    TextEnd,
    ReasoningStart,
    ReasoningDelta,
    ReasoningEnd,
    ToolCallStart,
    ToolCallDelta,
    ToolCallEnd,
    Finish,
    Error,
    ProviderEvent,
}

/// Stream event type with provider-specific extensions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamEventTypeOrString {
    Known(StreamEventType),
    Other(String),
}

impl From<StreamEventType> for StreamEventTypeOrString {
    fn from(value: StreamEventType) -> Self {
        StreamEventTypeOrString::Known(value)
    }
}

impl From<&str> for StreamEventTypeOrString {
    fn from(value: &str) -> Self {
        StreamEventTypeOrString::Other(value.to_string())
    }
}

impl From<String> for StreamEventTypeOrString {
    fn from(value: String) -> Self {
        StreamEventTypeOrString::Other(value)
    }
}

/// Unified stream event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: StreamEventTypeOrString,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_delta: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCall>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SDKError>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl StreamEvent {
    pub fn error(error: SDKError) -> Self {
        Self {
            event_type: StreamEventTypeOrString::Known(StreamEventType::Error),
            delta: None,
            text_id: None,
            reasoning_delta: None,
            tool_call: None,
            finish_reason: None,
            usage: None,
            response: None,
            error: Some(error),
            raw: None,
        }
    }
}
