use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AdapterProviderSpec {
    pub adapter_kind: String,
}

#[derive(Debug, Clone)]
pub struct EffectAdapterConfig {
    pub effect_timeout: Duration,
    pub http: HttpAdapterConfig,
    pub llm: Option<LlmAdapterConfig>,
    pub fabric: Option<FabricAdapterConfig>,
    pub adapter_routes: HashMap<String, AdapterProviderSpec>,
}

impl Default for EffectAdapterConfig {
    fn default() -> Self {
        Self {
            effect_timeout: Duration::from_secs(30),
            http: HttpAdapterConfig::default(),
            llm: LlmAdapterConfig::from_env().ok(),
            fabric: FabricAdapterConfig::from_env(),
            adapter_routes: default_adapter_routes(),
        }
    }
}

impl EffectAdapterConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(routes) = std::env::var("AOS_ADAPTER_ROUTES") {
            apply_adapter_routes_env(&mut cfg, &routes);
        }
        cfg
    }
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
        "host.fs.read_file.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.read_file".into(),
        },
    );
    routes.insert(
        "host.fs.write_file.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.write_file".into(),
        },
    );
    routes.insert(
        "host.fs.edit_file.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.edit_file".into(),
        },
    );
    routes.insert(
        "host.fs.apply_patch.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.apply_patch".into(),
        },
    );
    routes.insert(
        "host.fs.grep.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.grep".into(),
        },
    );
    routes.insert(
        "host.fs.glob.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.glob".into(),
        },
    );
    routes.insert(
        "host.fs.stat.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.stat".into(),
        },
    );
    routes.insert(
        "host.fs.exists.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.exists".into(),
        },
    );
    routes.insert(
        "host.fs.list_dir.default".into(),
        AdapterProviderSpec {
            adapter_kind: "host.fs.list_dir".into(),
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

fn apply_adapter_routes_env(cfg: &mut EffectAdapterConfig, routes: &str) {
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

#[derive(Debug, Clone)]
pub struct HttpAdapterConfig {
    pub timeout: Duration,
    pub max_body_size: usize,
}

impl Default for HttpAdapterConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_size: 10 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FabricAdapterConfig {
    pub controller_url: String,
    pub bearer_token: Option<String>,
    pub request_timeout: Duration,
    pub exec_progress_interval: Duration,
    pub default_image: Option<String>,
    pub default_runtime_class: Option<String>,
    pub default_network_mode: Option<String>,
}

impl FabricAdapterConfig {
    pub fn from_env() -> Option<Self> {
        let controller_url = std::env::var("AOS_FABRIC_CONTROLLER_URL").ok()?;
        let controller_url = controller_url.trim().trim_end_matches('/').to_string();
        if controller_url.is_empty() {
            return None;
        }

        let bearer_token = explicit_or_file_token();
        Some(Self {
            controller_url,
            bearer_token,
            request_timeout: duration_secs_env("AOS_FABRIC_REQUEST_TIMEOUT_SECS", 300),
            exec_progress_interval: duration_secs_env("AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS", 10),
            default_image: non_empty_env("AOS_FABRIC_DEFAULT_IMAGE"),
            default_runtime_class: non_empty_env("AOS_FABRIC_DEFAULT_RUNTIME_CLASS"),
            default_network_mode: non_empty_env("AOS_FABRIC_DEFAULT_NETWORK_MODE"),
        })
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn duration_secs_env(key: &str, default_secs: u64) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(default_secs))
}

fn explicit_or_file_token() -> Option<String> {
    if let Some(token) = non_empty_env("AOS_FABRIC_BEARER_TOKEN") {
        return Some(token);
    }

    let path = non_empty_env("AOS_FABRIC_BEARER_TOKEN_FILE")?;
    std::fs::read_to_string(path)
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmApiKind {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub base_url: String,
    pub timeout: Duration,
    pub api_kind: LlmApiKind,
}

#[derive(Debug, Clone)]
pub struct LlmAdapterConfig {
    pub providers: HashMap<String, ProviderConfig>,
    pub default_provider: String,
}

impl LlmAdapterConfig {
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

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
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
    fn effect_adapter_config_default_includes_default_routes() {
        let cfg = EffectAdapterConfig::default();
        assert!(cfg.adapter_routes.contains_key("http.default"));
        assert!(cfg.adapter_routes.contains_key("llm.default"));
        assert!(cfg.adapter_routes.contains_key("host.session.open.default"));
        assert!(cfg.adapter_routes.contains_key("host.exec.default"));
        assert!(
            cfg.adapter_routes
                .contains_key("host.session.signal.default")
        );
        assert!(cfg.adapter_routes.contains_key("host.fs.read_file.default"));
        assert!(
            cfg.adapter_routes
                .contains_key("host.fs.write_file.default")
        );
        assert!(cfg.adapter_routes.contains_key("host.fs.edit_file.default"));
        assert!(
            cfg.adapter_routes
                .contains_key("host.fs.apply_patch.default")
        );
        assert!(cfg.adapter_routes.contains_key("host.fs.grep.default"));
        assert!(cfg.adapter_routes.contains_key("host.fs.glob.default"));
        assert!(cfg.adapter_routes.contains_key("host.fs.stat.default"));
        assert!(cfg.adapter_routes.contains_key("host.fs.exists.default"));
        assert!(cfg.adapter_routes.contains_key("host.fs.list_dir.default"));
        assert!(cfg.adapter_routes.contains_key("timer.default"));
        assert!(cfg.adapter_routes.contains_key("vault.put.default"));
        assert!(cfg.adapter_routes.contains_key("vault.rotate.default"));
    }

    #[test]
    fn effect_adapter_config_from_env_applies_adapter_routes_override() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::set(
            "AOS_ADAPTER_ROUTES",
            "http.custom=http.request,llm.custom=llm.generate",
        );
        let cfg = EffectAdapterConfig::from_env();
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
    fn fabric_config_from_env_requires_controller_url() {
        let _lock = env_lock().lock().unwrap();
        let _url = EnvGuard::unset("AOS_FABRIC_CONTROLLER_URL");
        assert!(FabricAdapterConfig::from_env().is_none());
    }

    #[test]
    fn fabric_config_from_env_reads_controller_and_token() {
        let _lock = env_lock().lock().unwrap();
        let _url = EnvGuard::set("AOS_FABRIC_CONTROLLER_URL", "http://127.0.0.1:8787/");
        let _token = EnvGuard::set("AOS_FABRIC_BEARER_TOKEN", " token ");
        let _timeout = EnvGuard::set("AOS_FABRIC_REQUEST_TIMEOUT_SECS", "42");
        let _progress = EnvGuard::set("AOS_FABRIC_EXEC_PROGRESS_INTERVAL_SECS", "3");
        let cfg = FabricAdapterConfig::from_env().expect("fabric config");
        assert_eq!(cfg.controller_url, "http://127.0.0.1:8787");
        assert_eq!(cfg.bearer_token.as_deref(), Some("token"));
        assert_eq!(cfg.request_timeout, Duration::from_secs(42));
        assert_eq!(cfg.exec_progress_interval, Duration::from_secs(3));
    }
}
