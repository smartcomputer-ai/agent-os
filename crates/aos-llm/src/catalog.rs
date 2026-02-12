//! Model catalog helpers.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Advisory model metadata used for lookup and capability-based selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub context_window: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_reasoning: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cost_per_million: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cost_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

static MODEL_CATALOG: OnceLock<Vec<ModelInfo>> = OnceLock::new();

fn catalog() -> &'static [ModelInfo] {
    MODEL_CATALOG
        .get_or_init(|| {
            serde_json::from_str(include_str!("catalog_models.json"))
                .expect("catalog_models.json must be valid")
        })
        .as_slice()
}

/// Returns catalog information for a concrete model ID.
pub fn get_model_info(model_id: &str) -> Option<ModelInfo> {
    catalog().iter().find(|model| model.id == model_id).cloned()
}

/// Returns all known models, optionally filtered by provider.
pub fn list_models(provider: Option<&str>) -> Vec<ModelInfo> {
    match provider {
        None => catalog().to_vec(),
        Some(provider) => catalog()
            .iter()
            .filter(|model| model.provider.eq_ignore_ascii_case(provider))
            .cloned()
            .collect(),
    }
}

/// Returns the latest model for a provider, optionally filtered by capability.
///
/// The catalog ordering is authoritative: newer/preferred models appear first.
pub fn get_latest_model(provider: &str, capability: Option<&str>) -> Option<ModelInfo> {
    list_models(Some(provider))
        .into_iter()
        .find(|model| supports_capability(model, capability))
}

fn supports_capability(model: &ModelInfo, capability: Option<&str>) -> bool {
    match capability.map(str::trim).map(str::to_ascii_lowercase) {
        None => true,
        Some(value) if value.is_empty() => true,
        Some(value) if matches!(value.as_str(), "tools" | "tool") => model.supports_tools,
        Some(value) if matches!(value.as_str(), "vision" | "image" | "images") => {
            model.supports_vision
        }
        Some(value) if matches!(value.as_str(), "reasoning" | "thinking") => {
            model.supports_reasoning
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_models_returns_all_entries_in_catalog_order() {
        let models = list_models(None);
        assert_eq!(models.len(), 7);
        assert_eq!(models[0].id, "claude-opus-4-6");
        assert_eq!(models[6].id, "gemini-3-flash-preview");
    }

    #[test]
    fn list_models_filters_provider_case_insensitive() {
        let models = list_models(Some("OpenAI"));
        assert_eq!(models.len(), 3);
        assert!(models.iter().all(|model| model.provider == "openai"));
    }

    #[test]
    fn get_model_info_returns_model_or_none() {
        let known = get_model_info("gpt-5.2-codex").expect("model should exist");
        assert_eq!(known.provider, "openai");
        assert!(get_model_info("unknown-model").is_none());
    }

    #[test]
    fn get_latest_model_returns_first_provider_match() {
        let anthropic = get_latest_model("anthropic", None).expect("model");
        let openai = get_latest_model("openai", None).expect("model");
        let gemini = get_latest_model("gemini", None).expect("model");
        assert_eq!(anthropic.id, "claude-opus-4-6");
        assert_eq!(openai.id, "gpt-5.2");
        assert_eq!(gemini.id, "gemini-3-pro-preview");
    }

    #[test]
    fn get_latest_model_filters_capability_and_handles_unknown_capability() {
        let model = get_latest_model("openai", Some("reasoning")).expect("model");
        assert_eq!(model.id, "gpt-5.2");
        assert!(get_latest_model("openai", Some("nonexistent_capability")).is_none());
    }
}
