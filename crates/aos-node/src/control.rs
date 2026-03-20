use std::str::FromStr;
use std::sync::Arc;

use aos_air_types::{AirNode, Manifest};
use aos_cbor::Hash;
use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovProposeParams, GovShadowParams, HashRef,
    WorkspaceAnnotationsGetReceipt, WorkspaceAnnotationsPatch, WorkspaceDiffReceipt,
    WorkspaceListReceipt, WorkspaceReadRefReceipt, WorkspaceResolveReceipt,
};
use aos_kernel::StoreError;
use aos_kernel::{DefListing, KernelError};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

use crate::{
    CborPayload, CellStateProjectionRecord, CommandRecord, CommandStatus, CreateWorldRequest,
    DomainEventIngress, ForkPendingEffectPolicy, ForkWorldRequest, InboxSeq, PersistError,
    ReceiptIngress, SecretBindingRecord, SecretBindingSourceKind, SecretBindingStatus,
    SecretVersionRecord, SnapshotRecord, SnapshotSelector, UniverseCreateResult, UniverseId,
    UniverseRecord, WorldCreateResult, WorldForkResult, WorldId, WorldRuntimeInfo,
};

const CAS_BLOB_BODY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
}

impl ControlError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }
}

impl From<aos_effect_types::RefError> for ControlError {
    fn from(value: aos_effect_types::RefError) -> Self {
        Self::Invalid(value.to_string())
    }
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ControlError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            ControlError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ControlError::Persist(err) => match err {
                PersistError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
                PersistError::Validation(_) => (StatusCode::BAD_REQUEST, "validation_failed"),
                PersistError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "persist_error"),
            },
            ControlError::Kernel(_)
            | ControlError::Store(_)
            | ControlError::Cbor(_)
            | ControlError::Json(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        let body = Json(serde_json::json!({
            "code": code,
            "message": self.to_string(),
        }));
        (status, body).into_response()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfoResponse {
    pub service: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeadInfoResponse {
    pub journal_head: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UniverseSummaryResponse {
    #[serde(flatten)]
    pub record: UniverseRecord,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CreateUniverseBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universe_id: Option<UniverseId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PatchUniverseBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorldSummaryResponse {
    pub runtime: WorldRuntimeInfo,
    pub active_baseline: SnapshotRecord,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PatchWorldBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_pin: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PutSecretBindingBody {
    pub source_kind: SecretBindingSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_placement_pin: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default)]
    pub updated_at_ns: u64,
    #[serde(default)]
    pub status: Option<SecretBindingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PutSecretValueBody {
    pub plaintext_b64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretPutResponse {
    pub binding_id: String,
    pub version: u64,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestResponse {
    pub journal_head: u64,
    pub manifest_hash: String,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Serialize)]
pub struct DefsListResponse {
    pub journal_head: u64,
    pub manifest_hash: String,
    pub defs: Vec<DefListing>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DefGetResponse {
    pub journal_head: u64,
    pub manifest_hash: String,
    pub def: AirNode,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateGetResponse {
    pub journal_head: u64,
    pub workflow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellStateProjectionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateListResponse {
    pub journal_head: u64,
    pub workflow: String,
    pub cells: Vec<CellStateProjectionRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JournalEntryResponse {
    pub seq: u64,
    pub kind: String,
    pub record: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct JournalEntriesResponse {
    pub from: u64,
    pub next_from: u64,
    pub entries: Vec<JournalEntryResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawJournalEntryResponse {
    pub seq: u64,
    #[serde(with = "serde_bytes")]
    pub entry_cbor: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawJournalEntriesResponse {
    pub from: u64,
    pub next_from: u64,
    pub entries: Vec<RawJournalEntryResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandSubmitResponse {
    pub command_id: String,
    pub status: CommandStatus,
    pub poll_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceResolveResponse {
    pub workspace: String,
    #[serde(flatten)]
    pub receipt: WorkspaceResolveReceipt,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceApplyResponse {
    pub base_root_hash: HashRef,
    pub new_root_hash: HashRef,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkspaceApplyOp {
    WriteBytes {
        path: String,
        bytes_b64: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<u64>,
    },
    WriteRef {
        path: String,
        blob_hash: HashRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<u64>,
    },
    Remove {
        path: String,
    },
    SetAnnotations {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        annotations_patch: WorkspaceAnnotationsPatch,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceApplyRequest {
    pub operations: Vec<WorkspaceApplyOp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlobPutResponse {
    pub hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CasBlobMetadata {
    pub hash: String,
    pub exists: bool,
}

pub trait NodeControl: Send + Sync + 'static {
    fn health(&self) -> Result<ServiceInfoResponse, ControlError>;
    fn create_universe(
        &self,
        body: CreateUniverseBody,
    ) -> Result<UniverseCreateResult, ControlError>;
    fn get_universe(&self, universe: UniverseId) -> Result<UniverseSummaryResponse, ControlError>;
    fn get_universe_by_handle(&self, handle: &str)
    -> Result<UniverseSummaryResponse, ControlError>;
    fn delete_universe(
        &self,
        universe: UniverseId,
    ) -> Result<UniverseSummaryResponse, ControlError>;
    fn patch_universe(
        &self,
        universe: UniverseId,
        body: PatchUniverseBody,
    ) -> Result<UniverseSummaryResponse, ControlError>;
    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseSummaryResponse>, ControlError>;
    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, ControlError>;
    fn put_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretBindingBody,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn delete_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        actor: Option<String>,
    ) -> Result<SecretBindingRecord, ControlError>;
    fn put_secret_value(
        &self,
        universe: UniverseId,
        binding_id: &str,
        body: PutSecretValueBody,
    ) -> Result<SecretPutResponse, ControlError>;
    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, ControlError>;
    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<SecretVersionRecord, ControlError>;
    fn list_worlds(
        &self,
        universe: UniverseId,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, ControlError>;
    fn create_world(
        &self,
        universe: UniverseId,
        request: CreateWorldRequest,
    ) -> Result<WorldCreateResult, ControlError>;
    fn get_world(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldSummaryResponse, ControlError>;
    fn get_world_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
    ) -> Result<WorldSummaryResponse, ControlError>;
    fn patch_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        body: PatchWorldBody,
    ) -> Result<WorldSummaryResponse, ControlError>;
    fn fork_world(
        &self,
        universe: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<WorldForkResult, ControlError>;
    fn get_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<CommandRecord, ControlError>;
    fn submit_command<T: Serialize>(
        &self,
        universe: UniverseId,
        world: WorldId,
        command: &str,
        command_id: Option<String>,
        actor: Option<String>,
        payload: &T,
    ) -> Result<CommandSubmitResponse, ControlError>;
    fn archive_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError>;
    fn delete_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        operation_id: Option<String>,
        reason: Option<String>,
    ) -> Result<WorldSummaryResponse, ControlError>;
    fn manifest(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<ManifestResponse, ControlError>;
    fn defs_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        kinds: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<DefsListResponse, ControlError>;
    fn def_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        kind: &str,
        name: &str,
    ) -> Result<DefGetResponse, ControlError>;
    fn state_get(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key: Option<Vec<u8>>,
        consistency: Option<&str>,
    ) -> Result<StateGetResponse, ControlError>;
    fn state_list(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        limit: u32,
        consistency: Option<&str>,
    ) -> Result<StateListResponse, ControlError>;
    fn enqueue_event(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: DomainEventIngress,
    ) -> Result<InboxSeq, ControlError>;
    fn enqueue_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: ReceiptIngress,
    ) -> Result<InboxSeq, ControlError>;
    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<HeadInfoResponse, ControlError>;
    fn journal_entries(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<JournalEntriesResponse, ControlError>;
    fn journal_entries_raw(
        &self,
        universe: UniverseId,
        world: WorldId,
        from: u64,
        limit: u32,
    ) -> Result<RawJournalEntriesResponse, ControlError>;
    fn runtime(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<WorldRuntimeInfo, ControlError>;
    fn trace(
        &self,
        universe: UniverseId,
        world: WorldId,
        event_hash: Option<&str>,
        schema: Option<&str>,
        correlate_by: Option<&str>,
        correlate_value: Option<serde_json::Value>,
        window_limit: Option<u64>,
    ) -> Result<serde_json::Value, ControlError>;
    fn trace_summary(
        &self,
        universe: UniverseId,
        world: WorldId,
        recent_limit: u32,
    ) -> Result<serde_json::Value, ControlError>;
    fn workspace_resolve(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace_name: &str,
        version: Option<u64>,
    ) -> Result<WorkspaceResolveResponse, ControlError>;
    fn workspace_empty_root(&self, universe: UniverseId) -> Result<HashRef, ControlError>;
    fn workspace_entries(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
        scope: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> Result<WorkspaceListReceipt, ControlError>;
    fn workspace_entry(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
    ) -> Result<WorkspaceReadRefReceipt, ControlError>;
    fn workspace_bytes(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<u8>, ControlError>;
    fn workspace_annotations(
        &self,
        universe: UniverseId,
        root_hash: &HashRef,
        path: Option<&str>,
    ) -> Result<WorkspaceAnnotationsGetReceipt, ControlError>;
    fn workspace_apply(
        &self,
        universe: UniverseId,
        root_hash: HashRef,
        request: WorkspaceApplyRequest,
    ) -> Result<WorkspaceApplyResponse, ControlError>;
    fn workspace_diff(
        &self,
        universe: UniverseId,
        root_a: &HashRef,
        root_b: &HashRef,
        prefix: Option<&str>,
    ) -> Result<WorkspaceDiffReceipt, ControlError>;
    fn put_blob(
        &self,
        universe: UniverseId,
        bytes: &[u8],
        expected_hash: Option<Hash>,
    ) -> Result<BlobPutResponse, ControlError>;
    fn head_blob(&self, universe: UniverseId, hash: Hash) -> Result<CasBlobMetadata, ControlError>;
    fn get_blob(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, ControlError>;
}

pub fn router<C: NodeControl>() -> Router<Arc<C>> {
    Router::<Arc<C>>::new()
        .merge(crate::control_openapi::router())
        .route("/v1/health", get(health::<C>))
        .route(
            "/v1/universes",
            get(list_universes::<C>).post(create_universe::<C>),
        )
        .route(
            "/v1/universes/{universe_id}",
            get(get_universe::<C>)
                .patch(patch_universe::<C>)
                .delete(delete_universe::<C>),
        )
        .route(
            "/v1/universes/by-handle/{handle}",
            get(get_universe_by_handle::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/secrets/bindings",
            get(secret_bindings_list::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/secrets/bindings/{binding_id}",
            put(secret_binding_put::<C>)
                .get(secret_binding_get::<C>)
                .delete(secret_binding_delete::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions",
            get(secret_versions_list::<C>).post(secret_value_put::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/secrets/bindings/{binding_id}/versions/{version}",
            get(secret_version_get::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds",
            get(list_worlds::<C>).post(create_world::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}",
            get(get_world::<C>)
                .patch(patch_world::<C>)
                .delete(world_delete::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/by-handle/{handle}",
            get(get_world_by_handle::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/fork",
            post(fork_world::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/commands/{command_id}",
            get(command_get::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/governance/propose",
            post(governance_propose::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/governance/shadow",
            post(governance_shadow::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/governance/approve",
            post(governance_approve::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/governance/apply",
            post(governance_apply::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/pause",
            post(world_pause::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/archive",
            post(world_archive::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/manifest",
            get(manifest::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/defs",
            get(defs_list::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/defs/{kind}/{name}",
            get(def_get::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/state/{workflow}",
            get(state_get::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/state/{workflow}/cells",
            get(state_list::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/events",
            post(events_post::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/receipts",
            post(receipts_post::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/journal/head",
            get(journal_head::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/journal",
            get(journal_entries::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/runtime",
            get(runtime::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/trace",
            get(trace::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/trace-summary",
            get(trace_summary::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/worlds/{world_id}/workspace/resolve",
            get(workspace_resolve::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots",
            post(workspace_empty_root::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots/{root_hash}/entries",
            get(workspace_entries::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots/{root_hash}/entry",
            get(workspace_entry::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots/{root_hash}/bytes",
            get(workspace_bytes::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots/{root_hash}/annotations",
            get(workspace_annotations::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/roots/{root_hash}/apply",
            post(workspace_apply::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/workspace/diffs",
            post(workspace_diff::<C>),
        )
        .route(
            "/v1/universes/{universe_id}/cas/blobs",
            post(cas_post::<C>).layer(DefaultBodyLimit::max(CAS_BLOB_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/universes/{universe_id}/cas/blobs/{sha256}",
            put(cas_put::<C>)
                .layer(DefaultBodyLimit::max(CAS_BLOB_BODY_LIMIT_BYTES))
                .head(cas_head::<C>)
                .get(cas_get::<C>),
        )
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
struct UniversePageQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorldPageQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForkWorldBody {
    src_snapshot: SnapshotSelector,
    #[serde(default)]
    new_world_id: Option<WorldId>,
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    placement_pin: Option<String>,
    #[serde(default)]
    forked_at_ns: u64,
    #[serde(default)]
    pending_effect_policy: ForkPendingEffectPolicy,
}

#[derive(Debug, Deserialize)]
struct DomainEventBody {
    schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
}

impl DomainEventBody {
    fn into_payload(self) -> Result<CborPayload, ControlError> {
        match (self.value_b64, self.value_json) {
            (Some(value_b64), None) => Ok(CborPayload::inline(decode_b64(value_b64)?)),
            (None, Some(value_json)) => {
                let bytes = serde_cbor::to_vec(&value_json)
                    .map_err(|err| ControlError::invalid(format!("invalid event json: {err}")))?;
                Ok(CborPayload::inline(bytes))
            }
            (Some(_), Some(_)) => Err(ControlError::invalid(
                "provide only one of value_b64 or value_json",
            )),
            (None, None) => Err(ControlError::invalid("missing value_b64 or value_json")),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CommandSubmitBody<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[serde(flatten)]
    params: T,
}

#[derive(Debug, Deserialize, Default)]
struct LifecycleCommandBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SecretDeleteQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DefsQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kinds: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consistency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JournalQuery {
    #[serde(default)]
    from: u64,
    #[serde(default = "default_limit")]
    limit: u32,
}

#[derive(Debug, Deserialize)]
struct TraceQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correlate_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TraceSummaryQuery {
    #[serde(default = "default_trace_summary_recent_limit")]
    recent_limit: u32,
}

#[derive(Debug, Deserialize)]
struct WorkspaceResolveQuery {
    workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceEntriesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default = "default_workspace_limit")]
    limit: u64,
}

#[derive(Debug, Deserialize)]
struct WorkspaceEntryQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WorkspaceBytesQuery {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceAnnotationsQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceDiffBody {
    root_a: HashRef,
    root_b: HashRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
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

pub fn parse_universe_id(value: &str) -> Result<UniverseId, ControlError> {
    UniverseId::from_str(value)
        .map_err(|err| ControlError::invalid(format!("invalid universe id '{value}': {err}")))
}

pub fn parse_world_id(value: &str) -> Result<WorldId, ControlError> {
    WorldId::from_str(value)
        .map_err(|err| ControlError::invalid(format!("invalid world id '{value}': {err}")))
}

pub fn parse_hex_hash(value: &str) -> Result<Hash, ControlError> {
    Hash::from_hex_str(value)
        .map_err(|err| ControlError::invalid(format!("invalid hash '{value}': {err}")))
}

fn decode_b64(value: String) -> Result<Vec<u8>, ControlError> {
    BASE64_STANDARD
        .decode(value)
        .map_err(|err| ControlError::invalid(format!("invalid base64: {err}")))
}

fn parse_jsonish_query_value(raw: String) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .unwrap_or_else(|| serde_json::Value::String(raw))
}

fn parse_json_body_or_default<T>(body: &Bytes) -> Result<T, ControlError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    if body.is_empty() {
        Ok(T::default())
    } else {
        serde_json::from_slice(body)
            .map_err(|err| ControlError::invalid(format!("invalid request json: {err}")))
    }
}

fn submit_lifecycle_command<C: NodeControl>(
    control: &C,
    universe: UniverseId,
    world: WorldId,
    command: &str,
    body: LifecycleCommandBody,
) -> Result<(StatusCode, Json<CommandSubmitResponse>), ControlError> {
    let response = control.submit_command(
        universe,
        world,
        command,
        body.command_id,
        body.actor,
        &serde_json::json!({
            "reason": body.reason,
        }),
    )?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

fn wants_cbor(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("application/cbor"))
        .unwrap_or(false)
}

fn cbor_response<T: Serialize>(value: &T) -> Result<Response, ControlError> {
    let bytes = serde_cbor::to_vec(value)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/cbor")],
        bytes,
    )
        .into_response())
}

async fn health<C: NodeControl>(
    State(control): State<Arc<C>>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.health()?))
}

async fn list_universes<C: NodeControl>(
    State(control): State<Arc<C>>,
    Query(query): Query<UniversePageQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.list_universes(
        query.after.as_deref().map(parse_universe_id).transpose()?,
        query.limit,
    )?))
}

async fn create_universe<C: NodeControl>(
    State(control): State<Arc<C>>,
    Json(body): Json<CreateUniverseBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((StatusCode::CREATED, Json(control.create_universe(body)?)))
}

async fn get_universe<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        control.get_universe(parse_universe_id(&universe_id)?)?,
    ))
}

async fn get_universe_by_handle<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(handle): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_universe_by_handle(&handle)?))
}

async fn patch_universe<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    Json(body): Json<PatchUniverseBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        control.patch_universe(parse_universe_id(&universe_id)?, body)?,
    ))
}

async fn delete_universe<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(
        control.delete_universe(parse_universe_id(&universe_id)?)?,
    ))
}

async fn secret_bindings_list<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.list_secret_bindings(
        parse_universe_id(&universe_id)?,
        query.limit,
    )?))
}

async fn secret_binding_put<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id)): Path<(String, String)>,
    Json(body): Json<PutSecretBindingBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.put_secret_binding(
        parse_universe_id(&universe_id)?,
        &binding_id,
        body,
    )?))
}

async fn secret_binding_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_secret_binding(
        parse_universe_id(&universe_id)?,
        &binding_id,
    )?))
}

async fn secret_binding_delete<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id)): Path<(String, String)>,
    Query(query): Query<SecretDeleteQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.delete_secret_binding(
        parse_universe_id(&universe_id)?,
        &binding_id,
        query.actor,
    )?))
}

async fn secret_value_put<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id)): Path<(String, String)>,
    Json(body): Json<PutSecretValueBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(control.put_secret_value(parse_universe_id(&universe_id)?, &binding_id, body)?),
    ))
}

async fn secret_versions_list<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id)): Path<(String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.list_secret_versions(
        parse_universe_id(&universe_id)?,
        &binding_id,
        query.limit,
    )?))
}

async fn secret_version_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, binding_id, version)): Path<(String, String, u64)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_secret_version(
        parse_universe_id(&universe_id)?,
        &binding_id,
        version,
    )?))
}

async fn list_worlds<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    Query(query): Query<WorldPageQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.list_worlds(
        parse_universe_id(&universe_id)?,
        query.after.as_deref().map(parse_world_id).transpose()?,
        query.limit,
    )?))
}

async fn create_world<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    Json(body): Json<CreateWorldRequest>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(control.create_world(parse_universe_id(&universe_id)?, body)?),
    ))
}

async fn get_world<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_world(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
    )?))
}

async fn get_world_by_handle<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, handle)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_world_by_handle(
        parse_universe_id(&universe_id)?,
        &handle,
    )?))
}

