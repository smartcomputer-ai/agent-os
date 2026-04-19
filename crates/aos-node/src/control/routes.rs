use std::str::FromStr;
use std::sync::Arc;

use aos_cbor::Hash;
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceDiffReceipt, WorkspaceListReceipt,
};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Serialize;

use crate::control::openapi;
use crate::control::{
    AcceptWaitQuery, BlobPutResponse, CasBlobMetadata, CommandSubmitBody, CommandSubmitResponse,
    ControlError, CreateWorldBody, DefGetResponse, DefsListResponse, DefsQuery, ForkWorldBody,
    HeadInfoResponse, JournalEntriesResponse, JournalQuery, LimitQuery, ManifestResponse,
    PutSecretVersionBody, RawJournalEntriesResponse, ServiceInfoResponse, StateGetQuery,
    StateGetResponse, StateListResponse, SubmitEventBody, TraceQuery, TraceSummaryQuery,
    UniverseQuery, UpsertSecretBindingBody, WorkspaceAnnotationsQuery, WorkspaceApplyRequest,
    WorkspaceApplyResponse, WorkspaceBytesQuery, WorkspaceDiffBody, WorkspaceEntriesQuery,
    WorkspaceEntryQuery, WorkspaceResolveQuery, WorkspaceResolveResponse, WorldPageQuery,
    WorldSummaryResponse,
};
use crate::{
    CommandRecord, ReceiptIngress, SecretBindingRecord, SecretVersionRecord, UniverseId, WorldId,
    WorldRuntimeInfo,
};

const CAS_BLOB_UPLOAD_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

