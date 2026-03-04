use aos_air_types::{HashRef, SecretRef};
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
#[serde(tag = "$tag", content = "$value", rename_all = "snake_case")]
pub enum HostTarget {
    Local(HostLocalTarget),
}

impl HostTarget {
    pub fn local(local: HostLocalTarget) -> Self {
        Self::Local(local)
    }

    pub fn as_local(&self) -> &HostLocalTarget {
        match self {
            Self::Local(local) => local,
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostBlobRefInput {
    pub blob_ref: HashRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostFileContentInput {
    InlineText { inline_text: HostInlineText },
    InlineBytes { inline_bytes: HostInlineBytes },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileParams {
    pub session_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsReadFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<HostOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileParams {
    pub session_id: String,
    pub path: String,
    pub content: HostFileContentInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_parents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsWriteFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_mtime_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileParams {
    pub session_id: String,
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsEditFileReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacements: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostPatchInput {
    InlineText { inline_text: HostInlineText },
    BlobRef { blob_ref: HostBlobRefInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPatchOpsSummary {
    pub add: u64,
    pub update: u64,
    pub delete: u64,
    pub r#move: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchParams {
    pub session_id: String,
    pub patch: HostPatchInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsApplyPatchReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<HostPatchOpsSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum HostTextOutput {
    InlineText { inline_text: HostInlineText },
    Blob { blob: HostBlobOutput },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_insensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGrepReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobParams {
    pub session_id: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsGlobReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsStatReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_dir: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsParams {
    pub session_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsExistsReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostFsListDirReceipt {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<HostTextOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,
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
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextOrSecretRef {
    Literal(String),
    Secret(SecretRef),
}

impl TextOrSecretRef {
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn secret(alias: impl Into<String>, version: u64) -> Self {
        Self::Secret(SecretRef {
            alias: alias.into(),
            version,
        })
    }

    pub fn as_literal(&self) -> Option<&str> {
        match self {
            Self::Literal(value) => Some(value.as_str()),
            Self::Secret(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "$tag", content = "$value")]
enum TaggedTextOrSecretRef<'a> {
    #[serde(rename = "literal")]
    Literal(&'a str),
    #[serde(rename = "secret")]
    Secret(&'a SecretRef),
}

impl Serialize for TextOrSecretRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Literal(value) => TaggedTextOrSecretRef::Literal(value.as_str()).serialize(serializer),
            Self::Secret(value) => TaggedTextOrSecretRef::Secret(value).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TextOrSecretRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_cbor::Value::deserialize(deserializer)?;
        text_or_secret_from_value(value).map_err(serde::de::Error::custom)
    }
}

fn text_or_secret_from_value(value: serde_cbor::Value) -> Result<TextOrSecretRef, String> {
    match value {
        serde_cbor::Value::Text(text) => Ok(TextOrSecretRef::Literal(text)),
        serde_cbor::Value::Bytes(bytes) => {
            let text = String::from_utf8(bytes).map_err(|e| format!("literal bytes not utf8: {e}"))?;
            Ok(TextOrSecretRef::Literal(text))
        }
        serde_cbor::Value::Map(map) => {
            if let Some(serde_cbor::Value::Text(tag)) = map.get(&serde_cbor::Value::Text("$tag".into())) {
                let payload = map
                    .get(&serde_cbor::Value::Text("$value".into()))
                    .ok_or_else(|| "missing '$value' in tagged variant".to_string())?;
                return parse_tagged_text_or_secret(tag, payload);
            }
            if map.len() == 1 {
                if let Some((serde_cbor::Value::Text(tag), payload)) = map.iter().next() {
                    return parse_tagged_text_or_secret(tag, payload);
                }
            }
            Err("expected text/bytes or tagged variant for TextOrSecretRef".to_string())
        }
        other => Err(format!(
            "unsupported value for TextOrSecretRef: {:?}",
            other
        )),
    }
}

fn parse_tagged_text_or_secret(
    tag: &str,
    payload: &serde_cbor::Value,
) -> Result<TextOrSecretRef, String> {
    match tag {
        "literal" => match payload {
            serde_cbor::Value::Text(text) => Ok(TextOrSecretRef::Literal(text.clone())),
            serde_cbor::Value::Bytes(bytes) => {
                let text = String::from_utf8(bytes.clone())
                    .map_err(|e| format!("literal bytes not utf8: {e}"))?;
                Ok(TextOrSecretRef::Literal(text))
            }
            other => Err(format!(
                "literal payload must be text or bytes, got {:?}",
                other
            )),
        },
        "secret" => {
            let secret = parse_secret_ref_payload(payload)?;
            Ok(TextOrSecretRef::Secret(secret))
        }
        other => Err(format!("unsupported TextOrSecretRef tag '{other}'")),
    }
}

fn parse_secret_ref_payload(payload: &serde_cbor::Value) -> Result<SecretRef, String> {
    let serde_cbor::Value::Map(map) = payload else {
        return Err("secret payload must be a map".to_string());
    };
    let alias = match map.get(&serde_cbor::Value::Text("alias".into())) {
        Some(serde_cbor::Value::Text(alias)) => alias.clone(),
        Some(other) => return Err(format!("secret alias must be text, got {:?}", other)),
        None => return Err("secret payload missing alias".to_string()),
    };
    let version = match map.get(&serde_cbor::Value::Text("version".into())) {
        Some(serde_cbor::Value::Integer(version)) if *version >= 0 => *version as u64,
        Some(other) => return Err(format!("secret version must be nat, got {:?}", other)),
        None => return Err("secret payload missing version".to_string()),
    };
    Ok(SecretRef { alias, version })
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