async fn patch_world<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<PatchWorldBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.patch_world(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        body,
    )?))
}

async fn fork_world<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<ForkWorldBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(control.fork_world(
            parse_universe_id(&universe_id)?,
            ForkWorldRequest {
                src_world_id: parse_world_id(&world_id)?,
                src_snapshot: body.src_snapshot,
                new_world_id: body.new_world_id,
                handle: body.handle,
                placement_pin: body.placement_pin,
                forked_at_ns: body.forked_at_ns,
                pending_effect_policy: body.pending_effect_policy,
            },
        )?),
    ))
}

async fn command_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id, command_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.get_command(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        &command_id,
    )?))
}

async fn governance_propose<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<CommandSubmitBody<GovProposeParams>>,
) -> Result<impl IntoResponse, ControlError> {
    let response = control.submit_command(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        "gov-propose",
        body.command_id,
        body.actor,
        &body.params,
    )?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn governance_shadow<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<CommandSubmitBody<GovShadowParams>>,
) -> Result<impl IntoResponse, ControlError> {
    let response = control.submit_command(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        "gov-shadow",
        body.command_id,
        body.actor,
        &body.params,
    )?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn governance_approve<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<CommandSubmitBody<GovApproveParams>>,
) -> Result<impl IntoResponse, ControlError> {
    let response = control.submit_command(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        "gov-approve",
        body.command_id,
        body.actor,
        &body.params,
    )?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn governance_apply<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<CommandSubmitBody<GovApplyParams>>,
) -> Result<impl IntoResponse, ControlError> {
    let response = control.submit_command(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        "gov-apply",
        body.command_id,
        body.actor,
        &body.params,
    )?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn world_pause<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    let body = parse_json_body_or_default::<LifecycleCommandBody>(&body)?;
    Ok(submit_lifecycle_command(
        control.as_ref(),
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        "world-pause",
        body,
    )?)
}

