use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct FabricHostConfig {
    pub bind_addr: SocketAddr,
    pub state_root: PathBuf,
    pub host_id: String,
    pub controller_url: Option<String>,
    pub advertise_url: Option<String>,
    pub heartbeat_interval_ns: u128,
}

impl Default for FabricHostConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 8791)),
            state_root: PathBuf::from(".fabric-host"),
            host_id: "local-dev".to_owned(),
            controller_url: None,
            advertise_url: None,
            heartbeat_interval_ns: 5_000_000_000,
        }
    }
}
