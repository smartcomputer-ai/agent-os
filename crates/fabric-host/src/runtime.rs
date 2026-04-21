use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use thiserror::Error;

use fabric_protocol::{
    ExecEvent, ExecRequest, NetworkMode, SessionId, SessionOpenRequest, SessionOpenResponse,
    SessionStatus, SessionStatusResponse, SignalSessionRequest,
};

pub type ExecEventStream =
    Pin<Box<dyn Stream<Item = Result<ExecEvent, FabricHostError>> + Send + 'static>>;

#[async_trait]
pub trait FabricRuntime: Send + Sync + 'static {
    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionOpenResponse, FabricHostError>;

    async fn session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionStatusResponse, FabricHostError>;

    async fn exec_stream(&self, request: ExecRequest) -> Result<ExecEventStream, FabricHostError>;

    async fn signal_session(
        &self,
        session_id: &SessionId,
        request: SignalSessionRequest,
    ) -> Result<SessionStatusResponse, FabricHostError>;

    async fn inventory(&self) -> Result<Vec<RuntimeInventoryEntry>, FabricHostError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInventoryEntry {
    pub session_id: SessionId,
    pub machine_name: String,
    pub status: SessionStatus,
    pub image: Option<String>,
    pub workdir: Option<String>,
    pub network_mode: Option<NetworkMode>,
}

#[derive(Debug, Error)]
pub enum FabricHostError {
    #[error("{0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
    #[error("runtime error: {0}")]
    Runtime(String),
}