pub trait HttpBackend: Send + Sync + 'static {
    type CreateWorldResponse: Serialize;
    type ForkWorldResponse: Serialize;
    type SubmitEventResponse: Serialize;
    type SubmitReceiptResponse: Serialize;
    type WorkspaceEntryResponse: Serialize;

    fn health(&self) -> Result<ServiceInfoResponse, ControlError>;
    fn list_worlds(
        &self,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError>;
    fn create_world(
        &self,
        wait: AcceptWaitQuery,
        body: CreateWorldBody,
    ) -> Result<Self::CreateWorldResponse, ControlError>;
    fn get_world(&self, world_id: WorldId) -> Result<WorldSummaryResponse, ControlError>;
    fn checkpoint_world(&self, world_id: WorldId) -> Result<WorldSummaryResponse, ControlError>;
    fn fork_world(
        &self,
        src_world_id: WorldId,
        body: ForkWorldBody,
    ) -> Result<Self::ForkWorldResponse, ControlError>;
    fn manifest(&self, world_id: WorldId) -> Result<ManifestResponse, ControlError>;
    fn defs_list(
        &self,
        world_id: WorldId,
        query: DefsQuery,
    ) -> Result<DefsListResponse, ControlError>;
    fn def_get(
        &self,
        world_id: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError>;
    fn runtime(&self, world_id: WorldId) -> Result<WorldRuntimeInfo, ControlError>;
    fn trace(
        &self,
        world_id: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError>;
    fn trace_summary(
        &self,
        world_id: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError>;
    fn journal_head(&self, world_id: WorldId) -> Result<HeadInfoResponse, ControlError>;
    fn journal_entries(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<JournalEntriesResponse, ControlError>;
    fn journal_entries_raw(
        &self,
        world_id: WorldId,
        query: JournalQuery,
    ) -> Result<RawJournalEntriesResponse, ControlError>;
    fn get_command(
        &self,
        world_id: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError>;
    fn state_get(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: StateGetQuery,
    ) -> Result<StateGetResponse, ControlError>;
    fn state_list(
        &self,
        world_id: WorldId,
        workflow: &str,
        query: LimitQuery,
    ) -> Result<StateListResponse, ControlError>;
    fn workspace_resolve(
        &self,
        world_id: WorldId,
        query: WorkspaceResolveQuery,
    ) -> Result<WorkspaceResolveResponse, ControlError>;
    fn submit_event(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: SubmitEventBody,
    ) -> Result<Self::SubmitEventResponse, ControlError>;
    fn submit_receipt(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: ReceiptIngress,
    ) -> Result<Self::SubmitReceiptResponse, ControlError>;
    fn governance_propose(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovProposeParams>,
    ) -> Result<CommandSubmitResponse, ControlError>;
    fn governance_shadow(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovShadowParams>,
    ) -> Result<CommandSubmitResponse, ControlError>;
    fn governance_approve(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApproveParams>,
    ) -> Result<CommandSubmitResponse, ControlError>;
    fn governance_apply(
        &self,
        world_id: WorldId,
        wait: AcceptWaitQuery,
        body: CommandSubmitBody<GovApplyParams>,
    ) -> Result<CommandSubmitResponse, ControlError>;
    fn workspace_empty_root(
        &self,
        universe_id: Option<UniverseId>,
    ) -> Result<HashRef, ControlError>;
    fn workspace_entries(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntriesQuery,
    ) -> Result<WorkspaceListReceipt, ControlError>;
    fn workspace_entry(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceEntryQuery,
    ) -> Result<Self::WorkspaceEntryResponse, ControlError>;
    fn workspace_bytes(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceBytesQuery,
    ) -> Result<Vec<u8>, ControlError>;
    fn workspace_annotations(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: &HashRef,
        query: WorkspaceAnnotationsQuery,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError>;
    fn workspace_apply(
        &self,
        universe_id: Option<UniverseId>,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError>;
    fn workspace_diff(
        &self,
        universe_id: Option<UniverseId>,
        body: WorkspaceDiffBody,
    ) -> Result<WorkspaceDiffReceipt, ControlError>;
    fn put_blob(
        &self,
        bytes: &[u8],
        universe_id: Option<UniverseId>,
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError>;
    fn head_blob(
        &self,
        universe_id: Option<UniverseId>,
        hash: Hash,
    ) -> Result<CasBlobMetadata, ControlError>;
    fn get_blob(
        &self,
        universe_id: Option<UniverseId>,
        hash: Hash,
    ) -> Result<Vec<u8>, ControlError>;
    fn list_secret_bindings(
        &self,
        universe_id: Option<UniverseId>,
    ) -> Result<Vec<SecretBindingRecord>, ControlError>;
    fn get_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn upsert_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        body: UpsertSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn delete_secret_binding(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn list_secret_versions(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
    ) -> Result<Vec<SecretVersionRecord>, ControlError>;
    fn put_secret_version(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        body: PutSecretVersionBody,
    ) -> Result<SecretVersionRecord, ControlError>;
    fn get_secret_version(
        &self,
        universe_id: Option<UniverseId>,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError>;
}

pub fn router<B: HttpBackend>(backend: Arc<B>) -> Router {
    Router::new()
        .route("/health", get(health::<B>))
        .route("/v1/health", get(health::<B>))
        .route("/v1/worlds", get(list_worlds::<B>).post(create_world::<B>))
        .route("/v1/worlds/{world_id}", get(get_world::<B>))
        .route(
            "/v1/worlds/{world_id}/checkpoint",
            post(checkpoint_world::<B>),
        )
        .route("/v1/worlds/{world_id}/fork", post(fork_world::<B>))
        .route("/v1/secrets/bindings", get(secret_bindings_list::<B>))
        .route(
            "/v1/secrets/bindings/{binding_id}",
            get(secret_binding_get::<B>)
                .put(secret_binding_put::<B>)
                .delete(secret_binding_delete::<B>),
        )
        .route(
            "/v1/secrets/bindings/{binding_id}/versions",
            get(secret_versions_list::<B>).post(secret_version_post::<B>),
        )
        .route(
            "/v1/secrets/bindings/{binding_id}/versions/{version}",
            get(secret_version_get::<B>),
        )
        .route("/v1/worlds/{world_id}/manifest", get(get_manifest::<B>))
        .route("/v1/worlds/{world_id}/defs", get(defs_list::<B>))
        .route(
            "/v1/worlds/{world_id}/defs/{kind}/{name}",
            get(def_get::<B>),
        )
        .route("/v1/worlds/{world_id}/runtime", get(runtime::<B>))
        .route("/v1/worlds/{world_id}/trace", get(trace::<B>))
        .route(
            "/v1/worlds/{world_id}/trace-summary",
            get(trace_summary::<B>),
        )
        .route("/v1/worlds/{world_id}/journal/head", get(journal_head::<B>))
        .route("/v1/worlds/{world_id}/journal", get(journal_entries::<B>))
        .route(
            "/v1/worlds/{world_id}/commands/{command_id}",
            get(get_command::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/state/{workflow}",
            get(state_get::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/state/{workflow}/cells",
            get(state_list::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/workspace/resolve",
            get(workspace_resolve::<B>),
        )
        .route("/v1/worlds/{world_id}/events", post(events_post::<B>))
        .route("/v1/worlds/{world_id}/receipts", post(receipts_post::<B>))
        .route(
            "/v1/worlds/{world_id}/governance/propose",
            post(governance_propose::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/governance/shadow",
            post(governance_shadow::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/governance/approve",
            post(governance_approve::<B>),
        )
        .route(
            "/v1/worlds/{world_id}/governance/apply",
            post(governance_apply::<B>),
        )
        .route("/v1/workspace/roots", post(workspace_empty_root::<B>))
        .route(
            "/v1/workspace/roots/{root_hash}/entries",
            get(workspace_entries::<B>),
        )
        .route(
            "/v1/workspace/roots/{root_hash}/entry",
            get(workspace_entry::<B>),
        )
        .route(
            "/v1/workspace/roots/{root_hash}/bytes",
            get(workspace_bytes::<B>),
        )
        .route(
            "/v1/workspace/roots/{root_hash}/annotations",
            get(workspace_annotations::<B>),
        )
        .route(
            "/v1/workspace/roots/{root_hash}/apply",
            post(workspace_apply::<B>),
        )
        .route("/v1/workspace/diffs", post(workspace_diff::<B>))
        .merge(cas_router::<B>())
        .merge(openapi::router())
        .with_state(backend)
}

fn cas_router<B: HttpBackend>() -> Router<Arc<B>> {
    Router::new()
        .route("/v1/cas/blobs", post(cas_post::<B>))
        .route(
            "/v1/cas/blobs/{sha256}",
            put(cas_put::<B>).head(cas_head::<B>).get(cas_get::<B>),
        )
        .layer(DefaultBodyLimit::max(CAS_BLOB_UPLOAD_LIMIT_BYTES))
}

async fn health<B: HttpBackend>(
    State(backend): State<Arc<B>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.health()?))
}

async fn create_world<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<CreateWorldBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((StatusCode::CREATED, Json(backend.create_world(wait, body)?)))
}

async fn list_worlds<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(query): Query<WorldPageQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let after = query.after.as_deref().map(parse_world_id).transpose()?;
    Ok(Json(backend.list_worlds(after, query.limit)?))
}

async fn get_world<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.get_world(parse_world_id(&world_id)?)?))
}

async fn checkpoint_world<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.checkpoint_world(parse_world_id(&world_id)?)?))
}

async fn fork_world<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Json(body): Json<ForkWorldBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(backend.fork_world(parse_world_id(&world_id)?, body)?),
    ))
}

async fn secret_bindings_list<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.list_secret_bindings(query.universe_id)?))
}

