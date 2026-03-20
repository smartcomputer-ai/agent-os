use std::net::SocketAddr;
use std::sync::Arc;

use aos_fdb::{HostedRuntimeStore, SecretStore, UniverseStore, WorldAdminStore};
use aos_node::control::{self as shared_control, ControlError};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::control::facade::ControlFacade;

#[derive(Debug, Clone)]
pub struct ControlHttpConfig {
    pub bind_addr: SocketAddr,
}

impl Default for ControlHttpConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 8080)),
        }
    }
}

pub async fn serve<P>(
    config: ControlHttpConfig,
    facade: Arc<ControlFacade<P>>,
) -> anyhow::Result<()>
where
    P: HostedRuntimeStore + SecretStore + WorldAdminStore + UniverseStore + 'static,
{
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    axum::serve(listener, router(facade)).await?;
    Ok(())
}

pub fn router<P>(facade: Arc<ControlFacade<P>>) -> Router
where
    P: HostedRuntimeStore + SecretStore + WorldAdminStore + UniverseStore + 'static,
{
    Router::new()
        .merge(shared_control::router::<ControlFacade<P>>())
        .route("/v1/universes/{universe_id}/workers", get(workers::<P>))
        .route(
            "/v1/universes/{universe_id}/workers/{worker_id}/worlds",
            get(worker_worlds::<P>),
        )
        .with_state(facade)
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    100
}

async fn workers<P>(
    State(facade): State<Arc<ControlFacade<P>>>,
    Path(universe_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError>
where
    P: HostedRuntimeStore + WorldAdminStore + UniverseStore + 'static,
{
    Ok(Json(facade.workers(
        shared_control::parse_universe_id(&universe_id)?,
        query.limit,
    )?))
}

async fn worker_worlds<P>(
    State(facade): State<Arc<ControlFacade<P>>>,
    Path((universe_id, worker_id)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError>
where
    P: HostedRuntimeStore + WorldAdminStore + UniverseStore + 'static,
{
    Ok(Json(facade.worker_worlds(
        shared_control::parse_universe_id(&universe_id)?,
        &worker_id,
        query.limit,
    )?))
}
