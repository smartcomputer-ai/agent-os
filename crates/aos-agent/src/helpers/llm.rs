use crate::contracts::{
    ActiveWindowItem, ActiveWindowItemKind, ProviderCompatibility, ReasoningEffort, RunConfig,
    TranscriptRange,
};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use aos_air_types::HashRef;
use aos_effects::builtins::{
    LlmCompactParams, LlmCompactStrategy, LlmCountTokensParams, LlmGenerateParams,
    LlmProviderCompatibility, LlmRuntimeArgs, LlmToolChoice, LlmTranscriptRange, LlmWindowItem,
    LlmWindowItemKind, TextOrSecretRef,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmStepContext {
    pub correlation_id: Option<String>,
    pub window_items: Vec<ActiveWindowItem>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmCompactStepContext {
    pub correlation_id: Option<String>,
    pub operation_id: String,
    pub source_window_items: Vec<ActiveWindowItem>,
    pub preserve_window_items: Vec<ActiveWindowItem>,
    pub recent_tail_items: Vec<ActiveWindowItem>,
    pub source_range: Option<TranscriptRange>,
    pub strategy: LlmCompactStrategy,
    pub target_tokens: Option<u64>,
    pub provider_options_ref: Option<String>,
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LlmCountTokensStepContext {
    pub correlation_id: Option<String>,
    pub window_items: Vec<ActiveWindowItem>,
    pub tool_definitions_ref: Option<String>,
    pub response_format_ref: Option<String>,
    pub provider_options_ref: Option<String>,
    pub rendering_profile: Option<String>,
    pub candidate_plan_id: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMappingError {
    MissingProvider,
    MissingModel,
    EmptyWindowItems,
    InvalidHashRef,
}

impl LlmMappingError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingProvider => "missing provider",
            Self::MissingModel => "missing model",
            Self::EmptyWindowItems => "window_items must not be empty",
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

    if step.window_items.is_empty() {
        return Err(LlmMappingError::EmptyWindowItems);
    }

    let window_items = step
        .window_items
        .iter()
        .map(map_window_item)
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
        window_items,
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

pub fn materialize_llm_compact_params(
    run_config: &RunConfig,
    step: &LlmCompactStepContext,
) -> Result<LlmCompactParams, LlmMappingError> {
    let provider = run_config.provider.trim();
    if provider.is_empty() {
        return Err(LlmMappingError::MissingProvider);
    }

    let model = run_config.model.trim();
    if model.is_empty() {
        return Err(LlmMappingError::MissingModel);
    }

    if step.source_window_items.is_empty() {
        return Err(LlmMappingError::EmptyWindowItems);
    }

    Ok(LlmCompactParams {
        correlation_id: step.correlation_id.clone(),
        operation_id: step.operation_id.clone(),
        provider: provider.into(),
        model: model.into(),
        strategy: step.strategy.clone(),
        source_window_items: step
            .source_window_items
            .iter()
            .map(map_window_item)
            .collect::<Result<Vec<_>, _>>()?,
        preserve_window_items: step
            .preserve_window_items
            .iter()
            .map(map_window_item)
            .collect::<Result<Vec<_>, _>>()?,
        recent_tail_items: step
            .recent_tail_items
            .iter()
            .map(map_window_item)
            .collect::<Result<Vec<_>, _>>()?,
        source_range: step.source_range.as_ref().map(map_transcript_range),
        target_tokens: step.target_tokens,
        provider_options_ref: step
            .provider_options_ref
            .as_ref()
            .map(|value| parse_hash_ref(value.clone()))
            .transpose()?,
        api_key: step.api_key.clone(),
    })
}

pub fn materialize_llm_count_tokens_params(
    run_config: &RunConfig,
    step: &LlmCountTokensStepContext,
) -> Result<LlmCountTokensParams, LlmMappingError> {
    let provider = run_config.provider.trim();
    if provider.is_empty() {
        return Err(LlmMappingError::MissingProvider);
    }

    let model = run_config.model.trim();
    if model.is_empty() {
        return Err(LlmMappingError::MissingModel);
    }

    if step.window_items.is_empty() {
        return Err(LlmMappingError::EmptyWindowItems);
    }

    Ok(LlmCountTokensParams {
        correlation_id: step.correlation_id.clone(),
        provider: provider.into(),
        model: model.into(),
        window_items: step
            .window_items
            .iter()
            .map(map_window_item)
            .collect::<Result<Vec<_>, _>>()?,
        tool_definitions_ref: step
            .tool_definitions_ref
            .as_ref()
            .map(|value| parse_hash_ref(value.clone()))
            .transpose()?,
        response_format_ref: step
            .response_format_ref
            .as_ref()
            .map(|value| parse_hash_ref(value.clone()))
            .transpose()?,
        provider_options_ref: step
            .provider_options_ref
            .as_ref()
            .map(|value| parse_hash_ref(value.clone()))
            .transpose()?,
        rendering_profile: step.rendering_profile.clone(),
        candidate_plan_id: step.candidate_plan_id.clone(),
        metadata: step.metadata.clone(),
        api_key: step.api_key.clone(),
    })
}

/// Apply prompt refs from run config to a step context.
pub fn apply_prompt_refs_to_step_context(
    run_config: &RunConfig,
    mut step: LlmStepContext,
) -> LlmStepContext {
    let mut window_items = run_config
        .prompt_refs
        .clone()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(idx, ref_)| {
            ActiveWindowItem::message_ref(format!("prompt:{idx}"), ref_, None, None, None)
        })
        .collect::<Vec<_>>();
    window_items.extend(core::mem::take(&mut step.window_items));
    step.window_items = window_items;
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

pub fn map_window_item(item: &ActiveWindowItem) -> Result<LlmWindowItem, LlmMappingError> {
    Ok(LlmWindowItem {
        item_id: item.item_id.clone(),
        kind: map_window_item_kind(&item.kind),
        ref_: parse_hash_ref(item.ref_.clone())?,
        lane: item.lane.as_ref().map(|lane| format!("{lane:?}")),
        source_range: item.source_range.as_ref().map(map_transcript_range),
        source_refs: item
            .source_refs
            .iter()
            .cloned()
            .map(parse_hash_ref)
            .collect::<Result<Vec<_>, _>>()?,
        provider_compatibility: item
            .provider_compatibility
            .as_ref()
            .map(map_provider_compatibility),
        estimated_tokens: item.estimated_tokens,
        metadata: item
            .metadata
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone()))
            .collect(),
    })
}

fn map_window_item_kind(kind: &ActiveWindowItemKind) -> LlmWindowItemKind {
    match kind {
        ActiveWindowItemKind::MessageRef => LlmWindowItemKind::MessageRef,
        ActiveWindowItemKind::AosSummaryRef => LlmWindowItemKind::AosSummaryRef,
        ActiveWindowItemKind::ProviderNativeArtifactRef => {
            LlmWindowItemKind::ProviderNativeArtifactRef
        }
        ActiveWindowItemKind::ProviderRawWindowRef => LlmWindowItemKind::ProviderRawWindowRef,
        ActiveWindowItemKind::Custom { kind } => LlmWindowItemKind::Custom { kind: kind.clone() },
    }
}

pub fn active_window_item_from_llm(item: &LlmWindowItem) -> ActiveWindowItem {
    ActiveWindowItem {
        item_id: item.item_id.clone(),
        kind: active_window_item_kind_from_llm(&item.kind),
        ref_: item.ref_.to_string(),
        lane: None,
        source_range: item.source_range.as_ref().map(transcript_range_from_llm),
        source_refs: item
            .source_refs
            .iter()
            .map(|value| value.to_string())
            .collect(),
        provider_compatibility: item
            .provider_compatibility
            .as_ref()
            .map(provider_compatibility_from_llm),
        estimated_tokens: item.estimated_tokens,
        metadata: item
            .metadata
            .iter()
            .map(|(key, value)| crate::contracts::ContextMetadataEntry {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn active_window_item_kind_from_llm(kind: &LlmWindowItemKind) -> ActiveWindowItemKind {
    match kind {
        LlmWindowItemKind::MessageRef => ActiveWindowItemKind::MessageRef,
        LlmWindowItemKind::AosSummaryRef => ActiveWindowItemKind::AosSummaryRef,
        LlmWindowItemKind::ProviderNativeArtifactRef => {
            ActiveWindowItemKind::ProviderNativeArtifactRef
        }
        LlmWindowItemKind::ProviderRawWindowRef => ActiveWindowItemKind::ProviderRawWindowRef,
        LlmWindowItemKind::Custom { kind } => ActiveWindowItemKind::Custom { kind: kind.clone() },
    }
}

fn map_transcript_range(range: &TranscriptRange) -> LlmTranscriptRange {
    LlmTranscriptRange {
        start_seq: range.start_seq,
        end_seq: range.end_seq,
    }
}

fn transcript_range_from_llm(range: &LlmTranscriptRange) -> TranscriptRange {
    TranscriptRange {
        start_seq: range.start_seq,
        end_seq: range.end_seq,
    }
}

fn map_provider_compatibility(value: &ProviderCompatibility) -> LlmProviderCompatibility {
    LlmProviderCompatibility {
        provider: value.provider.clone(),
        api_kind: value.api_kind.clone(),
        model: value.model.clone(),
        model_family: value.model_family.clone(),
        artifact_type: value.artifact_type.clone(),
        opaque: value.opaque,
        encrypted: value.encrypted,
    }
}

fn provider_compatibility_from_llm(value: &LlmProviderCompatibility) -> ProviderCompatibility {
    ProviderCompatibility {
        provider: value.provider.clone(),
        api_kind: value.api_kind.clone(),
        model: value.model.clone(),
        model_family: value.model_family.clone(),
        artifact_type: value.artifact_type.clone(),
        opaque: value.opaque,
        encrypted: value.encrypted,
    }
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

    fn window_item(seed: char) -> ActiveWindowItem {
        ActiveWindowItem::message_ref(format!("test:{seed}"), hash(seed), None, None, None)
    }

    fn window_refs(items: &[LlmWindowItem]) -> Vec<HashRef> {
        items.iter().map(|item| item.ref_.clone()).collect()
    }

    fn active_refs(items: &[ActiveWindowItem]) -> Vec<String> {
        items.iter().map(|item| item.ref_.clone()).collect()
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
            window_items: vec![window_item('a')],
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
            window_items: vec![window_item('a')],
            ..LlmStepContext::default()
        };

        let mapped =
            materialize_llm_generate_params_with_prompt_refs(&run, step).expect("map params");
        assert_eq!(window_refs(&mapped.window_items), vec![hash_ref('a')]);
        assert_eq!(mapped.runtime.tool_refs, None);
    }

    #[test]
    fn explicit_tool_refs_are_preserved_when_prompt_refs_applied() {
        let run = RunConfig {
            prompt_refs: Some(vec![hash('r')]),
            ..run_config()
        };
        let step = LlmStepContext {
            window_items: vec![window_item('a')],
            tool_refs: Some(vec![hash('z')]),
            ..LlmStepContext::default()
        };

        let applied = apply_prompt_refs_to_step_context(&run, step);
        assert_eq!(applied.tool_refs, Some(vec![hash('z')]));
        assert_eq!(
            active_refs(&applied.window_items),
            vec![hash('r'), hash('a')]
        );
    }

    #[test]
    fn run_prompt_refs_prepend_to_history() {
        let run = RunConfig {
            prompt_refs: Some(vec![hash('r')]),
            ..run_config()
        };
        let step = LlmStepContext {
            window_items: vec![window_item('a')],
            ..LlmStepContext::default()
        };

        let mapped = materialize_llm_generate_params_with_prompt_refs(&run, step)
            .expect("map with direct prompt refs");
        assert_eq!(
            window_refs(&mapped.window_items),
            vec![hash_ref('r'), hash_ref('a')]
        );
    }

    #[test]
    fn rejects_missing_provider_model_or_messages() {
        let mut run = run_config();
        run.provider = " ".into();
        let step = LlmStepContext {
            window_items: vec![window_item('a')],
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
        assert_eq!(msg_err, LlmMappingError::EmptyWindowItems);
    }

    #[test]
    fn leaves_optional_runtime_controls_unset_when_not_provided() {
        let run = RunConfig {
            reasoning_effort: None,
            max_tokens: None,
            ..run_config()
        };
        let step = LlmStepContext {
            window_items: vec![window_item('f')],
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