async fn secret_binding_get<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(binding_id): Path<String>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.get_secret_binding(query.universe_id, &binding_id)?,
    ))
}

async fn secret_binding_put<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(binding_id): Path<String>,
    Query(query): Query<UniverseQuery>,
    Json(body): Json<UpsertSecretBindingBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.upsert_secret_binding(
        query.universe_id,
        &binding_id,
        body,
    )?))
}

async fn secret_binding_delete<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(binding_id): Path<String>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.delete_secret_binding(query.universe_id, &binding_id)?,
    ))
}

async fn secret_versions_list<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(binding_id): Path<String>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.list_secret_versions(query.universe_id, &binding_id)?,
    ))
}

async fn secret_version_post<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(binding_id): Path<String>,
    Query(query): Query<UniverseQuery>,
    Json(body): Json<PutSecretVersionBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.put_secret_version(
        query.universe_id,
        &binding_id,
        body,
    )?))
}

async fn secret_version_get<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path((binding_id, version)): Path<(String, u64)>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.get_secret_version(
        query.universe_id,
        &binding_id,
        version,
    )?))
}

async fn get_manifest<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.manifest(parse_world_id(&world_id)?)?))
}

async fn defs_list<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(query): Query<DefsQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.defs_list(parse_world_id(&world_id)?, query)?))
}