async fn world_archive<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    let body = parse_json_body_or_default::<LifecycleCommandBody>(&body)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(control.archive_world(
            parse_universe_id(&universe_id)?,
            parse_world_id(&world_id)?,
            body.command_id,
            body.reason,
        )?),
    ))
}

async fn world_delete<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    let body = parse_json_body_or_default::<LifecycleCommandBody>(&body)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(control.delete_world(
            parse_universe_id(&universe_id)?,
            parse_world_id(&world_id)?,
            body.command_id,
            body.reason,
        )?),
    ))
}

async fn manifest<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.manifest(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
    )?))
}

async fn defs_list<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Query(query): Query<DefsQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let kinds = query.kinds.map(|raw| {
        raw.split(',')
            .filter(|kind| !kind.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    });
    Ok(Json(control.defs_list(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        kinds,
        query.prefix,
    )?))
}

async fn def_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id, kind, name)): Path<(String, String, String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.def_get(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        &kind,
        &name,
    )?))
}

async fn state_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id, workflow)): Path<(String, String, String)>,
    Query(query): Query<StateQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let key = query
        .key_b64
        .map(|value| {
            BASE64_STANDARD
                .decode(value)
                .map_err(|err| ControlError::invalid(format!("invalid base64: {err}")))
        })
        .transpose()?;
    Ok(Json(control.state_get(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        &workflow,
        key,
        query.consistency.as_deref(),
    )?))
}

