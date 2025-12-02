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
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            effect_timeout: Duration::from_secs(30),
            module_cache_dir: None,
            eager_module_load: false,
            allow_placeholder_secrets: false,
        }
    }
}