async fn def_get<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path((world_id, kind, name)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.def_get(
        parse_world_id(&world_id)?,
        &kind,
        &name,
    )?))
}

async fn runtime<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.runtime(parse_world_id(&world_id)?)?))
}

async fn trace<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(query): Query<TraceQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let correlate_value = query.value.map(parse_jsonish_query_value);
    Ok(Json(backend.trace(
        parse_world_id(&world_id)?,
        query.event_hash.as_deref(),
        query.schema.as_deref(),
        query.correlate_by.as_deref(),
        correlate_value,
        query.window_limit,
    )?))
}

async fn trace_summary<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(query): Query<TraceSummaryQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.trace_summary(
        parse_world_id(&world_id)?,
        query.recent_limit,
    )?))
}

async fn journal_head<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.journal_head(parse_world_id(&world_id)?)?))
}

async fn journal_entries<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    headers: HeaderMap,
    Path(world_id): Path<String>,
    Query(query): Query<JournalQuery>,
) -> Result<Response, ControlError> {
    let world_id = parse_world_id(&world_id)?;
    if wants_cbor(&headers) {
        return cbor_response(&backend.journal_entries_raw(world_id, query)?);
    }
    Ok(Json(backend.journal_entries(world_id, query)?).into_response())
}

async fn events_post<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<SubmitEventBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(backend.submit_event(parse_world_id(&world_id)?, wait, body)?),
    ))
}

async fn receipts_post<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<ReceiptIngress>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::ACCEPTED,
        Json(backend.submit_receipt(parse_world_id(&world_id)?, wait, body)?),
    ))
}

async fn get_command<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path((world_id, command_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.get_command(parse_world_id(&world_id)?, &command_id)?,
    ))
}

async fn governance_propose<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<CommandSubmitBody<GovProposeParams>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.governance_propose(
        parse_world_id(&world_id)?,
        wait,
        body,
    )?))
}

async fn governance_shadow<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<CommandSubmitBody<GovShadowParams>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.governance_shadow(
        parse_world_id(&world_id)?,
        wait,
        body,
    )?))
}

async fn governance_approve<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<CommandSubmitBody<GovApproveParams>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.governance_approve(
        parse_world_id(&world_id)?,
        wait,
        body,
    )?))
}

async fn governance_apply<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(wait): Query<AcceptWaitQuery>,
    Json(body): Json<CommandSubmitBody<GovApplyParams>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.governance_apply(
        parse_world_id(&world_id)?,
        wait,
        body,
    )?))
}

async fn state_get<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path((world_id, workflow)): Path<(String, String)>,
    Query(query): Query<StateGetQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.state_get(
        parse_world_id(&world_id)?,
        &workflow,
        query,
    )?))
}

async fn state_list<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path((world_id, workflow)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.state_list(
        parse_world_id(&world_id)?,
        &workflow,
        query,
    )?))
}

async fn workspace_resolve<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(world_id): Path<String>,
    Query(query): Query<WorkspaceResolveQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.workspace_resolve(parse_world_id(&world_id)?, query)?,
    ))
}

async fn workspace_empty_root<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "root_hash": backend.workspace_empty_root(query.universe_id)?,
        })),
    ))
}

