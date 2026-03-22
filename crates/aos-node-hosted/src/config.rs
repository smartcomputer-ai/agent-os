use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("AOS_PARTITION_COUNT must be greater than zero")]
    InvalidPartitionCount,
    #[error("invalid {field} value '{value}': {source}")]
    InvalidNumber {
        field: &'static str,
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("invalid {field} value '{value}': expected true or false")]
    InvalidBool { field: &'static str, value: String },
}

#[derive(Debug, Clone)]
pub struct HostedWorkerConfig {
    pub worker_id: String,
    pub partition_count: u32,
    pub supervisor_poll_interval: Duration,
    pub checkpoint_interval: Duration,
    pub checkpoint_every_events: Option<u32>,
    pub checkpoint_on_create: bool,
}

impl Default for HostedWorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::new_v4()),
            partition_count: 1,
            supervisor_poll_interval: Duration::from_millis(100),
            checkpoint_interval: Duration::from_secs(60 * 2),
            checkpoint_every_events: Some(2000),
            checkpoint_on_create: true,
        }
    }
}

impl HostedWorkerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut cfg = Self::default();
        if let Ok(raw) = std::env::var("AOS_WORKER_ID") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                cfg.worker_id = trimmed.to_owned();
            }
        }
        if let Ok(raw) = std::env::var("AOS_PARTITION_COUNT") {
            let parsed = parse_u32_env("AOS_PARTITION_COUNT", &raw)?;
            if parsed == 0 {
                return Err(ConfigError::InvalidPartitionCount);
            }
            cfg.partition_count = parsed;
        }
        if let Ok(raw) = std::env::var("AOS_WORKER_POLL_INTERVAL_MS") {
            cfg.supervisor_poll_interval = Duration::from_millis(u64::from(parse_u32_env(
                "AOS_WORKER_POLL_INTERVAL_MS",
                &raw,
            )?));
        }
        if let Ok(raw) = std::env::var("AOS_CHECKPOINT_INTERVAL_MS") {
            cfg.checkpoint_interval = Duration::from_millis(u64::from(parse_u32_env(
                "AOS_CHECKPOINT_INTERVAL_MS",
                &raw,
            )?));
        }
        if let Ok(raw) = std::env::var("AOS_CHECKPOINT_EVERY_EVENTS") {
            let parsed = parse_u32_env("AOS_CHECKPOINT_EVERY_EVENTS", &raw)?;
            cfg.checkpoint_every_events = (parsed > 0).then_some(parsed);
        }
        if let Ok(raw) = std::env::var("AOS_CHECKPOINT_ON_CREATE") {
            cfg.checkpoint_on_create =
                raw.trim()
                    .parse::<bool>()
                    .map_err(|_| ConfigError::InvalidBool {
                        field: "AOS_CHECKPOINT_ON_CREATE",
                        value: raw.clone(),
                    })?;
        }
        Ok(cfg)
    }
}

fn parse_u32_env(field: &'static str, raw: &str) -> Result<u32, ConfigError> {
    raw.trim()
        .parse::<u32>()
        .map_err(|source| ConfigError::InvalidNumber {
            field,
            value: raw.to_owned(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_defaults_worker_pins() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_ID", None),
            ("AOS_PARTITION_COUNT", None),
            ("AOS_WORKER_POLL_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_EVERY_EVENTS", None),
        ]);

        let cfg = HostedWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.partition_count, 1);
        assert_eq!(cfg.checkpoint_every_events, Some(2000));
    }

    #[test]
    fn from_env_reads_partition_count() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_ID", None),
            ("AOS_PARTITION_COUNT", Some("4")),
            ("AOS_WORKER_POLL_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_EVERY_EVENTS", None),
        ]);

        let cfg = HostedWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.partition_count, 4);
    }

    #[test]
    fn from_env_reads_poll_interval() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_ID", None),
            ("AOS_PARTITION_COUNT", None),
            ("AOS_WORKER_POLL_INTERVAL_MS", Some("250")),
            ("AOS_CHECKPOINT_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_EVERY_EVENTS", None),
        ]);

        let cfg = HostedWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.supervisor_poll_interval, Duration::from_millis(250));
    }

    #[test]
    fn from_env_rejects_zero_partitions() {
        let _guard = EnvGuard::set(&[
            ("AOS_PARTITION_COUNT", Some("0")),
            ("AOS_WORKER_ID", None),
            ("AOS_WORKER_POLL_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_EVERY_EVENTS", None),
        ]);

        assert!(matches!(
            HostedWorkerConfig::from_env(),
            Err(ConfigError::InvalidPartitionCount)
        ));
    }

    #[test]
    fn from_env_reads_checkpoint_every_events() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_ID", None),
            ("AOS_PARTITION_COUNT", None),
            ("AOS_WORKER_POLL_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_INTERVAL_MS", None),
            ("AOS_CHECKPOINT_EVERY_EVENTS", Some("7")),
        ]);

        let cfg = HostedWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.checkpoint_every_events, Some(7));
    }

    struct EnvGuard {
        _guard: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let guard = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = vars
                .iter()
                .map(|(key, value)| {
                    let prior = std::env::var(key).ok();
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                    (*key, prior)
                })
                .collect();
            Self {
                _guard: guard,
                saved,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
}
