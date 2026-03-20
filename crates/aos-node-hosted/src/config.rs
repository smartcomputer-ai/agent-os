use std::collections::BTreeSet;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Persist(#[from] aos_fdb::PersistError),
}

#[derive(Debug, Clone)]
pub struct FdbWorkerConfig {
    pub worker_id: String,
    pub universe_filter: BTreeSet<aos_fdb::UniverseId>,
    pub worker_pins: BTreeSet<String>,
    pub heartbeat_interval: Duration,
    pub heartbeat_ttl: Duration,
    pub lease_ttl: Duration,
    pub lease_renew_interval: Duration,
    pub maintenance_idle_after: Duration,
    pub idle_release_after: Duration,
    pub warm_retain_after: Duration,
    pub effect_claim_timeout: Duration,
    pub timer_claim_timeout: Duration,
    pub shard_count: u32,
    pub ready_scan_limit: u32,
    pub world_scan_limit: u32,
    pub max_inbox_batch: u32,
    pub max_tick_steps_per_cycle: u32,
    pub max_effects_per_cycle: u32,
    pub max_timers_per_cycle: u32,
    pub dedupe_gc_sweep_limit: u32,
    pub supervisor_poll_interval: Duration,
    pub maintenance_universe_page_size: u32,
    pub effect_claim_requeue_interval: Duration,
    pub timer_claim_requeue_interval: Duration,
    pub effect_dedupe_gc_interval: Duration,
    pub timer_dedupe_gc_interval: Duration,
    pub portal_dedupe_gc_interval: Duration,
    pub cas_cache_bytes: usize,
    pub cas_cache_item_max_bytes: usize,
}

impl Default for FdbWorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", uuid::Uuid::new_v4()),
            universe_filter: BTreeSet::new(),
            worker_pins: BTreeSet::from([String::from("default")]),
            heartbeat_interval: Duration::from_secs(5),
            heartbeat_ttl: Duration::from_secs(15),
            lease_ttl: Duration::from_secs(20),
            lease_renew_interval: Duration::from_secs(5),
            maintenance_idle_after: Duration::from_secs(10),
            idle_release_after: Duration::from_secs(60),
            warm_retain_after: Duration::from_secs(5 * 60),
            effect_claim_timeout: Duration::from_secs(30),
            timer_claim_timeout: Duration::from_secs(30),
            shard_count: 1,
            ready_scan_limit: 256,
            world_scan_limit: 256,
            max_inbox_batch: 64,
            max_tick_steps_per_cycle: 256,
            max_effects_per_cycle: 64,
            max_timers_per_cycle: 64,
            dedupe_gc_sweep_limit: 64,
            supervisor_poll_interval: Duration::from_millis(500),
            maintenance_universe_page_size: 128,
            effect_claim_requeue_interval: Duration::from_millis(500),
            timer_claim_requeue_interval: Duration::from_millis(500),
            effect_dedupe_gc_interval: Duration::from_secs(30),
            timer_dedupe_gc_interval: Duration::from_secs(30),
            portal_dedupe_gc_interval: Duration::from_secs(30),
            cas_cache_bytes: 512 * 1024 * 1024,
            cas_cache_item_max_bytes: 32 * 1024 * 1024,
        }
    }
}

impl FdbWorkerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut cfg = Self::default();
        if let Ok(raw) = std::env::var("AOS_UNIVERSE_IDS") {
            let universes = raw
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    value.parse::<aos_fdb::UniverseId>().map_err(|err| {
                        aos_fdb::PersistError::validation(format!(
                            "invalid AOS_UNIVERSE_IDS entry '{value}': {err}"
                        ))
                    })
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            cfg.universe_filter = universes;
        }
        if let Ok(raw) = std::env::var("AOS_WORKER_PINS") {
            let pins: BTreeSet<_> = raw
                .split(',')
                .map(str::trim)
                .filter(|pin| !pin.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            if !pins.is_empty() {
                cfg.worker_pins = pins;
            }
        }
        if let Ok(raw) = std::env::var("AOS_CAS_CACHE_BYTES") {
            let parsed = raw.trim().parse::<usize>().map_err(|err| {
                aos_fdb::PersistError::validation(format!(
                    "invalid AOS_CAS_CACHE_BYTES '{raw}': {err}"
                ))
            })?;
            cfg.cas_cache_bytes = parsed;
        }
        if let Ok(raw) = std::env::var("AOS_CAS_CACHE_ITEM_MAX_BYTES") {
            let parsed = raw.trim().parse::<usize>().map_err(|err| {
                aos_fdb::PersistError::validation(format!(
                    "invalid AOS_CAS_CACHE_ITEM_MAX_BYTES '{raw}': {err}"
                ))
            })?;
            cfg.cas_cache_item_max_bytes = parsed;
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_defaults_worker_pins() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_PINS", None),
            ("AOS_UNIVERSE_IDS", None),
            ("AOS_CAS_CACHE_BYTES", None),
            ("AOS_CAS_CACHE_ITEM_MAX_BYTES", None),
        ]);

        let cfg = FdbWorkerConfig::from_env().unwrap();
        assert!(cfg.worker_pins.contains("default"));
    }

    #[test]
    fn from_env_reads_cas_cache_bytes() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_PINS", None),
            ("AOS_UNIVERSE_IDS", None),
            ("AOS_CAS_CACHE_BYTES", Some("12345")),
            ("AOS_CAS_CACHE_ITEM_MAX_BYTES", None),
        ]);

        let cfg = FdbWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.cas_cache_bytes, 12345);
    }

    #[test]
    fn from_env_reads_cas_cache_item_max_bytes() {
        let _guard = EnvGuard::set(&[
            ("AOS_WORKER_PINS", None),
            ("AOS_UNIVERSE_IDS", None),
            ("AOS_CAS_CACHE_BYTES", None),
            ("AOS_CAS_CACHE_ITEM_MAX_BYTES", Some("678")),
        ]);

        let cfg = FdbWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.cas_cache_item_max_bytes, 678);
    }

    #[test]
    fn from_env_reads_universe_filters() {
        let _guard = EnvGuard::set(&[
            (
                "AOS_UNIVERSE_IDS",
                Some("11111111-1111-1111-1111-111111111111,22222222-2222-2222-2222-222222222222"),
            ),
            ("AOS_WORKER_PINS", None),
            ("AOS_CAS_CACHE_BYTES", None),
            ("AOS_CAS_CACHE_ITEM_MAX_BYTES", None),
        ]);

        let cfg = FdbWorkerConfig::from_env().unwrap();
        assert_eq!(cfg.universe_filter.len(), 2);
    }

    struct EnvGuard {
        _guard: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let guard = ENV_LOCK.lock().unwrap();
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
