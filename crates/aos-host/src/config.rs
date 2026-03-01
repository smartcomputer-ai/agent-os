use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AdapterProviderSpec {
    /// Concrete in-process adapter kind to execute for this logical route.
    pub adapter_kind: String,
}

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
    /// HTTP server configuration.
    pub http_server: HttpServerConfig,
    /// LLM adapter configuration (None disables registration).
    pub llm: Option<LlmAdapterConfig>,
    /// Host profile route map: adapter_id -> provider spec.
    pub adapter_routes: HashMap<String, AdapterProviderSpec>,
    /// Require explicit `manifest.effect_bindings` for all external effect kinds.
    /// When enabled, legacy kind-based fallback routing is disabled.
    pub strict_effect_bindings: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            effect_timeout: Duration::from_secs(30),
            module_cache_dir: None,
            eager_module_load: false,
            allow_placeholder_secrets: false,
            http: HttpAdapterConfig::default(),
            http_server: HttpServerConfig::default(),
            llm: LlmAdapterConfig::from_env().ok(),
            adapter_routes: default_adapter_routes(),
            strict_effect_bindings: false,
        }
    }
}

impl HostConfig {
    /// Build HostConfig using environment defaults (currently same as Default).
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(bind) = std::env::var("AOS_HTTP_BIND") {
            if let Ok(addr) = bind.parse::<SocketAddr>() {
                cfg.http_server.bind = addr;
            }
        }
        if let Ok(disable) = std::env::var("AOS_HTTP_DISABLE") {
            if matches!(disable.as_str(), "1" | "true" | "yes") {
                cfg.http_server.enabled = false;
            }
        }
        if let Ok(routes) = std::env::var("AOS_ADAPTER_ROUTES") {
            apply_adapter_routes_env(&mut cfg, &routes);
        }
        if env_truthy("AOS_STRICT_EFFECT_BINDINGS") {
            cfg.strict_effect_bindings = true;
        }
        cfg
    }
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn default_adapter_routes() -> HashMap<String, AdapterProviderSpec> {
    let mut routes = HashMap::new();
    routes.insert(
        "timer.default".into(),
        AdapterProviderSpec {
            adapter_kind: "timer.set".into(),
        },
    );
    routes.insert(
        "blob.put.default".into(),
        AdapterProviderSpec {
            adapter_kind: "blob.put".into(),
        },
    );
    routes.insert(
        "blob.get.default".into(),
        AdapterProviderSpec {
            adapter_kind: "blob.get".into(),
        },
    );
    routes.insert(
        "http.default".into(),
        AdapterProviderSpec {
            adapter_kind: "http.request".into(),
        },
    );
    routes.insert(
        "llm.default".into(),
        AdapterProviderSpec {
            adapter_kind: "llm.generate".into(),
        },
    );
    routes.insert(
        "host.session.open.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.session.open".into(),
        },
    );
    routes.insert(
        "host.exec.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.exec".into(),
        },
    );
    routes.insert(
        "host.session.signal.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.session.signal".into(),
        },
    );
    routes.insert(
        "vault.put.default".into(),
        AdapterProviderSpec {
            adapter_kind: "vault.put".into(),
        },
    );
    routes.insert(
        "vault.rotate.default".into(),
        AdapterProviderSpec {
            adapter_kind: "vault.rotate".into(),
        },
    );
    routes
}

fn apply_adapter_routes_env(cfg: &mut HostConfig, routes: &str) {
    // AOS_ADAPTER_ROUTES format: "adapter.id=adapter.kind,adapter.id2=adapter.kind2"
    for pair in routes.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((adapter_id, adapter_kind)) = pair.split_once('=') else {
            log::warn!("ignoring malformed AOS_ADAPTER_ROUTES entry: '{pair}'");
            continue;
        };
        let adapter_id = adapter_id.trim();
        let adapter_kind = adapter_kind.trim();
        if adapter_id.is_empty() || adapter_kind.is_empty() {
            log::warn!("ignoring malformed AOS_ADAPTER_ROUTES entry: '{pair}'");
            continue;
        }
        cfg.adapter_routes.insert(
            adapter_id.to_string(),
            AdapterProviderSpec {
                adapter_kind: adapter_kind.to_string(),
            },
        );
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

/// Configuration for the HTTP server.
#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub enabled: bool,
    pub bind: SocketAddr,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: SocketAddr::from(([127, 0, 0, 1], 7777)),
        }
    }
}

impl Default for HttpAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Provider API kind for the LLM adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmApiKind {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

/// Per-provider configuration for LLM adapter.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub timeout: Duration,
    pub api_kind: LlmApiKind,
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
        let openai_base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let anthropic_base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com/v1".into());

        let mut providers = HashMap::new();
        let openai_chat = ProviderConfig {
            base_url: openai_base_url.clone(),
            timeout: Duration::from_secs(120),
            api_kind: LlmApiKind::ChatCompletions,
        };
        let openai_responses = ProviderConfig {
            base_url: openai_base_url,
            timeout: Duration::from_secs(120),
            api_kind: LlmApiKind::Responses,
        };
        let anthropic_messages = ProviderConfig {
            base_url: anthropic_base_url,
            timeout: Duration::from_secs(120),
            api_kind: LlmApiKind::AnthropicMessages,
        };

        providers.insert("openai-chat".into(), openai_chat.clone());
        providers.insert("openai-responses".into(), openai_responses);
        providers.insert("anthropic".into(), anthropic_messages);
        // Back-compat alias for existing configs.
        providers.insert("openai".into(), openai_chat);

        Ok(Self {
            providers,
            default_provider: "openai-responses".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn host_config_default_includes_default_adapter_routes() {
        let cfg = HostConfig::default();
        assert!(cfg.adapter_routes.contains_key("http.default"));
        assert!(cfg.adapter_routes.contains_key("llm.default"));
        assert!(cfg.adapter_routes.contains_key("host.session.open.default"));
        assert!(cfg.adapter_routes.contains_key("host.exec.default"));
        assert!(
            cfg.adapter_routes
                .contains_key("host.session.signal.default")
        );
        assert!(cfg.adapter_routes.contains_key("timer.default"));
        assert!(cfg.adapter_routes.contains_key("vault.put.default"));
        assert!(cfg.adapter_routes.contains_key("vault.rotate.default"));
        assert!(!cfg.strict_effect_bindings);
    }

    #[test]
    fn host_config_from_env_applies_adapter_routes_override() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::set(
            "AOS_ADAPTER_ROUTES",
            "http.custom=http.request,llm.custom=llm.generate",
        );
        let cfg = HostConfig::from_env();
        assert_eq!(
            cfg.adapter_routes
                .get("http.custom")
                .map(|spec| spec.adapter_kind.as_str()),
            Some("http.request")
        );
        assert_eq!(
            cfg.adapter_routes
                .get("llm.custom")
                .map(|spec| spec.adapter_kind.as_str()),
            Some("llm.generate")
        );
    }

    #[test]
    fn host_config_from_env_enables_strict_effect_bindings() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::set("AOS_STRICT_EFFECT_BINDINGS", "true");
        let cfg = HostConfig::from_env();
        assert!(cfg.strict_effect_bindings);
    }
}
