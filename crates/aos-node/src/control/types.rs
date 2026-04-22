use std::path::PathBuf;
use std::time::Duration;

use aos_air_types::{AirNode, Manifest};
use aos_effect_types::{
    HashRef, WorkspaceAnnotationsGetReceipt, WorkspaceAnnotationsPatch, WorkspaceDiffReceipt,
    WorkspaceListReceipt, WorkspaceReadRefReceipt, WorkspaceResolveReceipt,
};
use aos_kernel::{DefListing, KernelError, StoreError};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::IntoParams;

use crate::{
    CborPayload, CommandStatus, CreateWorldSource, ForkPendingEffectPolicy, PersistError,
    SecretBindingSourceKind, SecretBindingStatus, SnapshotRecord, SnapshotSelector, UniverseId,
    WorldId, WorldRuntimeInfo,
};

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
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

impl ControlError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::NotImplemented(message.into())
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::Timeout(message.into())
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
            ControlError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            ControlError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeadInfoResponse {
    pub journal_head: u64,
    pub retained_from: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorldPageQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorldSummaryResponse {
    pub runtime: WorldRuntimeInfo,
    pub active_baseline: SnapshotRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateCellSummary {
    pub journal_head: u64,
    pub workflow: String,
    #[serde(with = "serde_bytes")]
    pub key_hash: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub key_bytes: Vec<u8>,
    pub state_hash: String,
    pub size: u64,
    #[serde(default)]
    pub last_active_ns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestResponse {
    pub journal_head: u64,
    pub manifest_hash: String,
    pub summary: ManifestSummary,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestSummary {
    pub schema_count: usize,
    pub module_count: usize,
    pub workflow_count: usize,
    pub effect_count: usize,
    pub secret_count: usize,
    pub routing_subscription_count: usize,
    pub routes: Vec<RouteSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteSummary {
    pub event: String,
    pub workflow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field: Option<String>,
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
    pub cell: Option<StateCellSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateListResponse {
    pub journal_head: u64,
    pub workflow: String,
    pub cells: Vec<StateCellSummary>,
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
    pub retained_from: u64,
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
    pub retained_from: u64,
    pub next_from: u64,
    pub entries: Vec<RawJournalEntryResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandSubmitResponse {
    pub command_id: String,
    pub status: CommandStatus,
    pub poll_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AcceptWaitQuery {
    #[serde(default)]
    pub wait_for_flush: bool,
    #[serde(default)]
    pub wait_timeout_ms: Option<u64>,
}

impl AcceptWaitQuery {
    pub fn timeout(&self) -> Option<Duration> {
        self.wait_timeout_ms.map(Duration::from_millis)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceResolveResponse {
    pub workspace: String,
    #[serde(flatten)]
    pub receipt: WorkspaceResolveReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitEventBody {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_json: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_world_epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorldBody {
    #[serde(default)]
    pub world_id: Option<WorldId>,
    #[serde(default)]
    pub universe_id: UniverseId,
    #[serde(default)]
    pub created_at_ns: u64,
    pub source: CreateWorldSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkWorldBody {
    pub src_snapshot: SnapshotSelector,
    #[serde(default)]
    pub new_world_id: Option<WorldId>,
    #[serde(default)]
    pub forked_at_ns: u64,
    #[serde(default)]
    pub pending_effect_policy: ForkPendingEffectPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSubmitBody<T> {
    #[serde(default)]
    pub command_id: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(flatten)]
    pub params: T,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DefsQuery {
    #[serde(default)]
    pub kinds: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StateGetQuery {
    #[serde(default)]
    pub key_b64: Option<String>,
    #[serde(default)]
    pub consistency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LimitQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub consistency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceResolveQuery {
    pub workspace: String,
    #[serde(default)]
    pub version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JournalQuery {
    #[serde(default)]
    pub from: u64,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceEntriesQuery {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_workspace_limit")]
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceEntryQuery {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceBytesQuery {
    pub path: String,
    #[serde(default)]
    pub start: Option<u64>,
    #[serde(default)]
    pub end: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WorkspaceAnnotationsQuery {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceDiffBody {
    pub root_a: HashRef,
    pub root_b: HashRef,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TraceQuery {
    #[serde(default)]
    pub event_hash: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub correlate_by: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub window_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TraceSummaryQuery {
    #[serde(default = "default_trace_summary_recent_limit")]
    pub recent_limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UniverseQuery {
    #[serde(default)]
    pub universe_id: Option<UniverseId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertSecretBindingBody {
    pub source_kind: SecretBindingSourceKind,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub required_placement_pin: Option<String>,
    #[serde(default)]
    pub status: SecretBindingStatus,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutSecretVersionBody {
    pub plaintext_b64: String,
    #[serde(default)]
    pub expected_digest: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceReadResponse {
    pub receipt: WorkspaceReadRefReceipt,
    pub payload: CborPayload,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceListResponse {
    #[serde(flatten)]
    pub receipt: WorkspaceListReceipt,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceAnnotationsResponse {
    #[serde(flatten)]
    pub receipt: WorkspaceAnnotationsGetReceipt,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDiffResponse {
    #[serde(flatten)]
    pub receipt: WorkspaceDiffReceipt,
}

pub fn default_limit() -> u32 {
    100
}

pub fn default_workspace_limit() -> u64 {
    100
}

pub fn default_trace_summary_recent_limit() -> u32 {
    10
}
