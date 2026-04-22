use aos_kernel::{KernelConfig, world::DEFAULT_CELL_CACHE_SIZE};

#[derive(Debug, Clone)]
pub struct WorldConfig {
    pub module_cache_dir: Option<std::path::PathBuf>,
    pub eager_module_load: bool,
    pub allow_placeholder_secrets: bool,
    pub strict_effect_routes: bool,
    pub cell_cache_size: usize,
    pub forced_replay_seed_height: Option<u64>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            module_cache_dir: None,
            eager_module_load: false,
            allow_placeholder_secrets: false,
            strict_effect_routes: false,
            cell_cache_size: DEFAULT_CELL_CACHE_SIZE,
            forced_replay_seed_height: None,
        }
    }
}

impl WorldConfig {
    pub fn from_env_with_fallback_module_cache_dir(
        fallback_module_cache_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self::from_env_values(
            fallback_module_cache_dir,
            std::env::var("AOS_MODULE_CACHE_DIR").ok(),
            std::env::var("AOS_STRICT_EFFECT_ROUTES")
                .ok()
                .or_else(|| std::env::var("AOS_STRICT_OP_ROUTES").ok()),
            std::env::var("AOS_CELL_CACHE_SIZE").ok(),
            std::env::var("AOS_NODE_REPLAY_SEED_HEIGHT").ok(),
        )
    }

    fn from_env_values(
        fallback_module_cache_dir: Option<std::path::PathBuf>,
        module_cache_dir: Option<String>,
        strict_effect_routes: Option<String>,
        cell_cache_size: Option<String>,
        forced_replay_seed_height: Option<String>,
    ) -> Self {
        let mut cfg = Self::default();
        if let Some(raw) = module_cache_dir {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                cfg.module_cache_dir = Some(std::path::PathBuf::from(trimmed));
            }
        } else {
            cfg.module_cache_dir = fallback_module_cache_dir;
        }
        if matches!(
            strict_effect_routes
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Some("1" | "true" | "yes" | "on")
        ) {
            cfg.strict_effect_routes = true;
        }
        if let Some(raw) = cell_cache_size
            && let Ok(parsed) = raw.trim().parse::<usize>()
        {
            cfg.cell_cache_size = parsed.max(1);
        }
        if let Some(raw) = forced_replay_seed_height
            && let Ok(parsed) = raw.trim().parse::<u64>()
        {
            cfg.forced_replay_seed_height = Some(parsed);
        }
        cfg
    }

    pub fn apply_kernel_defaults(&self, mut kernel_config: KernelConfig) -> KernelConfig {
        let defaults = KernelConfig::default();
        if kernel_config.module_cache_dir == defaults.module_cache_dir {
            kernel_config.module_cache_dir = self.module_cache_dir.clone();
        }
        if kernel_config.eager_module_load == defaults.eager_module_load {
            kernel_config.eager_module_load = self.eager_module_load;
        }
        if kernel_config.allow_placeholder_secrets == defaults.allow_placeholder_secrets {
            kernel_config.allow_placeholder_secrets = self.allow_placeholder_secrets;
        }
        if kernel_config.cell_cache_size == defaults.cell_cache_size {
            kernel_config.cell_cache_size = self.cell_cache_size.max(1);
        }
        kernel_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_config_from_env_enables_strict_effect_routes() {
        let cfg = WorldConfig::from_env_values(None, None, Some("true".into()), None, None);
        assert!(cfg.strict_effect_routes);
    }

    #[test]
    fn world_config_from_env_reads_module_cache_dir() {
        let cfg = WorldConfig::from_env_values(
            None,
            Some("/tmp/aos-test-cache".into()),
            None,
            None,
            None,
        );
        assert_eq!(
            cfg.module_cache_dir,
            Some(std::path::PathBuf::from("/tmp/aos-test-cache"))
        );
    }

    #[test]
    fn world_config_from_env_uses_fallback_module_cache_dir_when_env_missing() {
        let cfg = WorldConfig::from_env_values(
            Some(std::path::PathBuf::from("/tmp/aos-fallback-cache")),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            cfg.module_cache_dir,
            Some(std::path::PathBuf::from("/tmp/aos-fallback-cache"))
        );
    }

    #[test]
    fn world_config_applies_kernel_cache_defaults() {
        let config = WorldConfig {
            module_cache_dir: Some(std::path::PathBuf::from("/tmp/aos-wasmtime-cache")),
            eager_module_load: true,
            allow_placeholder_secrets: true,
            strict_effect_routes: false,
            cell_cache_size: 512,
            forced_replay_seed_height: None,
        };

        let kernel = config.apply_kernel_defaults(KernelConfig::default());
        assert_eq!(
            kernel.module_cache_dir,
            Some(std::path::PathBuf::from("/tmp/aos-wasmtime-cache"))
        );
        assert!(kernel.eager_module_load);
        assert!(kernel.allow_placeholder_secrets);
        assert_eq!(kernel.cell_cache_size, 512);
    }

    #[test]
    fn world_config_preserves_non_default_kernel_overrides() {
        let config = WorldConfig {
            module_cache_dir: Some(std::path::PathBuf::from("/tmp/aos-wasmtime-cache")),
            eager_module_load: true,
            allow_placeholder_secrets: true,
            strict_effect_routes: false,
            cell_cache_size: 512,
            forced_replay_seed_height: None,
        };
        let kernel = config.apply_kernel_defaults(KernelConfig {
            module_cache_dir: Some(std::path::PathBuf::from("/tmp/custom-cache")),
            eager_module_load: true,
            allow_placeholder_secrets: true,
            secret_resolver: None,
            cell_cache_size: 128,
            ..KernelConfig::default()
        });

        assert_eq!(
            kernel.module_cache_dir,
            Some(std::path::PathBuf::from("/tmp/custom-cache"))
        );
        assert!(kernel.eager_module_load);
        assert!(kernel.allow_placeholder_secrets);
        assert_eq!(kernel.cell_cache_size, 128);
    }
}
