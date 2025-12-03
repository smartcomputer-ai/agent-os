use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub effect_timeout: Duration,
    /// Optional directory for kernel module cache; if None, kernel chooses default.
    pub module_cache_dir: Option<std::path::PathBuf>,
    /// Whether to load modules eagerly on startup.
    pub eager_module_load: bool,
    /// Allow placeholder secrets when no resolver is configured.
    pub allow_placeholder_secrets: bool,
    /// HTTP adapter configuration (always present).
    pub http: HttpAdapterConfig,
    /// LLM adapter configuration (None disables registration).
    pub llm: Option<LlmAdapterConfig>,
}

impl Default for HostConfig {
    fn default() -> Self {
        let llm = LlmAdapterConfig::from_env()
            .ok()
            .and_then(|cfg| {
                if cfg.providers.values().any(|p| p.api_key.is_some()) {
                    Some(cfg)
                } else {
                    None
                }
            });
        Self {
            effect_timeout: Duration::from_secs(30),
            module_cache_dir: None,
            eager_module_load: false,
            allow_placeholder_secrets: false,
            http: HttpAdapterConfig::default(),
            llm,
        }
    }
}

/// Configuration for the HTTP adapter.
#[derive(Debug, Clone)]
pub struct HttpAdapterConfig {
    /// Default timeout for requests.
    pub timeout: Duration,
    /// Maximum response body size in bytes.
    pub max_body_size: usize,
}

impl Default for HttpAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Per-provider configuration for LLM adapter.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub timeout: Duration,
    /// Optional API key to use when params.api_key is None (convenience for CLI).
    pub api_key: Option<String>,
}

/// Configuration for the LLM adapter.
#[derive(Debug, Clone)]
pub struct LlmAdapterConfig {
    pub providers: HashMap<String, ProviderConfig>,
    pub default_provider: String,
}

impl LlmAdapterConfig {
    /// Build provider map from environment; returns error only on malformed input.
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let api_key = std::env::var("OPENAI_API_KEY").ok();

        let mut providers = HashMap::new();
        providers.insert(
            "openai".into(),
            ProviderConfig {
                base_url,
                timeout: Duration::from_secs(120),
                api_key,
            },
        );

        Ok(Self {
            providers,
            default_provider: "openai".into(),
        })
    }
}