async fn state_list<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id, workflow)): Path<(String, String, String)>,
    Query(query): Query<LimitQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.state_list(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        &workflow,
        query.limit,
        None,
    )?))
}

async fn events_post<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<DomainEventBody>,
) -> Result<impl IntoResponse, ControlError> {
    let DomainEventBody {
        schema,
        value_b64,
        value_json,
        key_b64,
        correlation_id,
    } = body;
    let ingress = DomainEventIngress {
        schema,
        value: DomainEventBody {
            schema: String::new(),
            value_b64,
            value_json,
            key_b64: None,
            correlation_id: None,
        }
        .into_payload()?,
        key: key_b64.map(decode_b64).transpose()?,
        correlation_id,
    };
    let seq = control.enqueue_event(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        ingress,
    )?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "inbox_seq": seq })),
    ))
}

async fn receipts_post<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Json(body): Json<ReceiptIngress>,
) -> Result<impl IntoResponse, ControlError> {
    body.payload.validate()?;
    let seq = control.enqueue_receipt(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        body,
    )?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "inbox_seq": seq })),
    ))
}

async fn journal_head<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.journal_head(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
    )?))
}

async fn journal_entries<C: NodeControl>(
    State(control): State<Arc<C>>,
    headers: HeaderMap,
    Path((universe_id, world_id)): Path<(String, String)>,
    Query(query): Query<JournalQuery>,
) -> Result<Response, ControlError> {
    let universe = parse_universe_id(&universe_id)?;
    let world = parse_world_id(&world_id)?;
    if wants_cbor(&headers) {
        return cbor_response(&control.journal_entries_raw(
            universe,
            world,
            query.from,
            query.limit,
        )?);
    }
    Ok(Json(control.journal_entries(universe, world, query.from, query.limit)?).into_response())
}

