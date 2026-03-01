use aos_air_types::HashRef;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Shared headers map type used by HTTP receipts/params for deterministic ordering.
pub type HeaderMap = IndexMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpRequestReceipt {
    pub status: i32,
    #[serde(default)]
    pub headers: HeaderMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<HashRef>,
    pub timings: RequestTimings,
    pub adapter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestTimings {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutParams {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<HashRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobEdge {
    pub blob_ref: HashRef,
    pub refs: Vec<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobPutReceipt {
    pub blob_ref: HashRef,
    pub edge_ref: HashRef,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetParams {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobGetReceipt {
    pub blob_ref: HashRef,
    pub size: u64,
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimerSetParams {
    /// Logical-time deadline (monotonic), in nanoseconds.
    pub deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimerSetReceipt {
    /// Logical-time delivery timestamp (monotonic), in nanoseconds.
    pub delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostMount {
    pub host_path: String,
    pub guest_path: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostLocalTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<HostMount>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    pub network_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<HostLocalTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenParams {
    pub target: HostTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ttl_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<IndexMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionOpenReceipt {
    pub session_id: String,
    pub status: String,
    pub started_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineText {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInlineBytes {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobOutput {
    pub blob_ref: HashRef,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostOutput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecParams {
    pub session_id: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_patch: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostExecReceipt {
    pub exit_code: i32,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<HostOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<HostOutput>,
    pub started_at_ns: u64,
    pub ended_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalParams {
    pub session_id: String,
    pub signal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_timeout_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSessionSignalReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmRuntimeArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_refs: Option<Vec<HashRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<LlmToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmGenerateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub message_refs: Vec<HashRef>,
    pub runtime: LlmRuntimeArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmFinishReason {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_ref: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
}

pub type LlmToolCallList = Vec<LlmToolCall>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmOutputEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsageDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmGenerateReceipt {
    pub output_ref: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
    pub finish_reason: LlmFinishReason,
    pub token_usage: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_details: Option<LlmUsageDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_ref: Option<HashRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum LlmToolChoice {
    Auto,
    #[serde(rename = "None")]
    NoneChoice,
    Required,
    Tool {
        name: String,
    },
}
