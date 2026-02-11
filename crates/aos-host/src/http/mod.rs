pub mod api;
pub mod publish;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use utoipa_swagger_ui::SwaggerUi;

use crate::config::HttpServerConfig;
use crate::control::{ControlError, RequestEnvelope, handle_request};
use crate::modes::daemon::ControlMsg;

#[derive(Clone)]
pub struct HttpState {
    pub control_tx: mpsc::Sender<ControlMsg>,
    pub shutdown_tx: broadcast::Sender<()>,
    next_id: Arc<AtomicU64>,
}

impl HttpState {
    pub fn new(control_tx: mpsc::Sender<ControlMsg>, shutdown_tx: broadcast::Sender<()>) -> Self {
        Self {
            control_tx,
            shutdown_tx,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn next_request_id(&self) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("http-{id}")
    }
}

pub fn spawn_http_server(
    config: HttpServerConfig,
    control_tx: mpsc::Sender<ControlMsg>,
    shutdown_tx: broadcast::Sender<()>,
) -> Option<JoinHandle<()>> {
    if !config.enabled {
        return None;
    }
    let state = HttpState::new(control_tx, shutdown_tx.clone());
    let app = Router::new()
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api::openapi()))
        .nest("/api", api::router())
        .fallback(publish::handler)
        .with_state(state);

    Some(tokio::spawn(async move {
        let addr = config.bind;
        if let Err(err) = serve(addr, app, shutdown_tx).await {
            tracing::error!("http server error: {err}");
        }
    }))
}

pub async fn control_call(
    state: &HttpState,
    cmd: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, ControlError> {
    let env = RequestEnvelope {
        v: 1,
        id: state.next_request_id(),
        cmd: cmd.to_string(),
        payload,
    };
    let resp = handle_request(env, &state.control_tx, &state.shutdown_tx).await;
    if resp.ok {
        Ok(resp.result.unwrap_or_else(|| serde_json::json!({})))
    } else {
        Err(resp.error.unwrap_or_else(|| ControlError {
            code: "unknown".into(),
            message: "control request failed".into(),
        }))
    }
}

async fn serve(
    addr: SocketAddr,
    app: Router,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    tracing::info!("HTTP server listening on http://{}", addr);
    let mut shutdown_rx = shutdown_tx.subscribe();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.recv().await;
        })
        .await
        .map_err(|e| format!("serve {addr}: {e}"))
}
