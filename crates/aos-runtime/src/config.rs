use aos_kernel::world::DEFAULT_CELL_CACHE_SIZE;

#[derive(Debug, Clone)]
pub struct WorldConfig {
    pub module_cache_dir: Option<std::path::PathBuf>,
    pub eager_module_load: bool,
    pub allow_placeholder_secrets: bool,
    pub strict_effect_bindings: bool,
    pub cell_cache_size: usize,
    pub forced_replay_seed_height: Option<u64>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            module_cache_dir: None,
            eager_module_load: false,
            allow_placeholder_secrets: false,
            strict_effect_bindings: false,
            cell_cache_size: DEFAULT_CELL_CACHE_SIZE,
            forced_replay_seed_height: None,
        }
    }
}

impl WorldConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(raw) = std::env::var("AOS_MODULE_CACHE_DIR") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                cfg.module_cache_dir = Some(std::path::PathBuf::from(trimmed));
            }
        } else if let Ok(cwd) = std::env::current_dir() {
            cfg.module_cache_dir = Some(cwd.join(".aos").join("cache").join("wasmtime"));
        }
        if matches!(
            std::env::var("AOS_STRICT_EFFECT_BINDINGS")
                .ok()
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Some("1" | "true" | "yes" | "on")
        ) {
            cfg.strict_effect_bindings = true;
        }
        if let Ok(raw) = std::env::var("AOS_CELL_CACHE_SIZE")
            && let Ok(parsed) = raw.trim().parse::<usize>()
        {
            cfg.cell_cache_size = parsed.max(1);
        }
        if let Ok(raw) = std::env::var("AOS_HOSTED_REPLAY_SEED_HEIGHT")
            && let Ok(parsed) = raw.trim().parse::<u64>()
        {
            cfg.forced_replay_seed_height = Some(parsed);
        }
        cfg
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
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(value: &str) -> Self {
            let previous = std::env::var("AOS_STRICT_EFFECT_BINDINGS").ok();
            unsafe {
                std::env::set_var("AOS_STRICT_EFFECT_BINDINGS", value);
            }
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                unsafe {
                    std::env::set_var("AOS_STRICT_EFFECT_BINDINGS", value);
                }
            } else {
                unsafe {
                    std::env::remove_var("AOS_STRICT_EFFECT_BINDINGS");
                }
            }
        }
    }

    #[test]
    fn world_config_from_env_enables_strict_effect_bindings() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::set("true");
        let cfg = WorldConfig::from_env();
        assert!(cfg.strict_effect_bindings);
    }

    #[test]
    fn world_config_from_env_reads_module_cache_dir() {
        let _lock = env_lock().lock().unwrap();
        let previous = std::env::var("AOS_MODULE_CACHE_DIR").ok();
        unsafe {
            std::env::set_var("AOS_MODULE_CACHE_DIR", "/tmp/aos-test-cache");
        }
        let cfg = WorldConfig::from_env();
        assert_eq!(
            cfg.module_cache_dir,
            Some(std::path::PathBuf::from("/tmp/aos-test-cache"))
        );
        unsafe {
            match previous {
                Some(value) => std::env::set_var("AOS_MODULE_CACHE_DIR", value),
                None => std::env::remove_var("AOS_MODULE_CACHE_DIR"),
            }
        }
    }
}