async fn runtime<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.runtime(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
    )?))
}

async fn trace<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Query(query): Query<TraceQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let correlate_value = query.value.map(parse_jsonish_query_value);
    Ok(Json(control.trace(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        query.event_hash.as_deref(),
        query.schema.as_deref(),
        query.correlate_by.as_deref(),
        correlate_value,
        query.window_limit,
    )?))
}

async fn trace_summary<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Query(query): Query<TraceSummaryQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.trace_summary(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        query.recent_limit,
    )?))
}

async fn workspace_resolve<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, world_id)): Path<(String, String)>,
    Query(query): Query<WorkspaceResolveQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_resolve(
        parse_universe_id(&universe_id)?,
        parse_world_id(&world_id)?,
        &query.workspace,
        query.version,
    )?))
}

async fn workspace_empty_root<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "root_hash": control.workspace_empty_root(parse_universe_id(&universe_id)?)?,
        })),
    ))
}

async fn workspace_entries<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, root_hash)): Path<(String, String)>,
    Query(query): Query<WorkspaceEntriesQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_entries(
        parse_universe_id(&universe_id)?,
        &HashRef::new(root_hash)?,
        query.path.as_deref(),
        query.scope.as_deref(),
        query.cursor.as_deref(),
        query.limit,
    )?))
}

