use crate::contracts::{ReasoningEffort, RunConfig};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmStepContext {
    pub correlation_id: Option<String>,
    pub message_refs: Vec<String>,
    pub temperature: Option<String>,
    pub top_p: Option<String>,
    pub tool_refs: Option<Vec<String>>,
    pub tool_choice: Option<LlmToolChoice>,
    pub stop_sequences: Option<Vec<String>>,
    pub metadata: Option<BTreeMap<String, String>>,
    pub provider_options_ref: Option<String>,
    pub response_format_ref: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SysLlmRuntimeArgs {
    pub temperature: Option<String>,
    pub top_p: Option<String>,
    pub max_tokens: Option<u64>,
    pub tool_refs: Option<Vec<String>>,
    pub tool_choice: Option<LlmToolChoice>,
    pub reasoning_effort: Option<String>,
    pub stop_sequences: Option<Vec<String>>,
    pub metadata: Option<BTreeMap<String, String>>,
    pub provider_options_ref: Option<String>,
    pub response_format_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SysLlmGenerateParams {
    pub correlation_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub message_refs: Vec<String>,
    pub runtime: SysLlmRuntimeArgs,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMappingError {
    MissingProvider,
    MissingModel,
    EmptyMessageRefs,
}

impl LlmMappingError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingProvider => "missing provider",
            Self::MissingModel => "missing model",
            Self::EmptyMessageRefs => "message_refs must not be empty",
        }
    }
}

impl core::fmt::Display for LlmMappingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::error::Error for LlmMappingError {}

pub fn materialize_llm_generate_params(
    run_config: &RunConfig,
    step: &LlmStepContext,
) -> Result<SysLlmGenerateParams, LlmMappingError> {
    let provider = run_config.provider.trim();
    if provider.is_empty() {
        return Err(LlmMappingError::MissingProvider);
    }

    let model = run_config.model.trim();
    if model.is_empty() {
        return Err(LlmMappingError::MissingModel);
    }

    if step.message_refs.is_empty() {
        return Err(LlmMappingError::EmptyMessageRefs);
    }

    Ok(SysLlmGenerateParams {
        correlation_id: step.correlation_id.clone(),
        provider: provider.into(),
        model: model.into(),
        message_refs: step.message_refs.clone(),
        runtime: SysLlmRuntimeArgs {
            temperature: step.temperature.clone(),
            top_p: step.top_p.clone(),
            max_tokens: run_config.max_tokens,
            tool_refs: step.tool_refs.clone(),
            tool_choice: step.tool_choice.clone(),
            reasoning_effort: run_config.reasoning_effort.map(reasoning_effort_text),
            stop_sequences: step.stop_sequences.clone(),
            metadata: step.metadata.clone(),
            provider_options_ref: step.provider_options_ref.clone(),
            response_format_ref: step.response_format_ref.clone(),
        },
        api_key: step.api_key.clone(),
    })
}

fn reasoning_effort_text(value: ReasoningEffort) -> String {
    match value {
        ReasoningEffort::Low => "low".into(),
        ReasoningEffort::Medium => "medium".into(),
        ReasoningEffort::High => "high".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{ReasoningEffort, RunConfig};
    use alloc::collections::BTreeMap;
    use alloc::vec;

    fn hash(seed: char) -> String {
        let mut value = String::from("sha256:");
        for _ in 0..64 {
            value.push(seed);
        }
        value
    }

    #[test]
    fn maps_run_and_step_into_sys_params() {
        let run = RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: Some(ReasoningEffort::Medium),
            max_tokens: Some(512),
        };

        let mut metadata = BTreeMap::new();
        metadata.insert("trace_id".into(), "abc-123".into());

        let step = LlmStepContext {
            correlation_id: Some("run-1-turn-1".into()),
            message_refs: vec![hash('a')],
            temperature: Some("0.2".into()),
            top_p: Some("0.9".into()),
            tool_refs: Some(vec![hash('b')]),
            tool_choice: Some(LlmToolChoice::Tool {
                name: "echo_payload".into(),
            }),
            stop_sequences: Some(vec!["DONE".into()]),
            metadata: Some(metadata),
            provider_options_ref: Some(hash('c')),
            response_format_ref: Some(hash('d')),
            api_key: Some("secret-ref".into()),
        };

        let mapped = materialize_llm_generate_params(&run, &step).expect("map params");
        assert_eq!(mapped.provider, "openai");
        assert_eq!(mapped.model, "gpt-5.2");
        assert_eq!(mapped.runtime.max_tokens, Some(512));
        assert_eq!(mapped.runtime.reasoning_effort.as_deref(), Some("medium"));

        let encoded = serde_cbor::to_vec(&mapped).expect("encode cbor");
        let decoded: aos_effects::builtins::LlmGenerateParams =
            serde_cbor::from_slice(&encoded).expect("decode core params");
        assert_eq!(decoded.provider, "openai");
        assert_eq!(decoded.model, "gpt-5.2");
        assert_eq!(decoded.runtime.max_tokens, Some(512));
        assert_eq!(decoded.runtime.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(
            decoded
                .runtime
                .provider_options_ref
                .as_ref()
                .map(|h| h.as_str()),
            Some("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
        );
        assert_eq!(
            decoded
                .runtime
                .response_format_ref
                .as_ref()
                .map(|h| h.as_str()),
            Some("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
        );
    }

    #[test]
    fn rejects_missing_provider() {
        let run = RunConfig {
            provider: "   ".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: None,
        };
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            ..LlmStepContext::default()
        };

        let err = materialize_llm_generate_params(&run, &step).expect_err("missing provider");
        assert_eq!(err, LlmMappingError::MissingProvider);
    }

    #[test]
    fn rejects_missing_model_or_messages() {
        let run_missing_model = RunConfig {
            provider: "openai".into(),
            model: "".into(),
            reasoning_effort: None,
            max_tokens: None,
        };
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            ..LlmStepContext::default()
        };
        let model_err =
            materialize_llm_generate_params(&run_missing_model, &step).expect_err("missing model");
        assert_eq!(model_err, LlmMappingError::MissingModel);

        let run = RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: None,
        };
        let message_err = materialize_llm_generate_params(&run, &LlmStepContext::default())
            .expect_err("messages");
        assert_eq!(message_err, LlmMappingError::EmptyMessageRefs);
    }

    #[test]
    fn leaves_optional_runtime_controls_unset_when_not_provided() {
        let run = RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: None,
            max_tokens: None,
        };
        let step = LlmStepContext {
            message_refs: vec![hash('f')],
            ..LlmStepContext::default()
        };

        let mapped = materialize_llm_generate_params(&run, &step).expect("map");
        let encoded = serde_cbor::to_vec(&mapped).expect("encode");
        let decoded: aos_effects::builtins::LlmGenerateParams =
            serde_cbor::from_slice(&encoded).expect("decode");

        assert_eq!(decoded.runtime.max_tokens, None);
        assert_eq!(decoded.runtime.reasoning_effort, None);
        assert_eq!(decoded.runtime.provider_options_ref, None);
        assert_eq!(decoded.runtime.response_format_ref, None);
    }
}
