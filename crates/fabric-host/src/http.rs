use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::StreamExt;

use crate::{FabricHostError, FabricHostService};
use fabric_protocol::{
    ErrorResponse, ExecRequest, FsApplyPatchRequest, FsEditFileRequest, FsFileWriteRequest,
    FsGlobRequest, FsGrepRequest, FsMkdirRequest, FsPathQuery, FsRemoveRequest, HealthResponse,
    SessionId, SessionOpenRequest, SignalSessionRequest,
};

pub fn router(service: Arc<FabricHostService>) -> Router {
    Router::new()
        .merge(crate::openapi::router())
        .route("/healthz", get(healthz))
        .route("/v1/host/info", get(host_info))
        .route("/v1/host/inventory", get(host_inventory))
        .route("/v1/sessions", post(open_session))
        .route("/v1/sessions/{session_id}", get(session_status))
        .route("/v1/sessions/{session_id}/exec", post(exec_session))
        .route("/v1/sessions/{session_id}/signal", post(signal_session))
        .route(
            "/v1/sessions/{session_id}/fs/file",
            get(read_file).put(write_file),
        )
        .route("/v1/sessions/{session_id}/fs/edit", post(edit_file))
        .route(
            "/v1/sessions/{session_id}/fs/apply_patch",
            post(apply_patch),
        )
        .route("/v1/sessions/{session_id}/fs/mkdir", post(mkdir))
        .route("/v1/sessions/{session_id}/fs/remove", post(remove))
        .route("/v1/sessions/{session_id}/fs/stat", get(stat))
        .route("/v1/sessions/{session_id}/fs/exists", get(exists))
        .route("/v1/sessions/{session_id}/fs/list_dir", get(list_dir))
        .route("/v1/sessions/{session_id}/fs/grep", post(grep))
        .route("/v1/sessions/{session_id}/fs/glob", post(glob))
        .with_state(service)
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "fabric-host".to_owned(),
    })
}

async fn host_info(
    State(service): State<Arc<FabricHostService>>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.host_info()))
}

async fn host_inventory(
    State(service): State<Arc<FabricHostService>>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.inventory().await?))
}

async fn open_session(
    State(service): State<Arc<FabricHostService>>,
    Json(request): Json<SessionOpenRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    let response = service.open_session(request).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn session_status(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, HostHttpError> {
    let response = service.session_status(&SessionId(session_id)).await?;
    Ok(Json(response))
}

async fn exec_session(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(mut request): Json<ExecRequest>,
) -> Result<axum::response::Response, HostHttpError> {
    request.session_id = SessionId(session_id);
    let stream = service.exec_stream(request).await?;
    let body_stream = stream.map(|event| {
        let line = match event {
            Ok(event) => serde_json::to_vec(&event),
            Err(error) => serde_json::to_vec(&ErrorResponse {
                code: StatusCode::INTERNAL_SERVER_ERROR.as_str().to_owned(),
                message: error.to_string(),
            }),
        }
        .map(|mut data| {
            data.push(b'\n');
            Bytes::from(data)
        })
        .map_err(std::io::Error::other);

        line
    });

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(body_stream),
    )
        .into_response())
}

async fn signal_session(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<SignalSessionRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    let response = service
        .signal_session(&SessionId(session_id), request)
        .await?;
    Ok(Json(response))
}

async fn read_file(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.read_file(&SessionId(session_id), query)?))
}

async fn write_file(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsFileWriteRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.write_file(&SessionId(session_id), request)?))
}

async fn edit_file(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsEditFileRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.edit_file(&SessionId(session_id), request)?))
}

async fn apply_patch(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsApplyPatchRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.apply_patch(&SessionId(session_id), request)?))
}

async fn mkdir(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsMkdirRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.mkdir(&SessionId(session_id), request)?))
}

async fn remove(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsRemoveRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.remove(&SessionId(session_id), request)?))
}

async fn stat(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.stat(&SessionId(session_id), query)?))
}

async fn exists(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.exists(&SessionId(session_id), query)?))
}

async fn list_dir(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.list_dir(&SessionId(session_id), query)?))
}

async fn grep(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsGrepRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.grep(&SessionId(session_id), request)?))
}

async fn glob(
    State(service): State<Arc<FabricHostService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsGlobRequest>,
) -> Result<impl IntoResponse, HostHttpError> {
    Ok(Json(service.glob(&SessionId(session_id), request)?))
}

struct HostHttpError(FabricHostError);

impl From<FabricHostError> for HostHttpError {
    fn from(error: FabricHostError) -> Self {
        Self(error)
    }
}

impl IntoResponse for HostHttpError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self.0 {
            FabricHostError::BadRequest(_) => StatusCode::BAD_REQUEST,
            FabricHostError::Conflict(_) => StatusCode::CONFLICT,
            FabricHostError::NotFound(_) => StatusCode::NOT_FOUND,
            FabricHostError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            FabricHostError::Runtime(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = Json(ErrorResponse {
            code: host_error_code(&self.0).to_owned(),
            message: self.0.to_string(),
        });

        (status, body).into_response()
    }
}

fn host_error_code(error: &FabricHostError) -> &'static str {
    match error {
        FabricHostError::BadRequest(_) => "invalid_request",
        FabricHostError::Conflict(_) => "conflict",
        FabricHostError::NotFound(_) => "not_found",
        FabricHostError::NotImplemented(_) => "not_implemented",
        FabricHostError::Runtime(_) => "runtime_error",
    }
}
