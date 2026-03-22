use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;

use aos_node::LocalControl;

#[derive(Debug, Clone)]
pub struct LocalHttpConfig {
    pub bind_addr: SocketAddr,
}

impl Default for LocalHttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 9010)),
        }
    }
}

pub async fn serve(config: LocalHttpConfig, control: Arc<LocalControl>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        bind = %local_addr,
        health = %format!("http://{local_addr}/v1/health"),
        docs = %format!("http://{local_addr}/docs"),
        roles = "supervisor,control",
        "aos-node-local listening"
    );
    axum::serve(listener, router(control)).await?;
    Ok(())
}

pub fn router(control: Arc<LocalControl>) -> Router {
    aos_node::api::http::router(control)
}