async fn workspace_entry<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, root_hash)): Path<(String, String)>,
    Query(query): Query<WorkspaceEntryQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_entry(
        parse_universe_id(&universe_id)?,
        &HashRef::new(root_hash)?,
        &query.path,
    )?))
}

async fn workspace_bytes<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, root_hash)): Path<(String, String)>,
    Query(query): Query<WorkspaceBytesQuery>,
) -> Result<impl IntoResponse, ControlError> {
    let range = match (query.start, query.end) {
        (Some(start), Some(end)) => Some((start, end)),
        (None, None) => None,
        _ => {
            return Err(ControlError::invalid(
                "start and end must be provided together",
            ));
        }
    };
    let bytes = control.workspace_bytes(
        parse_universe_id(&universe_id)?,
        &HashRef::new(root_hash)?,
        &query.path,
        range,
    )?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

async fn workspace_annotations<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, root_hash)): Path<(String, String)>,
    Query(query): Query<WorkspaceAnnotationsQuery>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_annotations(
        parse_universe_id(&universe_id)?,
        &HashRef::new(root_hash)?,
        query.path.as_deref(),
    )?))
}

async fn workspace_apply<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, root_hash)): Path<(String, String)>,
    Json(body): Json<WorkspaceApplyRequest>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_apply(
        parse_universe_id(&universe_id)?,
        HashRef::new(root_hash)?,
        body,
    )?))
}

