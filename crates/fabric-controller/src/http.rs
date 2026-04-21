use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, RawQuery, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, patch, post},
};
use fabric_protocol::{
    ControllerExecRequest, ControllerSessionOpenRequest, ControllerSignalSessionRequest,
    ErrorResponse, FsApplyPatchRequest, FsEditFileRequest, FsFileWriteRequest, FsGlobRequest,
    FsGrepRequest, FsMkdirRequest, FsPathQuery, FsRemoveRequest, HealthResponse,
    HostHeartbeatRequest, HostId, HostRegisterRequest, SessionId, SessionLabelsPatchRequest,
};
use futures_util::StreamExt;

use crate::{FabricControllerError, FabricControllerService};

pub fn router(service: Arc<FabricControllerService>) -> Router {
    Router::new()
        .merge(crate::openapi::router())
        .route("/healthz", get(healthz))
        .route("/v1/controller/info", get(controller_info))
        .route("/v1/hosts", get(list_hosts))
        .route("/v1/hosts/{host_id}", get(host))
        .route("/v1/hosts/{host_id}/inventory", get(host_inventory))
        .route("/v1/sessions", get(list_sessions).post(open_session))
        .route("/v1/sessions/{session_id}", get(session))
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
        .route(
            "/v1/sessions/{session_id}/labels",
            patch(patch_session_labels),
        )
        .route("/v1/hosts/register", post(register_host))
        .route("/v1/hosts/{host_id}/heartbeat", post(heartbeat_host))
        .with_state(service)
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "fabric-controller".to_owned(),
    })
}

async fn controller_info(
    State(service): State<Arc<FabricControllerService>>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.controller_info()))
}

async fn register_host(
    State(service): State<Arc<FabricControllerService>>,
    Json(request): Json<HostRegisterRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.register_host(request)?))
}

async fn heartbeat_host(
    State(service): State<Arc<FabricControllerService>>,
    Path(host_id): Path<String>,
    Json(request): Json<HostHeartbeatRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.heartbeat_host(HostId(host_id), request)?))
}

async fn list_hosts(
    State(service): State<Arc<FabricControllerService>>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.list_hosts()?))
}

async fn host(
    State(service): State<Arc<FabricControllerService>>,
    Path(host_id): Path<String>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.host(&HostId(host_id))?))
}

async fn host_inventory(
    State(service): State<Arc<FabricControllerService>>,
    Path(host_id): Path<String>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.host_inventory(&HostId(host_id))?))
}

async fn open_session(
    State(service): State<Arc<FabricControllerService>>,
    Json(request): Json<ControllerSessionOpenRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok((
        StatusCode::CREATED,
        Json(service.open_session(request).await?),
    ))
}

async fn session(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.session(&SessionId(session_id))?))
}

async fn list_sessions(
    State(service): State<Arc<FabricControllerService>>,
    RawQuery(query): RawQuery,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.list_sessions(&parse_label_filters(query.as_deref())?)?,
    ))
}

async fn patch_session_labels(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionLabelsPatchRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.patch_session_labels(&SessionId(session_id), request)?,
    ))
}

async fn exec_session(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<ControllerExecRequest>,
) -> Result<axum::response::Response, ControllerHttpError> {
    let stream = service
        .exec_session_stream(SessionId(session_id), request)
        .await?;
    let body_stream = stream.map(|event| {
        let line = match event {
            Ok(event) => serde_json::to_vec(&event),
            Err(error) => serde_json::to_vec(&ErrorResponse {
                code: controller_error_code(&error),
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
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<ControllerSignalSessionRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service
            .signal_session(&SessionId(session_id), request)
            .await?,
    ))
}

async fn read_file(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.read_file(&SessionId(session_id), &query).await?,
    ))
}

async fn write_file(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsFileWriteRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.write_file(&SessionId(session_id), &request).await?,
    ))
}

async fn edit_file(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsEditFileRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.edit_file(&SessionId(session_id), &request).await?,
    ))
}

async fn apply_patch(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsApplyPatchRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service
            .apply_patch(&SessionId(session_id), &request)
            .await?,
    ))
}

async fn mkdir(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsMkdirRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.mkdir(&SessionId(session_id), &request).await?))
}

async fn remove(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsRemoveRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.remove(&SessionId(session_id), &request).await?,
    ))
}

async fn stat(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.stat(&SessionId(session_id), &query).await?))
}

async fn exists(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.exists(&SessionId(session_id), &query).await?))
}

async fn list_dir(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Query(query): Query<FsPathQuery>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(
        service.list_dir(&SessionId(session_id), &query).await?,
    ))
}

async fn grep(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsGrepRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.grep(&SessionId(session_id), &request).await?))
}

async fn glob(
    State(service): State<Arc<FabricControllerService>>,
    Path(session_id): Path<String>,
    Json(request): Json<FsGlobRequest>,
) -> Result<impl IntoResponse, ControllerHttpError> {
    Ok(Json(service.glob(&SessionId(session_id), &request).await?))
}

struct ControllerHttpError(FabricControllerError);

impl From<FabricControllerError> for ControllerHttpError {
    fn from(error: FabricControllerError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ControllerHttpError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self.0 {
            FabricControllerError::BadRequest(_) => StatusCode::BAD_REQUEST,
            FabricControllerError::Conflict { .. } => StatusCode::CONFLICT,
            FabricControllerError::NotFound(_) => StatusCode::NOT_FOUND,
            FabricControllerError::UnsupportedTarget(_) => StatusCode::UNPROCESSABLE_ENTITY,
            FabricControllerError::UnsupportedLifecycle(_) => StatusCode::UNPROCESSABLE_ENTITY,
            FabricControllerError::NoHealthyHost(_) => StatusCode::SERVICE_UNAVAILABLE,
            FabricControllerError::HostError(_) => StatusCode::BAD_GATEWAY,
            FabricControllerError::Database(_)
            | FabricControllerError::Json(_)
            | FabricControllerError::Time(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = Json(ErrorResponse {
            code: controller_error_code(&self.0),
            message: self.0.to_string(),
        });

        (status, body).into_response()
    }
}

fn controller_error_code(error: &FabricControllerError) -> String {
    match error {
        FabricControllerError::BadRequest(_) => "invalid_request".to_owned(),
        FabricControllerError::Conflict { code, .. } => code.clone(),
        FabricControllerError::NotFound(_) => "not_found".to_owned(),
        FabricControllerError::UnsupportedTarget(_) => "unsupported_target".to_owned(),
        FabricControllerError::UnsupportedLifecycle(_) => "unsupported_lifecycle".to_owned(),
        FabricControllerError::NoHealthyHost(_) => "no_healthy_host".to_owned(),
        FabricControllerError::HostError(_) => "host_error".to_owned(),
        FabricControllerError::Database(_)
        | FabricControllerError::Json(_)
        | FabricControllerError::Time(_) => "runtime_error".to_owned(),
    }
}

fn parse_label_filters(query: Option<&str>) -> Result<Vec<(String, String)>, ControllerHttpError> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };

    url::form_urlencoded::parse(query.as_bytes())
        .filter_map(|(key, value)| (key == "label").then_some(value.into_owned()))
        .map(|label| {
            let Some((key, value)) = label.split_once(':') else {
                return Err(ControllerHttpError(FabricControllerError::BadRequest(
                    "label filters must use key:value".to_owned(),
                )));
            };
            if key.is_empty() || value.is_empty() {
                return Err(ControllerHttpError(FabricControllerError::BadRequest(
                    "label filters must use non-empty key:value".to_owned(),
                )));
            }
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}
