#![allow(dead_code)]

use axum::Router;
use serde::Deserialize;
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use super::{
    DefsQuery, JournalQuery, LimitQuery, StateGetQuery, TraceQuery, TraceSummaryQuery,
    WorkspaceAnnotationsQuery, WorkspaceBytesQuery, WorkspaceEntriesQuery, WorkspaceEntryQuery,
    WorkspaceResolveQuery, WorldPageQuery,
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "AgentOS Node World API",
        version = "0.1.0",
        description = "Shared world-centric HTTP control API used by local and hosted AgentOS nodes."
    ),
    paths(
        doc_health,
        doc_list_worlds,
        doc_create_world,
        doc_get_world,
        doc_fork_world,
        doc_secret_bindings_list,
        doc_secret_binding_get,
        doc_secret_binding_put,
        doc_secret_binding_delete,
        doc_secret_versions_list,
        doc_secret_version_put,
        doc_secret_version_get,
        doc_manifest,
        doc_defs_list,
        doc_def_get,
        doc_runtime,
        doc_trace,
        doc_trace_summary,
        doc_journal_head,
        doc_journal_entries,
        doc_command_get,
        doc_state_get,
        doc_state_list,
        doc_workspace_resolve,
        doc_events_post,
        doc_receipts_post,
        doc_governance_propose,
        doc_governance_shadow,
        doc_governance_approve,
        doc_governance_apply,
        doc_workspace_empty_root,
        doc_workspace_entries,
        doc_workspace_entry,
        doc_workspace_bytes,
        doc_workspace_annotations,
        doc_workspace_apply,
        doc_workspace_diff,
        doc_cas_post,
        doc_cas_put,
        doc_cas_head,
        doc_cas_get
    ),
    components(schemas(ApiErrorResponse)),
    tags(
        (name = "service", description = "Service metadata"),
        (name = "worlds", description = "World lifecycle and inspection"),
        (name = "secrets", description = "Secret bindings and secret versions"),
        (name = "events", description = "Event and receipt ingress"),
        (name = "governance", description = "Governance command submission"),
        (name = "journal", description = "Journal and runtime inspection"),
        (name = "trace", description = "Trace inspection"),
        (name = "workspace", description = "Workspace tree and blob operations"),
        (name = "cas", description = "Content-addressed blob storage")
    )
)]
struct ApiDoc;

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
}

#[derive(Debug, Deserialize, ToSchema)]
struct ApiErrorResponse {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct UniverseParams {
    #[serde(default)]
    universe_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "service",
    responses(
        (status = 200, description = "Service health information", body = serde_json::Value),
        (status = 500, body = ApiErrorResponse)
    )
)]
fn doc_health() {}

#[utoipa::path(
    get,
    path = "/v1/worlds",
    tag = "worlds",
    params(WorldPageQuery),
    responses((status = 200, body = serde_json::Value))
)]
fn doc_list_worlds() {}

#[utoipa::path(
    post,
    path = "/v1/worlds",
    tag = "worlds",
    request_body = serde_json::Value,
    responses((status = 201, body = serde_json::Value))
)]
fn doc_create_world() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}",
    tag = "worlds",
    params(("world_id" = String, Path, description = "World identifier")),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_get_world() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/fork",
    tag = "worlds",
    params(("world_id" = String, Path, description = "Source world identifier")),
    request_body = serde_json::Value,
    responses((status = 201, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_fork_world() {}

#[utoipa::path(
    get,
    path = "/v1/secrets/bindings",
    tag = "secrets",
    params(UniverseParams),
    responses((status = 200, body = serde_json::Value))
)]
fn doc_secret_bindings_list() {}

#[utoipa::path(
    get,
    path = "/v1/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        UniverseParams
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_secret_binding_get() {}

#[utoipa::path(
    put,
    path = "/v1/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        UniverseParams
    ),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value))
)]
fn doc_secret_binding_put() {}

#[utoipa::path(
    delete,
    path = "/v1/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        UniverseParams
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_secret_binding_delete() {}

#[utoipa::path(
    get,
    path = "/v1/secrets/bindings/{binding_id}/versions",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        UniverseParams
    ),
    responses((status = 200, body = serde_json::Value))
)]
fn doc_secret_versions_list() {}

#[utoipa::path(
    post,
    path = "/v1/secrets/bindings/{binding_id}/versions",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        UniverseParams
    ),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value))
)]
fn doc_secret_version_put() {}

