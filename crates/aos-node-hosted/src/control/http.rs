use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;

use crate::control::facade::ControlFacade;

#[derive(Debug, Clone)]
pub struct ControlHttpConfig {
    pub bind_addr: SocketAddr,
}

impl Default for ControlHttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 9011)),
        }
    }
}

pub async fn serve(config: ControlHttpConfig, facade: Arc<ControlFacade>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        bind = %local_addr,
        health = %format!("http://{local_addr}/v1/health"),
        docs = %format!("http://{local_addr}/docs"),
        roles = "control",
        "aos-node-hosted control listening"
    );
    axum::serve(listener, router(facade)).await?;
    Ok(())
}

pub fn router(facade: Arc<ControlFacade>) -> Router {
    aos_node::api::http::router(facade)
}
