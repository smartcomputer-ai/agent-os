#![allow(dead_code)]

use axum::Router;
use serde::Deserialize;
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "AgentOS Node Control API",
        version = "0.1.0",
        description = "Administrative and world-control HTTP API shared by local and hosted AgentOS nodes."
    ),
    paths(
        doc_health,
        doc_list_universes,
        doc_create_universe,
        doc_get_universe,
        doc_get_universe_by_handle,
        doc_patch_universe,
        doc_delete_universe,
        doc_secret_bindings_list,
        doc_secret_binding_put,
        doc_secret_binding_get,
        doc_secret_binding_delete,
        doc_secret_value_put,
        doc_secret_versions_list,
        doc_secret_version_get,
        doc_list_worlds,
        doc_create_world,
        doc_get_world,
        doc_get_world_by_handle,
        doc_patch_world,
        doc_fork_world,
        doc_command_get,
        doc_governance_propose,
        doc_governance_shadow,
        doc_governance_approve,
        doc_governance_apply,
        doc_world_pause,
        doc_world_archive,
        doc_world_delete,
        doc_manifest,
        doc_defs_list,
        doc_def_get,
        doc_state_get,
        doc_state_list,
        doc_events_post,
        doc_receipts_post,
        doc_journal_head,
        doc_journal_entries,
        doc_runtime,
        doc_trace,
        doc_trace_summary,
        doc_workspace_resolve,
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
    components(
        schemas(
            ApiErrorResponse,
            CreateUniverseRequestSchema,
            PatchUniverseRequestSchema,
            PutSecretBindingRequestSchema,
            PutSecretValueRequestSchema,
            CreateWorldRequestSchema,
            CreateWorldSourceSchema,
            PatchWorldRequestSchema,
            SnapshotSelectorSchema,
            ForkWorldRequestSchema,
            DomainEventRequestSchema,
            CborPayloadSchema,
            ReceiptIngressRequestSchema,
            GovProposeRequestSchema,
            GovShadowRequestSchema,
            GovApproveRequestSchema,
            GovApplyRequestSchema,
            LifecycleCommandRequestSchema,
            WorkspaceApplyRequestSchema,
            WorkspaceApplyOpSchema,
            WorkspaceDiffRequestSchema
        )
    ),
    tags(
        (name = "service", description = "Service and universe metadata"),
        (name = "secrets", description = "Secret binding and secret value operations"),
        (name = "worlds", description = "World lifecycle and inspection"),
        (name = "governance", description = "Governance command submission"),
        (name = "events", description = "Event and receipt ingress"),
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

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct ApiErrorResponse {
    code: String,
    message: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct CreateUniverseRequestSchema {
    #[serde(default)]
    #[schema(format = "uuid")]
    universe_id: Option<String>,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    created_at_ns: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct PatchUniverseRequestSchema {
    #[serde(default)]
    handle: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct PutSecretBindingRequestSchema {
    source_kind: String,
    #[serde(default)]
    env_var: Option<String>,
    #[serde(default)]
    required_placement_pin: Option<String>,
    #[serde(default)]
    created_at_ns: u64,
    #[serde(default)]
    updated_at_ns: u64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    actor: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct PutSecretValueRequestSchema {
    plaintext_b64: String,
    #[serde(default)]
    expected_digest: Option<String>,
    #[serde(default)]
    created_at_ns: u64,
    #[serde(default)]
    actor: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CreateWorldSourceSchema {
    Seed { seed: serde_json::Value },
    Manifest { manifest_hash: String },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct CreateWorldRequestSchema {
    #[serde(default)]
    #[schema(format = "uuid")]
    world_id: Option<String>,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    placement_pin: Option<String>,
    #[serde(default)]
    created_at_ns: u64,
    source: CreateWorldSourceSchema,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct PatchWorldRequestSchema {
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    placement_pin: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SnapshotSelectorSchema {
    ActiveBaseline,
    ByHeight { height: u64 },
    ByRef { snapshot_ref: String },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct ForkWorldRequestSchema {
    src_snapshot: SnapshotSelectorSchema,
    #[serde(default)]
    #[schema(format = "uuid")]
    new_world_id: Option<String>,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    placement_pin: Option<String>,
    #[serde(default)]
    forked_at_ns: u64,
    #[serde(default)]
    pending_effect_policy: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct DomainEventRequestSchema {
    schema: String,
    #[serde(default)]
    value_b64: Option<String>,
    #[serde(default)]
    value_json: Option<serde_json::Value>,
    #[serde(default)]
    key_b64: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct CborPayloadSchema {
    #[serde(default)]
    inline_cbor: Option<Vec<u8>>,
    #[serde(default)]
    cbor_ref: Option<String>,
    #[serde(default)]
    cbor_size: Option<u64>,
    #[serde(default)]
    cbor_sha256: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct ReceiptIngressRequestSchema {
    intent_hash: Vec<u8>,
    effect_kind: String,
    adapter_id: String,
    status: String,
    payload: CborPayloadSchema,
    #[serde(default)]
    cost_cents: Option<u64>,
    signature: Vec<u8>,
    #[serde(default)]
    correlation_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct GovProposeRequestSchema {
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    patch: serde_json::Value,
    #[serde(default)]
    summary: Option<serde_json::Value>,
    #[serde(default)]
    manifest_base: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct GovShadowRequestSchema {
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    proposal_id: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct GovApproveRequestSchema {
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    proposal_id: u64,
    decision: String,
    approver: String,
    #[serde(default)]
    reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct GovApplyRequestSchema {
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    proposal_id: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct LifecycleCommandRequestSchema {
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WorkspaceApplyOpSchema {
    WriteBytes {
        path: String,
        bytes_b64: String,
        #[serde(default)]
        mode: Option<u64>,
    },
    WriteRef {
        path: String,
        blob_hash: String,
        #[serde(default)]
        mode: Option<u64>,
    },
    Remove {
        path: String,
    },
    SetAnnotations {
        #[serde(default)]
        path: Option<String>,
        annotations_patch: serde_json::Value,
    },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct WorkspaceApplyRequestSchema {
    operations: Vec<WorkspaceApplyOpSchema>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, ToSchema)]
struct WorkspaceDiffRequestSchema {
    root_a: String,
    root_b: String,
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct UniversePageQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    after: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorldPageQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    after: Option<String>,
}

#[derive(Debug, Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
struct SecretDeleteQuery {
    #[serde(default)]
    actor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct DefsQuery {
    #[serde(default)]
    kinds: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct StateQuery {
    #[serde(default)]
    key_b64: Option<String>,
    #[serde(default)]
    consistency: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct JournalQuery {
    #[serde(default)]
    from: u64,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TraceQuery {
    #[serde(default)]
    event_hash: Option<String>,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    correlate_by: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    window_limit: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct TraceSummaryQuery {
    #[serde(default = "default_trace_summary_recent_limit")]
    recent_limit: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceResolveQuery {
    workspace: String,
    #[serde(default)]
    version: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceEntriesQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default = "default_workspace_limit")]
    limit: u64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceEntryQuery {
    path: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceBytesQuery {
    path: String,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceAnnotationsQuery {
    #[serde(default)]
    path: Option<String>,
}

fn default_limit() -> u32 {
    100
}

fn default_trace_summary_recent_limit() -> u32 {
    20
}

fn default_workspace_limit() -> u64 {
    1_000
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
    path = "/v1/universes",
    tag = "service",
    params(UniversePageQuery),
    responses(
        (status = 200, description = "List universes", body = serde_json::Value),
        (status = 400, body = ApiErrorResponse),
        (status = 500, body = ApiErrorResponse)
    )
)]
fn doc_list_universes() {}

#[utoipa::path(
    post,
    path = "/v1/universes",
    tag = "service",
    request_body = CreateUniverseRequestSchema,
    responses(
        (status = 201, description = "Universe created", body = serde_json::Value),
        (status = 400, body = ApiErrorResponse)
    )
)]
fn doc_create_universe() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}",
    tag = "service",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    responses(
        (status = 200, description = "Universe details", body = serde_json::Value),
        (status = 400, body = ApiErrorResponse),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_get_universe() {}

#[utoipa::path(
    get,
    path = "/v1/universes/by-handle/{handle}",
    tag = "service",
    params(("handle" = String, Path, description = "Universe handle")),
    responses(
        (status = 200, description = "Universe details", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_get_universe_by_handle() {}

#[utoipa::path(
    patch,
    path = "/v1/universes/{universe_id}",
    tag = "service",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    request_body = PatchUniverseRequestSchema,
    responses(
        (status = 200, description = "Universe updated", body = serde_json::Value),
        (status = 400, body = ApiErrorResponse),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_patch_universe() {}

#[utoipa::path(
    delete,
    path = "/v1/universes/{universe_id}",
    tag = "service",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    responses(
        (status = 200, description = "Universe deleted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_delete_universe() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/secrets/bindings",
    tag = "secrets",
    params(("universe_id" = String, Path, description = "Universe identifier"), LimitQuery),
    responses(
        (status = 200, description = "Secret bindings", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_bindings_list() {}

#[utoipa::path(
    put,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier")
    ),
    request_body = PutSecretBindingRequestSchema,
    responses(
        (status = 200, description = "Secret binding upserted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_binding_put() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier")
    ),
    responses(
        (status = 200, description = "Secret binding", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_binding_get() {}

#[utoipa::path(
    delete,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        SecretDeleteQuery
    ),
    responses(
        (status = 200, description = "Secret binding deleted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_binding_delete() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier")
    ),
    request_body = PutSecretValueRequestSchema,
    responses(
        (status = 201, description = "Secret value stored", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_value_put() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        LimitQuery
    ),
    responses(
        (status = 200, description = "Secret versions", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_versions_list() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions/{version}",
    tag = "secrets",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("binding_id" = String, Path, description = "Secret binding identifier"),
        ("version" = u64, Path, description = "Secret version")
    ),
    responses(
        (status = 200, description = "Secret version details", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_secret_version_get() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds",
    tag = "worlds",
    params(("universe_id" = String, Path, description = "Universe identifier"), WorldPageQuery),
    responses(
        (status = 200, description = "List worlds", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_list_worlds() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds",
    tag = "worlds",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    request_body = CreateWorldRequestSchema,
    responses(
        (status = 201, description = "World created", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_create_world() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    responses(
        (status = 200, description = "World details", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_get_world() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/by-handle/{handle}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("handle" = String, Path, description = "World handle")
    ),
    responses(
        (status = 200, description = "World details", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_get_world_by_handle() {}

#[utoipa::path(
    patch,
    path = "/v1/universes/{universe_id}/worlds/{world_id}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = PatchWorldRequestSchema,
    responses(
        (status = 200, description = "World updated", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_patch_world() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/fork",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "Source world identifier")
    ),
    request_body = ForkWorldRequestSchema,
    responses(
        (status = 201, description = "World forked", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_fork_world() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/commands/{command_id}",
    tag = "governance",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        ("command_id" = String, Path, description = "Command identifier")
    ),
    responses(
        (status = 200, description = "Command status", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_command_get() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/governance/propose",
    tag = "governance",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = GovProposeRequestSchema,
    responses(
        (status = 202, description = "Propose command accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_governance_propose() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/governance/shadow",
    tag = "governance",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = GovShadowRequestSchema,
    responses(
        (status = 202, description = "Shadow command accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_governance_shadow() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/governance/approve",
    tag = "governance",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = GovApproveRequestSchema,
    responses(
        (status = 202, description = "Approve command accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_governance_approve() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/governance/apply",
    tag = "governance",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = GovApplyRequestSchema,
    responses(
        (status = 202, description = "Apply command accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_governance_apply() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/pause",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = Option<LifecycleCommandRequestSchema>,
    responses(
        (status = 202, description = "Pause command accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_world_pause() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/archive",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = Option<LifecycleCommandRequestSchema>,
    responses(
        (status = 202, description = "Archive transition accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_world_archive() {}

#[utoipa::path(
    delete,
    path = "/v1/universes/{universe_id}/worlds/{world_id}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = Option<LifecycleCommandRequestSchema>,
    responses(
        (status = 202, description = "Delete transition accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_world_delete() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/manifest",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    responses(
        (status = 200, description = "Loaded manifest", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_manifest() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/defs",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        DefsQuery
    ),
    responses(
        (status = 200, description = "Definition listing", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_defs_list() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/defs/{kind}/{name}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        ("kind" = String, Path, description = "Definition kind"),
        ("name" = String, Path, description = "Definition name")
    ),
    responses(
        (status = 200, description = "Definition document", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_def_get() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/state/{workflow}",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        ("workflow" = String, Path, description = "Workflow name"),
        StateQuery
    ),
    responses(
        (status = 200, description = "Workflow state", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_state_get() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/state/{workflow}/cells",
    tag = "worlds",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        ("workflow" = String, Path, description = "Workflow name"),
        LimitQuery
    ),
    responses(
        (status = 200, description = "Workflow cell state listing", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_state_list() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/events",
    tag = "events",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = DomainEventRequestSchema,
    responses(
        (status = 202, description = "Event accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_events_post() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/receipts",
    tag = "events",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    request_body = ReceiptIngressRequestSchema,
    responses(
        (status = 202, description = "Receipt accepted", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_receipts_post() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/journal/head",
    tag = "journal",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    responses(
        (status = 200, description = "Journal head", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_journal_head() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/journal",
    tag = "journal",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        JournalQuery
    ),
    responses(
        (status = 200, description = "Journal entries. Set Accept: application/cbor for CBOR output.", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_journal_entries() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/runtime",
    tag = "journal",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier")
    ),
    responses(
        (status = 200, description = "Runtime information", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_runtime() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/trace",
    tag = "trace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        TraceQuery
    ),
    responses(
        (status = 200, description = "Trace output", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_trace() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/trace-summary",
    tag = "trace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        TraceSummaryQuery
    ),
    responses(
        (status = 200, description = "Trace summary", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_trace_summary() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/worlds/{world_id}/workspace/resolve",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("world_id" = String, Path, description = "World identifier"),
        WorkspaceResolveQuery
    ),
    responses(
        (status = 200, description = "Resolve workspace root", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_resolve() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/workspace/roots",
    tag = "workspace",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    responses(
        (status = 201, description = "Created empty workspace root", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_empty_root() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/workspace/roots/{root_hash}/entries",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("root_hash" = String, Path, description = "Workspace root hash"),
        WorkspaceEntriesQuery
    ),
    responses(
        (status = 200, description = "Workspace entries", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_entries() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/workspace/roots/{root_hash}/entry",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("root_hash" = String, Path, description = "Workspace root hash"),
        WorkspaceEntryQuery
    ),
    responses(
        (status = 200, description = "Single workspace entry", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_entry() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/workspace/roots/{root_hash}/bytes",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("root_hash" = String, Path, description = "Workspace root hash"),
        WorkspaceBytesQuery
    ),
    responses(
        (status = 200, description = "Raw workspace file bytes", content_type = "application/octet-stream"),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_bytes() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/workspace/roots/{root_hash}/annotations",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("root_hash" = String, Path, description = "Workspace root hash"),
        WorkspaceAnnotationsQuery
    ),
    responses(
        (status = 200, description = "Workspace annotations", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_annotations() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/workspace/roots/{root_hash}/apply",
    tag = "workspace",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("root_hash" = String, Path, description = "Workspace root hash")
    ),
    request_body = WorkspaceApplyRequestSchema,
    responses(
        (status = 200, description = "Workspace patch applied", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_apply() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/workspace/diffs",
    tag = "workspace",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    request_body = WorkspaceDiffRequestSchema,
    responses(
        (status = 200, description = "Workspace diff", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_workspace_diff() {}

#[utoipa::path(
    post,
    path = "/v1/universes/{universe_id}/cas/blobs",
    tag = "cas",
    params(("universe_id" = String, Path, description = "Universe identifier")),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 201, description = "Blob stored", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_cas_post() {}

#[utoipa::path(
    put,
    path = "/v1/universes/{universe_id}/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("sha256" = String, Path, description = "Expected SHA-256 hash")
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 201, description = "Blob stored with expected hash", body = serde_json::Value),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_cas_put() {}

#[utoipa::path(
    head,
    path = "/v1/universes/{universe_id}/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("sha256" = String, Path, description = "Blob SHA-256 hash")
    ),
    responses(
        (status = 200, description = "Blob exists"),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_cas_head() {}

#[utoipa::path(
    get,
    path = "/v1/universes/{universe_id}/cas/blobs/{sha256}",
    tag = "cas",
    params(
        ("universe_id" = String, Path, description = "Universe identifier"),
        ("sha256" = String, Path, description = "Blob SHA-256 hash")
    ),
    responses(
        (status = 200, description = "Blob bytes", content_type = "application/octet-stream"),
        (status = 404, body = ApiErrorResponse)
    )
)]
fn doc_cas_get() {}
