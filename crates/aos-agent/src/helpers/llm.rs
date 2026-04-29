use crate::contracts::{ReasoningEffort, RunConfig};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use aos_air_types::HashRef;
use aos_effects::builtins::{LlmGenerateParams, LlmRuntimeArgs, LlmToolChoice, TextOrSecretRef};
use serde::{Deserialize, Serialize};

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
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMappingError {
    MissingProvider,
    MissingModel,
    EmptyMessageRefs,
    InvalidHashRef,
}

impl LlmMappingError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingProvider => "missing provider",
            Self::MissingModel => "missing model",
            Self::EmptyMessageRefs => "message_refs must not be empty",
            Self::InvalidHashRef => "invalid hash ref",
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
) -> Result<LlmGenerateParams, LlmMappingError> {
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

    let message_refs = step
        .message_refs
        .iter()
        .cloned()
        .map(parse_hash_ref)
        .collect::<Result<Vec<_>, _>>()?;
    let tool_refs = step
        .tool_refs
        .as_ref()
        .map(|values| {
            values
                .iter()
                .cloned()
                .map(parse_hash_ref)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let provider_options_ref = step
        .provider_options_ref
        .as_ref()
        .map(|value| parse_hash_ref(value.clone()))
        .transpose()?;
    let response_format_ref = step
        .response_format_ref
        .as_ref()
        .map(|value| parse_hash_ref(value.clone()))
        .transpose()?;

    Ok(LlmGenerateParams {
        correlation_id: step.correlation_id.clone(),
        provider: provider.into(),
        model: model.into(),
        message_refs,
        runtime: LlmRuntimeArgs {
            temperature: step.temperature.clone(),
            top_p: step.top_p.clone(),
            max_tokens: run_config.max_tokens,
            tool_refs,
            tool_choice: step.tool_choice.clone(),
            reasoning_effort: run_config.reasoning_effort.map(reasoning_effort_text),
            stop_sequences: step.stop_sequences.clone(),
            metadata: step.metadata.clone(),
            provider_options_ref,
            response_format_ref,
        },
        api_key: step.api_key.clone(),
    })
}

/// Apply prompt refs from run config to a step context.
pub fn apply_prompt_refs_to_step_context(
    run_config: &RunConfig,
    mut step: LlmStepContext,
) -> LlmStepContext {
    let mut message_refs = run_config.prompt_refs.clone().unwrap_or_default();
    message_refs.extend(core::mem::take(&mut step.message_refs));
    step.message_refs = message_refs;
    step
}

/// Convenience wrapper that applies prompt refs and then maps to
/// `sys/llm.generate` params.
pub fn materialize_llm_generate_params_with_prompt_refs(
    run_config: &RunConfig,
    step: LlmStepContext,
) -> Result<LlmGenerateParams, LlmMappingError> {
    let step = apply_prompt_refs_to_step_context(run_config, step);
    materialize_llm_generate_params(run_config, &step)
}

fn parse_hash_ref(value: String) -> Result<HashRef, LlmMappingError> {
    HashRef::new(value).map_err(|_| LlmMappingError::InvalidHashRef)
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
    use aos_air_types::HashRef;

    fn hash(seed: char) -> String {
        let mut value = String::from("sha256:");
        let nibble = b"0123456789abcdef"[seed as usize % 16] as char;
        for _ in 0..64 {
            value.push(nibble);
        }
        value
    }

    fn hash_ref(seed: char) -> HashRef {
        HashRef::new(hash(seed)).expect("valid hash ref")
    }

    fn run_config() -> RunConfig {
        RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            reasoning_effort: Some(ReasoningEffort::Medium),
            max_tokens: Some(512),
            prompt_refs: None,
            tool_profile: None,
            tool_enable: None,
            tool_disable: None,
            tool_force: None,
            host_session_open: None,
        }
    }

    #[test]
    fn maps_run_and_step_into_sys_params() {
        let run = run_config();

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
            api_key: Some(TextOrSecretRef::literal("secret-ref")),
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
            decoded.api_key,
            Some(aos_effects::builtins::TextOrSecretRef::literal(
                "secret-ref"
            ))
        );
        assert_eq!(
            decoded
                .runtime
                .provider_options_ref
                .as_ref()
                .map(|h| h.as_str()),
            Some(hash('c').as_str())
        );
        assert_eq!(
            decoded
                .runtime
                .response_format_ref
                .as_ref()
                .map(|h| h.as_str()),
            Some(hash('d').as_str())
        );
    }

    #[test]
    fn applies_run_prompt_refs_to_step_context() {
        let run = run_config();
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            ..LlmStepContext::default()
        };

        let mapped =
            materialize_llm_generate_params_with_prompt_refs(&run, step).expect("map params");
        assert_eq!(mapped.message_refs, vec![hash_ref('a')]);
        assert_eq!(mapped.runtime.tool_refs, None);
    }

    #[test]
    fn explicit_tool_refs_are_preserved_when_prompt_refs_applied() {
        let run = RunConfig {
            prompt_refs: Some(vec![hash('r')]),
            ..run_config()
        };
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            tool_refs: Some(vec![hash('z')]),
            ..LlmStepContext::default()
        };

        let applied = apply_prompt_refs_to_step_context(&run, step);
        assert_eq!(applied.tool_refs, Some(vec![hash('z')]));
        assert_eq!(applied.message_refs, vec![hash('r'), hash('a')]);
    }

    #[test]
    fn run_prompt_refs_prepend_to_history() {
        let run = RunConfig {
            prompt_refs: Some(vec![hash('r')]),
            ..run_config()
        };
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            ..LlmStepContext::default()
        };

        let mapped = materialize_llm_generate_params_with_prompt_refs(&run, step)
            .expect("map with direct prompt refs");
        assert_eq!(mapped.message_refs, vec![hash_ref('r'), hash_ref('a')]);
    }

    #[test]
    fn rejects_missing_provider_model_or_messages() {
        let mut run = run_config();
        run.provider = " ".into();
        let step = LlmStepContext {
            message_refs: vec![hash('a')],
            ..LlmStepContext::default()
        };

        let provider_err = materialize_llm_generate_params(&run, &step).expect_err("provider");
        assert_eq!(provider_err, LlmMappingError::MissingProvider);

        run.provider = "openai".into();
        run.model.clear();
        let model_err = materialize_llm_generate_params(&run, &step).expect_err("model");
        assert_eq!(model_err, LlmMappingError::MissingModel);

        let msg_err = materialize_llm_generate_params(&run_config(), &LlmStepContext::default())
            .expect_err("messages");
        assert_eq!(msg_err, LlmMappingError::EmptyMessageRefs);
    }

    #[test]
    fn leaves_optional_runtime_controls_unset_when_not_provided() {
        let run = RunConfig {
            reasoning_effort: None,
            max_tokens: None,
            ..run_config()
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