async fn workspace_diff<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    Json(body): Json<WorkspaceDiffBody>,
) -> Result<impl IntoResponse, ControlError> {
    Ok(Json(control.workspace_diff(
        parse_universe_id(&universe_id)?,
        &body.root_a,
        &body.root_b,
        body.prefix.as_deref(),
    )?))
}

async fn cas_post<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path(universe_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(control.put_blob(parse_universe_id(&universe_id)?, &body, None)?),
    ))
}

async fn cas_put<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, sha256)): Path<(String, String)>,
    body: Bytes,
) -> Result<impl IntoResponse, ControlError> {
    let expected = parse_hex_hash(&sha256)?;
    Ok((
        StatusCode::CREATED,
        Json(control.put_blob(parse_universe_id(&universe_id)?, &body, Some(expected))?),
    ))
}

async fn cas_head<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, sha256)): Path<(String, String)>,
) -> Result<Response, ControlError> {
    let meta = control.head_blob(parse_universe_id(&universe_id)?, parse_hex_hash(&sha256)?)?;
    if meta.exists {
        Ok(StatusCode::OK.into_response())
    } else {
        Ok(StatusCode::NOT_FOUND.into_response())
    }
}

async fn cas_get<C: NodeControl>(
    State(control): State<Arc<C>>,
    Path((universe_id, sha256)): Path<(String, String)>,
) -> Result<impl IntoResponse, ControlError> {
    let bytes = control.get_blob(parse_universe_id(&universe_id)?, parse_hex_hash(&sha256)?)?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}
