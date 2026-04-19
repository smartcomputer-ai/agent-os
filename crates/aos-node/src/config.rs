use std::time::Duration;

use aos_node::WorldId;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD must be greater than zero")]
    InvalidMaxUncommittedSlicesPerWorld,
    #[error("invalid {field} value '{value}': {source}")]
    InvalidNumber {
        field: &'static str,
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("invalid {field} world id '{value}': {source}")]
    InvalidWorldId {
        field: &'static str,
        value: String,
        #[source]
        source: uuid::Error,
    },
}

#[derive(Debug, Clone)]
pub struct HostedWorkerConfig {
    pub worker_id: String,
    pub checkpoint_interval: Duration,
    pub checkpoint_every_events: Option<u32>,
    pub max_local_continuation_slices_per_flush: usize,
    pub max_uncommitted_slices_per_world: usize,
    pub owned_worlds: Option<std::collections::BTreeSet<WorldId>>,
}

impl Default for HostedWorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::new_v4()),
            checkpoint_interval: Duration::from_secs(60 * 2),
            checkpoint_every_events: Some(2000),
            max_local_continuation_slices_per_flush: 64,
            max_uncommitted_slices_per_world: 256,
            owned_worlds: None,
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
        if let Ok(raw) = std::env::var("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH") {
            cfg.max_local_continuation_slices_per_flush =
                parse_u32_env("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", &raw)? as usize;
        }
        if let Ok(raw) = std::env::var("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD") {
            let parsed = parse_u32_env("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", &raw)?;
            if parsed == 0 {
                return Err(ConfigError::InvalidMaxUncommittedSlicesPerWorld);
            }
            cfg.max_uncommitted_slices_per_world = parsed as usize;
        }
        if let Ok(raw) = std::env::var("AOS_OWNED_WORLD_IDS") {
            let owned_worlds = parse_world_ids_env("AOS_OWNED_WORLD_IDS", &raw)?;
            if !owned_worlds.is_empty() {
                cfg.owned_worlds = Some(owned_worlds);
            }
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

fn parse_world_ids_env(
    field: &'static str,
    raw: &str,
) -> Result<std::collections::BTreeSet<WorldId>, ConfigError> {
    let mut world_ids = std::collections::BTreeSet::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let world_id = value
            .parse::<WorldId>()
            .map_err(|source| ConfigError::InvalidWorldId {
                field,
                value: value.to_owned(),
                source,
            })?;
        world_ids.insert(world_id);
    }
    Ok(world_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_defaults_worker_pins() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", None),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(cfg.checkpoint_interval, Duration::from_secs(120));
                assert_eq!(cfg.checkpoint_every_events, Some(2000));
                assert_eq!(cfg.max_local_continuation_slices_per_flush, 64);
                assert_eq!(cfg.max_uncommitted_slices_per_world, 256);
                assert!(cfg.owned_worlds.is_none());
            },
        );
    }

    #[test]
    fn from_env_reads_checkpoint_interval() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", Some("250")),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", None),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(cfg.checkpoint_interval, Duration::from_millis(250));
            },
        );
    }

    #[test]
    fn from_env_reads_checkpoint_every_events() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", Some("7")),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", None),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(cfg.checkpoint_every_events, Some(7));
            },
        );
    }

    #[test]
    fn from_env_reads_max_uncommitted_slices_per_world() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", Some("4")),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(cfg.max_uncommitted_slices_per_world, 4);
            },
        );
    }

    #[test]
    fn from_env_rejects_zero_max_uncommitted_slices_per_world() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", Some("0")),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                assert!(matches!(
                    HostedWorkerConfig::from_env(),
                    Err(ConfigError::InvalidMaxUncommittedSlicesPerWorld)
                ));
            },
        );
    }

    #[test]
    fn from_env_reads_max_local_continuation_slices_per_flush() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", Some("7")),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", None),
                ("AOS_OWNED_WORLD_IDS", None),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(cfg.max_local_continuation_slices_per_flush, 7);
            },
        );
    }

    #[test]
    fn from_env_reads_owned_world_ids() {
        with_env(
            &[
                ("AOS_WORKER_ID", None),
                ("AOS_CHECKPOINT_INTERVAL_MS", None),
                ("AOS_CHECKPOINT_EVERY_EVENTS", None),
                ("AOS_MAX_LOCAL_CONTINUATION_SLICES_PER_FLUSH", None),
                ("AOS_MAX_UNCOMMITTED_SLICES_PER_WORLD", None),
                (
                    "AOS_OWNED_WORLD_IDS",
                    Some(
                        "00000000-0000-0000-0000-000000000001,00000000-0000-0000-0000-000000000002",
                    ),
                ),
            ],
            || {
                let cfg = HostedWorkerConfig::from_env().unwrap();
                assert_eq!(
                    cfg.owned_worlds.as_ref().map(|worlds| worlds.len()),
                    Some(2)
                );
            },
        );
    }

    fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let vars = vars
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.map(str::to_owned)))
            .collect::<Vec<_>>();
        temp_env::with_vars(vars, f);
    }
}