async fn workspace_entries<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(root_hash): Path<String>,
    Query(universe): Query<UniverseQuery>,
    Query(query): Query<WorkspaceEntriesQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.workspace_entries(
            universe.universe_id,
            &HashRef::from_str(&root_hash)
                .map_err(|_| ControlError::invalid(format!("invalid root hash '{root_hash}'")))?,
            query,
        )?,
    ))
}

async fn workspace_entry<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(root_hash): Path<String>,
    Query(universe): Query<UniverseQuery>,
    Query(query): Query<WorkspaceEntryQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.workspace_entry(
            universe.universe_id,
            &HashRef::from_str(&root_hash)
                .map_err(|_| ControlError::invalid(format!("invalid root hash '{root_hash}'")))?,
            query,
        )?,
    ))
}

async fn workspace_bytes<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(root_hash): Path<String>,
    Query(universe): Query<UniverseQuery>,
    Query(query): Query<WorkspaceBytesQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let bytes = backend.workspace_bytes(
        universe.universe_id,
        &HashRef::from_str(&root_hash)
            .map_err(|_| ControlError::invalid(format!("invalid root hash '{root_hash}'")))?,
        query,
    )?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

async fn workspace_annotations<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(root_hash): Path<String>,
    Query(universe): Query<UniverseQuery>,
    Query(query): Query<WorkspaceAnnotationsQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.workspace_annotations(
            universe.universe_id,
            &HashRef::from_str(&root_hash)
                .map_err(|_| ControlError::invalid(format!("invalid root hash '{root_hash}'")))?,
            query,
        )?,
    ))
}

async fn workspace_apply<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(root_hash): Path<String>,
    Query(query): Query<UniverseQuery>,
    Json(body): Json<WorkspaceApplyRequest>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        backend.workspace_apply(
            query.universe_id,
            HashRef::from_str(&root_hash)
                .map_err(|_| ControlError::invalid(format!("invalid root hash '{root_hash}'")))?,
            body,
        )?,
    ))
}

async fn workspace_diff<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(query): Query<UniverseQuery>,
    Json(body): Json<WorkspaceDiffBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(backend.workspace_diff(query.universe_id, body)?))
}

async fn cas_post<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Query(query): Query<UniverseQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(backend.put_blob(&body, query.universe_id, None)?),
    ))
}

async fn cas_put<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(sha256): Path<String>,
    Query(query): Query<UniverseQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(backend.put_blob(&body, query.universe_id, Some(parse_hash(&sha256)?))?),
    ))
}

async fn cas_head<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(sha256): Path<String>,
    Query(query): Query<UniverseQuery>,
) -> Result<Response, ControlError> {
    let metadata = backend.head_blob(query.universe_id, parse_hash(&sha256)?)?;
    if metadata.exists {
        Ok(StatusCode::OK.into_response())
    } else {
        Ok(StatusCode::NOT_FOUND.into_response())
    }
}

async fn cas_get<B: HttpBackend>(
    State(backend): State<Arc<B>>,
    Path(sha256): Path<String>,
    Query(query): Query<UniverseQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let bytes = backend.get_blob(query.universe_id, parse_hash(&sha256)?)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

pub fn parse_world_id(value: &str) -> Result<WorldId, ControlError> {
    WorldId::from_str(value)
        .map_err(|_| ControlError::invalid(format!("invalid world id '{value}'")))
}

pub fn parse_hash(value: &str) -> Result<Hash, ControlError> {
    Hash::from_hex_str(value)
        .map_err(|_| ControlError::invalid(format!("invalid sha256 '{value}'")))
}

pub fn wants_cbor(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("application/cbor"))
        .unwrap_or(false)
}

pub fn cbor_response<T: serde::Serialize>(value: &T) -> Result<Response, ControlError> {
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/cbor")],
        serde_cbor::to_vec(value)?,
    )
        .into_response())
}

pub fn parse_jsonish_query_value(raw: String) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .unwrap_or_else(|| serde_json::Value::String(raw))
}
