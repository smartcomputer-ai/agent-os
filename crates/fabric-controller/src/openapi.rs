#![allow(dead_code)]

use axum::Router;
use fabric_protocol::*;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Fabric Controller API",
        version = "0.1.0",
        description = "Controller-mediated Fabric API for host registration, scheduling, sessions, exec, signals, labels, and filesystem access."
    ),
    paths(
        doc_healthz,
        doc_controller_info,
        doc_register_host,
        doc_heartbeat_host,
        doc_list_hosts,
        doc_host,
        doc_host_inventory,
        doc_open_session,
        doc_list_sessions,
        doc_session,
        doc_patch_session_labels,
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
        AttachedHostProviderInfo,
        AttachedWorkspacePolicy,
        CloseSignal,
        ControllerExecRequest,
        ControllerInfoResponse,
        ControllerSessionListResponse,
        ControllerSessionOpenRequest,
        ControllerSessionOpenResponse,
        ControllerSessionStatus,
        ControllerSessionSummary,
        ControllerSignalSessionRequest,
        ErrorResponse,
        ExecEvent,
        ExecEventKind,
        ExecId,
        ExecStdin,
        FabricBytes,
        FabricAttachedHostTarget,
        FabricHostProvider,
        FabricSandboxTarget,
        FabricSessionSignal,
        FabricSessionSignalKind,
        FabricSessionTarget,
        FabricSessionTargetKind,
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
        HostHeartbeatRequest,
        HostId,
        HostInventoryResponse,
        HostInventorySession,
        HostListResponse,
        HostRegisterRequest,
        HostRegisterResponse,
        HostSelector,
        HostStatus,
        HostSummary,
        MountSpec,
        NetworkMode,
        ProviderCapacity,
        QuiesceSignal,
        RequestId,
        ResourceLimits,
        ResumeSignal,
        SessionId,
        SessionLabelsPatchRequest,
        SessionLabelsResponse,
        SessionOpenRequest,
        SessionOpenResponse,
        SessionSignal,
        SessionStatus,
        SessionStatusResponse,
        SignalSessionRequest,
        SmolvmProviderInfo,
        TerminateRuntimeSignal
    )),
    tags(
        (name = "service", description = "Service health and metadata"),
        (name = "hosts", description = "Host registration, heartbeat, and inventory"),
        (name = "sessions", description = "Controller session scheduling and inspection"),
        (name = "exec", description = "Controller-mediated streaming exec"),
        (name = "signals", description = "Session lifecycle signals"),
        (name = "labels", description = "Session label mutation"),
        (name = "filesystem", description = "Controller-mediated workspace filesystem operations")
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
    path = "/v1/controller/info",
    tag = "service",
    responses((status = 200, body = ControllerInfoResponse), (status = 500, body = ErrorResponse))
)]
fn doc_controller_info() {}

#[utoipa::path(
    post,
    path = "/v1/hosts/register",
    tag = "hosts",
    request_body = HostRegisterRequest,
    responses((status = 200, body = HostRegisterResponse), (status = 400, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_register_host() {}

#[utoipa::path(
    post,
    path = "/v1/hosts/{host_id}/heartbeat",
    tag = "hosts",
    params(("host_id" = String, Path, description = "Host id")),
    request_body = HostHeartbeatRequest,
    responses((status = 200, body = HostRegisterResponse), (status = 400, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_heartbeat_host() {}

#[utoipa::path(
    get,
    path = "/v1/hosts",
    tag = "hosts",
    responses((status = 200, body = HostListResponse), (status = 500, body = ErrorResponse))
)]
fn doc_list_hosts() {}

#[utoipa::path(
    get,
    path = "/v1/hosts/{host_id}",
    tag = "hosts",
    params(("host_id" = String, Path, description = "Host id")),
    responses((status = 200, body = HostSummary), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_host() {}

#[utoipa::path(
    get,
    path = "/v1/hosts/{host_id}/inventory",
    tag = "hosts",
    params(("host_id" = String, Path, description = "Host id")),
    responses((status = 200, body = HostInventoryResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_host_inventory() {}

#[utoipa::path(
    post,
    path = "/v1/sessions",
    tag = "sessions",
    request_body = ControllerSessionOpenRequest,
    responses((status = 201, body = ControllerSessionOpenResponse), (status = 400, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 503, body = ErrorResponse))
)]
fn doc_open_session() {}

#[utoipa::path(
    get,
    path = "/v1/sessions",
    tag = "sessions",
    params(("label" = Option<Vec<String>>, Query, description = "Repeated key:value label filters")),
    responses((status = 200, body = ControllerSessionListResponse), (status = 400, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_list_sessions() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}",
    tag = "sessions",
    params(("session_id" = String, Path, description = "Controller session id")),
    responses((status = 200, body = ControllerSessionSummary), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_session() {}

#[utoipa::path(
    patch,
    path = "/v1/sessions/{session_id}/labels",
    tag = "labels",
    params(("session_id" = String, Path, description = "Controller session id")),
    request_body = SessionLabelsPatchRequest,
    responses((status = 200, body = SessionLabelsResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
fn doc_patch_session_labels() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/exec",
    tag = "exec",
    params(("session_id" = String, Path, description = "Controller session id")),
    request_body = ControllerExecRequest,
    responses((status = 200, description = "NDJSON stream of ExecEvent records", content_type = "application/x-ndjson", body = ExecEvent), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_exec_session() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/signal",
    tag = "signals",
    params(("session_id" = String, Path, description = "Controller session id")),
    request_body = ControllerSignalSessionRequest,
    responses((status = 200, body = ControllerSessionSummary), (status = 404, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_signal_session() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/file",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query), ("offset_bytes" = Option<u64>, Query), ("max_bytes" = Option<u64>, Query)),
    responses((status = 200, body = FsFileReadResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_read_file() {}

#[utoipa::path(
    put,
    path = "/v1/sessions/{session_id}/fs/file",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsFileWriteRequest,
    responses((status = 200, body = FsWriteResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_write_file() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/edit",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsEditFileRequest,
    responses((status = 200, body = FsEditFileResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_edit_file() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/apply_patch",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsApplyPatchRequest,
    responses((status = 200, body = FsApplyPatchResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_apply_patch() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/mkdir",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsMkdirRequest,
    responses((status = 200, body = FsStatResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_mkdir() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/remove",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsRemoveRequest,
    responses((status = 200, body = FsRemoveResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_remove() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/stat",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsStatResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_stat() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/exists",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsExistsResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_exists() {}

#[utoipa::path(
    get,
    path = "/v1/sessions/{session_id}/fs/list_dir",
    tag = "filesystem",
    params(("session_id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = FsListDirResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_list_dir() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/grep",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsGrepRequest,
    responses((status = 200, body = FsGrepResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_grep() {}

#[utoipa::path(
    post,
    path = "/v1/sessions/{session_id}/fs/glob",
    tag = "filesystem",
    params(("session_id" = String, Path)),
    request_body = FsGlobRequest,
    responses((status = 200, body = FsGlobResponse), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 502, body = ErrorResponse))
)]
fn doc_glob() {}
