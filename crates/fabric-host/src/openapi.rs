#![allow(dead_code)]

use axum::Router;
use fabric_protocol::*;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Fabric Host API",
        version = "0.1.0",
        description = "Direct Fabric host API for host-local smolvm sessions, exec, signals, inventory, and workspace filesystem access."
    ),
    paths(
        doc_healthz,
        doc_host_info,
        doc_host_inventory,
        doc_open_session,
        doc_session_status,
        doc_exec_session,
        doc_signal_session,
        doc_read_file,
        doc_write_file,
        doc_edit_file,
        doc_apply_patch,
        doc_mkdir,
        doc_remove,
        doc_stat,
        doc_exists,
        doc_list_dir,
        doc_grep,
        doc_glob
    ),
    components(schemas(
        ErrorResponse,
        ExecEvent,
        ExecEventKind,
        ExecId,
        FabricBytes,
        FsApplyPatchRequest,
        FsApplyPatchResponse,
        FsDirEntry,
        FsEditFileRequest,
        FsEditFileResponse,
        FsEntryKind,
        FsExistsResponse,
        FsFileReadResponse,
        FsFileWriteRequest,
        FsGlobRequest,
        FsGlobResponse,
        FsGrepMatch,
        FsGrepRequest,
        FsGrepResponse,
        FsListDirResponse,
        FsMkdirRequest,
        FsPatchOpsSummary,
        FsPathQuery,
        FsRemoveRequest,
        FsRemoveResponse,
        FsStatResponse,
        FsWriteResponse,
        HealthResponse,
        HostId,
        HostInfoResponse,
        HostInventoryResponse,
        HostInventorySession,
        MountSpec,
        NetworkMode,
        ResourceLimits,
        SessionId,
        SessionOpenRequest,
        SessionOpenResponse,
        SessionSignal,
        SessionStatus,
        SessionStatusResponse,
        SignalSessionRequest
    )),
    tags(
        (name = "service", description = "Service health"),
        (name = "host", description = "Host metadata and inventory"),
        (name = "sessions", description = "Direct host session lifecycle"),
        (name = "exec", description = "Host-local streaming exec"),
        (name = "signals", description = "Host session signals"),
        (name = "filesystem", description = "Workspace filesystem operations")
    )
)]
struct ApiDoc;

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
}

#[utoipa::path(
    get,
    path = "/healthz",
    tag = "service",
    responses((status = 200, body = HealthResponse))
)]
fn doc_healthz() {}

#[utoipa::path(
    get,
    path = "/v1/host/info",
    tag = "host",
    responses((status = 200, body = HostInfoResponse), (status = 500, body = ErrorResponse))
)]
fn doc_host_info() {}

#[utoipa::path(
    get,
    path = "/v1/host/inventory",
    tag = "host",
    responses((status = 200, body = HostInventoryResponse), (status = 500, body = ErrorResponse))
)]
fn doc_host_inventory() {}

#[utoipa::path(
    post,
    path = "/v1/sessions",
    tag = "sessions",
    request_body = SessionOpenRequest,
    responses((status = 201, body = SessionOpenResponse), (status = 400, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_open_session() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}",
    tag = "sessions",
    params(("session_id" = String, Path, description = "Host session id")),
    responses((status = 200, body = SessionStatusResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_session_status() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/exec",
    tag = "exec",
    params(("session_id" = String, Path, description = "Host session id")),
    request_body = ExecRequest,
    responses((status = 200, description = "NDJSON stream of ExecEvent records", content_type = "application/x-ndjson", body = ExecEvent), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_exec_session() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/signal",
    tag = "signals",
    params(("session_id" = String, Path, description = "Host session id")),
    request_body = SignalSessionRequest,
    responses((status = 200, body = SessionStatusResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_signal_session() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/file",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query), ("offset_bytes" = Option<u64>, Query), ("max_bytes" = Option<u64>, Query)),
    responses((status = 200, body = FsFileReadResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_read_file() {}

#[utoipa::path(
    put,
    path = "/v1/sessions/{session_id}/fs/file",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsFileWriteRequest,
    responses((status = 200, body = FsWriteResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_write_file() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/edit",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsEditFileRequest,
    responses((status = 200, body = FsEditFileResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_edit_file() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/apply_patch",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsApplyPatchRequest,
    responses((status = 200, body = FsApplyPatchResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_apply_patch() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/mkdir",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsMkdirRequest,
    responses((status = 200, body = FsStatResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_mkdir() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/remove",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsRemoveRequest,
    responses((status = 200, body = FsRemoveResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_remove() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/stat",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsStatResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_stat() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/exists",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsExistsResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_exists() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/list_dir",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsListDirResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_list_dir() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/grep",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsGrepRequest,
    responses((status = 200, body = FsGrepResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_grep() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/glob",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsGlobRequest,
    responses((status = 200, body = FsGlobResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_glob() {}
