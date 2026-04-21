use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct FabricControllerConfig {
    pub bind_addr: SocketAddr,
    pub db_path: PathBuf,
    pub host_heartbeat_timeout_ns: u128,
    pub host_heartbeat_interval_ns: u128,
    pub default_session_ttl_ns: Option<u128>,
    pub allow_unauthenticated_loopback: bool,
}

impl Default for FabricControllerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 8788)),
            db_path: PathBuf::from(".fabric-ctrl/controller.sqlite"),
            host_heartbeat_timeout_ns: 30_000_000_000,
            host_heartbeat_interval_ns: 5_000_000_000,
            default_session_ttl_ns: Some(86_400_000_000_000),
            allow_unauthenticated_loopback: true,
        }
    }
}
