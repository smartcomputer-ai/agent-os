use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{HashRef, SecretRef};

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

impl From<String> for TextOrSecretRef {
    fn from(value: String) -> Self {
        Self::Literal(value)
    }
}

impl From<&str> for TextOrSecretRef {
    fn from(value: &str) -> Self {
        Self::Literal(value.into())
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
            Self::Literal(value) => {
                TaggedTextOrSecretRef::Literal(value.as_str()).serialize(serializer)
            }
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
            let text =
                String::from_utf8(bytes).map_err(|err| format!("literal bytes not utf8: {err}"))?;
            Ok(TextOrSecretRef::Literal(text))
        }
        serde_cbor::Value::Map(map) => {
            if let Some(serde_cbor::Value::Text(tag)) =
                map.get(&serde_cbor::Value::Text("$tag".into()))
            {
                let payload = map
                    .get(&serde_cbor::Value::Text("$value".into()))
                    .ok_or_else(|| "missing '$value' in tagged variant".to_string())?;
                return parse_tagged_text_or_secret(tag, payload);
            }
            if map.len() == 1
                && let Some((serde_cbor::Value::Text(tag), payload)) = map.iter().next()
            {
                return parse_tagged_text_or_secret(tag, payload);
            }
            Err("expected text/bytes or tagged variant for TextOrSecretRef".to_string())
        }
        other => Err(format!("unsupported value for TextOrSecretRef: {other:?}")),
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
                    .map_err(|err| format!("literal bytes not utf8: {err}"))?;
                Ok(TextOrSecretRef::Literal(text))
            }
            other => Err(format!(
                "literal payload must be text or bytes, got {other:?}"
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
        Some(other) => return Err(format!("secret alias must be text, got {other:?}")),
        None => return Err("secret payload missing alias".to_string()),
    };
    let version = match map.get(&serde_cbor::Value::Text("version".into())) {
        Some(serde_cbor::Value::Integer(version)) if *version >= 0 => *version as u64,
        Some(other) => return Err(format!("secret version must be nat, got {other:?}")),
        None => return Err("secret payload missing version".to_string()),
    };
    Ok(SecretRef { alias, version })
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmRuntimeArgs {
    #[serde(default)]
    pub temperature: Option<String>,
    #[serde(default)]
    pub top_p: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub tool_refs: Option<Vec<HashRef>>,
    #[serde(default)]
    pub tool_choice: Option<LlmToolChoice>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub provider_options_ref: Option<HashRef>,
    #[serde(default)]
    pub response_format_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmTranscriptRange {
    pub start_seq: u64,
    pub end_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum LlmWindowItemKind {
    MessageRef,
    AosSummaryRef,
    ProviderNativeArtifactRef,
    ProviderRawWindowRef,
    Custom { kind: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmProviderCompatibility {
    pub provider: String,
    pub api_kind: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_family: Option<String>,
    pub artifact_type: String,
    pub opaque: bool,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmWindowItem {
    pub item_id: String,
    pub kind: LlmWindowItemKind,
    pub ref_: HashRef,
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(default)]
    pub source_range: Option<LlmTranscriptRange>,
    #[serde(default)]
    pub source_refs: Vec<HashRef>,
    #[serde(default)]
    pub provider_compatibility: Option<LlmProviderCompatibility>,
    #[serde(default)]
    pub estimated_tokens: Option<u64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl LlmWindowItem {
    pub fn message_ref(ref_: HashRef) -> Self {
        Self {
            item_id: ref_.to_string(),
            kind: LlmWindowItemKind::MessageRef,
            ref_: ref_.clone(),
            lane: None,
            source_range: None,
            source_refs: alloc::vec![ref_],
            provider_compatibility: None,
            estimated_tokens: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn renderable_message_ref(&self, provider: &str, model: &str) -> Option<&HashRef> {
        match self.kind {
            LlmWindowItemKind::MessageRef | LlmWindowItemKind::AosSummaryRef => Some(&self.ref_),
            LlmWindowItemKind::ProviderNativeArtifactRef
            | LlmWindowItemKind::ProviderRawWindowRef => {
                if self.is_provider_compatible(provider, model) {
                    Some(&self.ref_)
                } else {
                    None
                }
            }
            LlmWindowItemKind::Custom { .. } => None,
        }
    }

    fn is_provider_compatible(&self, provider: &str, model: &str) -> bool {
        let Some(compatibility) = self.provider_compatibility.as_ref() else {
            return false;
        };
        if compatibility.provider != provider {
            return false;
        }
        if compatibility
            .model
            .as_ref()
            .is_some_and(|compatible_model| compatible_model != model)
        {
            return false;
        }
        compatibility
            .model_family
            .as_ref()
            .is_none_or(|family| model.starts_with(family))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmGenerateParams {
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub window_items: Vec<LlmWindowItem>,
    pub runtime: LlmRuntimeArgs,
    #[serde(default)]
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmFinishReason {
    pub reason: String,
    #[serde(default)]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_ref: HashRef,
    #[serde(default)]
    pub provider_call_id: Option<String>,
}

pub type LlmToolCallList = Vec<LlmToolCall>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmOutputEnvelope {
    #[serde(default)]
    pub assistant_text: Option<String>,
    #[serde(default)]
    pub tool_calls_ref: Option<HashRef>,
    #[serde(default)]
    pub reasoning_ref: Option<HashRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsageDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmGenerateReceipt {
    pub output_ref: HashRef,
    #[serde(default)]
    pub raw_output_ref: Option<HashRef>,
    #[serde(default)]
    pub provider_response_id: Option<String>,
    pub finish_reason: LlmFinishReason,
    pub token_usage: TokenUsage,
    #[serde(default)]
    pub usage_details: Option<LlmUsageDetails>,
    #[serde(default)]
    pub warnings_ref: Option<HashRef>,
    #[serde(default)]
    pub rate_limit_ref: Option<HashRef>,
    #[serde(default)]
    pub cost_cents: Option<u64>,
    pub provider_id: String,
}