#[utoipa::path(
    get,
    path = "/v1/secrets/bindings/{binding_id}/versions/{version}",
    tag = "secrets",
    params(
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        ("version" = u64, Path, description = "Secret version"),
        UniverseParams
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_secret_version_get() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/manifest",
    tag = "worlds",
    params(("world_id" = String, Path, description = "World identifier")),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_manifest() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/defs",
    tag = "worlds",
    params(("world_id" = String, Path, description = "World identifier"), DefsQuery),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_defs_list() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/defs/{kind}/{name}",
    tag = "worlds",
    params(
        ("world_id" = String, Path, description = "World identifier"),
        ("kind" = String, Path, description = "Definition kind"),
        ("name" = String, Path, description = "Definition name")
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_def_get() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/runtime",
    tag = "journal",
    params(("world_id" = String, Path, description = "World identifier")),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_runtime() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/trace",
    tag = "trace",
    params(("world_id" = String, Path, description = "World identifier"), TraceQuery),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_trace() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/trace-summary",
    tag = "trace",
    params(("world_id" = String, Path, description = "World identifier"), TraceSummaryQuery),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_trace_summary() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/journal/head",
    tag = "journal",
    params(("world_id" = String, Path, description = "World identifier")),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_journal_head() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/journal",
    tag = "journal",
    params(("world_id" = String, Path, description = "World identifier"), JournalQuery),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_journal_entries() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/commands/{command_id}",
    tag = "governance",
    params(
        ("world_id" = String, Path, description = "World identifier"),
        ("command_id" = String, Path, description = "Command identifier")
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_command_get() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/state/{workflow}",
    tag = "worlds",
    params(
        ("world_id" = String, Path, description = "World identifier"),
        ("workflow" = String, Path, description = "Workflow identifier"),
        StateGetQuery
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_state_get() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/state/{workflow}/cells",
    tag = "worlds",
    params(
        ("world_id" = String, Path, description = "World identifier"),
        ("workflow" = String, Path, description = "Workflow identifier"),
        LimitQuery
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_state_list() {}

#[utoipa::path(
    get,
    path = "/v1/worlds/{world_id}/workspace/resolve",
    tag = "workspace",
    params(("world_id" = String, Path, description = "World identifier"), WorkspaceResolveQuery),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_workspace_resolve() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/events",
    tag = "events",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 202, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_events_post() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/receipts",
    tag = "events",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 202, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_receipts_post() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/governance/propose",
    tag = "governance",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_governance_propose() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/governance/shadow",
    tag = "governance",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_governance_shadow() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/governance/approve",
    tag = "governance",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_governance_approve() {}

#[utoipa::path(
    post,
    path = "/v1/worlds/{world_id}/governance/apply",
    tag = "governance",
    params(("world_id" = String, Path, description = "World identifier")),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_governance_apply() {}

#[utoipa::path(
    post,
    path = "/v1/workspace/roots",
    tag = "workspace",
    params(UniverseParams),
    responses((status = 201, body = serde_json::Value))
)]
fn doc_workspace_empty_root() {}

#[utoipa::path(
    get,
    path = "/v1/workspace/roots/{root_hash}/entries",
    tag = "workspace",
    params(
        ("root_hash" = String, Path, description = "Workspace root hash"),
        UniverseParams,
        WorkspaceEntriesQuery
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_workspace_entries() {}

#[utoipa::path(
    get,
    path = "/v1/workspace/roots/{root_hash}/entry",
    tag = "workspace",
    params(
        ("root_hash" = String, Path, description = "Workspace root hash"),
        UniverseParams,
        WorkspaceEntryQuery
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_workspace_entry() {}

#[utoipa::path(
    get,
    path = "/v1/workspace/roots/{root_hash}/bytes",
    tag = "workspace",
    params(
        ("root_hash" = String, Path, description = "Workspace root hash"),
        UniverseParams,
        WorkspaceBytesQuery
    ),
    responses((status = 200, description = "Raw workspace file bytes", content_type = "application/octet-stream"))
)]
fn doc_workspace_bytes() {}

#[utoipa::path(
    get,
    path = "/v1/workspace/roots/{root_hash}/annotations",
    tag = "workspace",
    params(
        ("root_hash" = String, Path, description = "Workspace root hash"),
        UniverseParams,
        WorkspaceAnnotationsQuery
    ),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_workspace_annotations() {}

#[utoipa::path(
    post,
    path = "/v1/workspace/roots/{root_hash}/apply",
    tag = "workspace",
    params(
        ("root_hash" = String, Path, description = "Workspace root hash"),
        UniverseParams
    ),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiErrorResponse))
)]
fn doc_workspace_apply() {}

#[utoipa::path(
    post,
    path = "/v1/workspace/diffs",
    tag = "workspace",
    params(UniverseParams),
    request_body = serde_json::Value,
    responses((status = 200, body = serde_json::Value))
)]
fn doc_workspace_diff() {}

#[utoipa::path(
    post,
    path = "/v1/cas/blobs",
    tag = "cas",
    params(UniverseParams),
    request_body(content = String, content_type = "application/octet-stream"),
    responses((status = 201, body = serde_json::Value))
)]
fn doc_cas_post() {}

#[utoipa::path(
    put,
    path = "/v1/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("sha256" = String, Path, description = "Expected SHA-256 hash"),
        UniverseParams
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses((status = 201, body = serde_json::Value))
)]
fn doc_cas_put() {}

#[utoipa::path(
    head,
    path = "/v1/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("sha256" = String, Path, description = "Blob SHA-256 hash"),
        UniverseParams
    ),
    responses((status = 200, description = "Blob exists"), (status = 404, body = ApiErrorResponse))
)]
fn doc_cas_head() {}

#[utoipa::path(
    get,
    path = "/v1/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("sha256" = String, Path, description = "Blob SHA-256 hash"),
        UniverseParams
    ),
    responses((status = 200, description = "Blob bytes", content_type = "application/octet-stream"))
)]
fn doc_cas_get() {}
