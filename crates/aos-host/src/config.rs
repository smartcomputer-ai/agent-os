use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub effect_timeout: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            effect_timeout: Duration::from_secs(30),
        }
    }
}
