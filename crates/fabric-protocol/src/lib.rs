use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ExecId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct HostId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct RequestId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerInfoResponse {
    pub controller_version: String,
    pub db_path: String,
    pub heartbeat_timeout_ns: u128,
    pub default_session_ttl_ns: Option<u128>,
    pub auth_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostInfoResponse {
    pub host_id: HostId,
    pub daemon_version: String,
    pub runtime_kind: String,
    #[serde(default)]
    pub runtime_version: Option<String>,
    pub state_root: String,
    pub bind_addr: String,
    pub default_network_mode: NetworkMode,
    pub allowed_network_modes: Vec<NetworkMode>,
    pub resource_defaults: ResourceLimits,
    pub resource_max: ResourceLimits,
    pub allowed_images: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostInventoryResponse {
    pub host_id: HostId,
    pub sessions: Vec<HostInventorySession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostInventorySession {
    pub session_id: SessionId,
    pub status: SessionStatus,
    #[serde(default)]
    pub machine_name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub network_mode: Option<NetworkMode>,
    #[serde(default)]
    pub runtime_present: bool,
    #[serde(default)]
    pub workspace_present: bool,
    #[serde(default)]
    pub marker_present: bool,
    #[serde(default)]
    pub created_at_ns: Option<u128>,
    #[serde(default)]
    pub expires_at_ns: Option<u128>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerSessionOpenRequest {
    #[serde(default)]
    pub request_id: Option<RequestId>,
    pub target: FabricSessionTarget,
    #[serde(default)]
    pub ttl_ns: Option<u128>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerSessionOpenResponse {
    pub session_id: SessionId,
    pub status: ControllerSessionStatus,
    pub target_kind: FabricSessionTargetKind,
    pub host_id: HostId,
    pub host_session_id: SessionId,
    pub workdir: String,
    #[serde(default = "default_supported_session_signals")]
    pub supported_signals: Vec<FabricSessionSignalKind>,
    pub created_at_ns: u128,
    #[serde(default)]
    pub expires_at_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerSessionSummary {
    pub session_id: SessionId,
    pub status: ControllerSessionStatus,
    pub target_kind: FabricSessionTargetKind,
    pub host_id: HostId,
    pub host_session_id: SessionId,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default = "default_supported_session_signals")]
    pub supported_signals: Vec<FabricSessionSignalKind>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub created_at_ns: u128,
    pub updated_at_ns: u128,
    #[serde(default)]
    pub expires_at_ns: Option<u128>,
    #[serde(default)]
    pub closed_at_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerSessionListResponse {
    pub sessions: Vec<ControllerSessionSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct SessionLabelsPatchRequest {
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionLabelsResponse {
    pub session_id: SessionId,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSessionStatus {
    Creating,
    Ready,
    Quiesced,
    Closing,
    Closed,
    Error,
    Lost,
    HostUnreachable,
    OrphanedHostSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FabricSessionTargetKind {
    Sandbox,
    AttachedHost,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricSessionTarget {
    Sandbox(FabricSandboxTarget),
    AttachedHost(FabricAttachedHostTarget),
}

impl FabricSessionTarget {
    pub fn kind(&self) -> FabricSessionTargetKind {
        match self {
            Self::Sandbox(_) => FabricSessionTargetKind::Sandbox,
            Self::AttachedHost(_) => FabricSessionTargetKind::AttachedHost,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FabricSandboxTarget {
    pub image: String,
    #[serde(default)]
    pub runtime_class: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub network_mode: NetworkMode,
    #[serde(default)]
    pub mounts: Vec<MountSpec>,
    #[serde(default)]
    pub resources: ResourceLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FabricAttachedHostTarget {
    pub selector: HostSelector,
    pub workspace_policy: AttachedWorkspacePolicy,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum HostSelector {
    HostId(HostId),
    Pool(String),
    Labels(BTreeMap<String, String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum AttachedWorkspacePolicy {
    ExistingPath { path: String },
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerExecRequest {
    #[serde(default)]
    pub request_id: Option<RequestId>,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env_patch: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin: Option<ExecStdin>,
    #[serde(default)]
    pub timeout_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricBytes {
    Text(String),
    Base64(String),
}

impl FabricBytes {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn from_bytes_auto(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::Text(text),
            Err(error) => Self::Base64(BASE64_STANDARD.encode(error.into_bytes())),
        }
    }

    pub fn from_bytes_base64(bytes: &[u8]) -> Self {
        Self::Base64(BASE64_STANDARD.encode(bytes))
    }

    pub fn decode_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Text(text) => Ok(text.as_bytes().to_vec()),
            Self::Base64(encoded) => BASE64_STANDARD
                .decode(encoded)
                .map_err(|error| format!("invalid base64 content: {error}")),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Base64(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum ExecStdin {
    Text(String),
    Base64(String),
}

impl From<ExecStdin> for FabricBytes {
    fn from(value: ExecStdin) -> Self {
        match value {
            ExecStdin::Text(text) => Self::Text(text),
            ExecStdin::Base64(encoded) => Self::Base64(encoded),
        }
    }
}

impl From<FabricBytes> for ExecStdin {
    fn from(value: FabricBytes) -> Self {
        match value {
            FabricBytes::Text(text) => Self::Text(text),
            FabricBytes::Base64(encoded) => Self::Base64(encoded),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ControllerSignalSessionRequest {
    #[serde(default)]
    pub request_id: Option<RequestId>,
    pub signal: FabricSessionSignal,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricSessionSignal {
    Quiesce(QuiesceSignal),
    Resume(ResumeSignal),
    Close(CloseSignal),
    TerminateRuntime(TerminateRuntimeSignal),
}

impl FabricSessionSignal {
    pub fn kind(&self) -> FabricSessionSignalKind {
        match self {
            Self::Quiesce(_) => FabricSessionSignalKind::Quiesce,
            Self::Resume(_) => FabricSessionSignalKind::Resume,
            Self::Close(_) => FabricSessionSignalKind::Close,
            Self::TerminateRuntime(_) => FabricSessionSignalKind::TerminateRuntime,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FabricSessionSignalKind {
    Quiesce,
    Resume,
    Close,
    TerminateRuntime,
}

fn default_supported_session_signals() -> Vec<FabricSessionSignalKind> {
    vec![
        FabricSessionSignalKind::Quiesce,
        FabricSessionSignalKind::Resume,
        FabricSessionSignalKind::Close,
    ]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct QuiesceSignal {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ResumeSignal {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct CloseSignal {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct TerminateRuntimeSignal {}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "spec", rename_all = "snake_case")]
pub enum FabricHostProvider {
    Smolvm(SmolvmProviderInfo),
    AttachedHost(AttachedHostProviderInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SmolvmProviderInfo {
    #[serde(default)]
    pub runtime_version: Option<String>,
    #[serde(default)]
    pub supported_runtime_classes: Vec<String>,
    #[serde(default)]
    pub allowed_images: Vec<String>,
    #[serde(default)]
    pub allowed_network_modes: Vec<NetworkMode>,
    #[serde(default)]
    pub resource_defaults: ResourceLimits,
    #[serde(default)]
    pub resource_max: ResourceLimits,
    #[serde(default)]
    pub capacity: ProviderCapacity,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AttachedHostProviderInfo {
    pub selector: HostSelector,
    #[serde(default)]
    pub workspace_policies: Vec<AttachedWorkspacePolicy>,
    #[serde(default)]
    pub capacity: ProviderCapacity,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ProviderCapacity {
    #[serde(default)]
    pub max_sessions: Option<u64>,
    #[serde(default)]
    pub active_sessions: u64,
    #[serde(default)]
    pub max_concurrent_execs: Option<u64>,
    #[serde(default)]
    pub active_execs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostRegisterRequest {
    pub host_id: HostId,
    pub endpoint: String,
    #[serde(default)]
    pub providers: Vec<FabricHostProvider>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostRegisterResponse {
    pub host_id: HostId,
    pub status: HostStatus,
    pub heartbeat_interval_ns: u128,
    pub controller_time_ns: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostHeartbeatRequest {
    pub host_id: HostId,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub providers: Vec<FabricHostProvider>,
    #[serde(default)]
    pub inventory: Option<HostInventoryResponse>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostSummary {
    pub host_id: HostId,
    pub endpoint: String,
    pub status: HostStatus,
    pub providers: Vec<FabricHostProvider>,
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub last_heartbeat_ns: Option<u128>,
    pub created_at_ns: u128,
    pub updated_at_ns: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HostListResponse {
    pub hosts: Vec<HostSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionOpenRequest {
    #[serde(default)]
    pub session_id: Option<SessionId>,
    pub image: String,
    #[serde(default)]
    pub runtime_class: Option<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub network_mode: NetworkMode,
    #[serde(default)]
    pub mounts: Vec<MountSpec>,
    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionOpenResponse {
    pub session_id: SessionId,
    pub status: SessionStatus,
    pub workdir: String,
    #[serde(default)]
    pub host_id: Option<HostId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionStatusResponse {
    pub session_id: SessionId,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Disabled,
    Egress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Creating,
    Ready,
    Quiesced,
    Closing,
    Closed,
    OrphanedWorkspace,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ResourceLimits {
    #[serde(default)]
    pub cpu_limit_millis: Option<u64>,
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MountSpec {
    pub host_path: String,
    pub guest_path: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExecRequest {
    pub session_id: SessionId,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin: Option<FabricBytes>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExecEvent {
    pub exec_id: ExecId,
    pub seq: u64,
    pub kind: ExecEventKind,
    #[serde(default)]
    pub data: Option<FabricBytes>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecEventKind {
    Started,
    Stdout,
    Stderr,
    Exit,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SignalSessionRequest {
    pub action: SessionSignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionSignal {
    Quiesce,
    Resume,
    Terminate,
    Close,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsPathQuery {
    pub path: String,
    #[serde(default)]
    pub offset_bytes: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsFileReadResponse {
    pub path: String,
    pub content: FabricBytes,
    pub offset_bytes: u64,
    pub bytes_read: u64,
    pub size_bytes: u64,
    pub truncated: bool,
    #[serde(default)]
    pub mtime_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsFileWriteRequest {
    pub path: String,
    pub content: FabricBytes,
    #[serde(default)]
    pub create_parents: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsWriteResponse {
    pub path: String,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsEditFileRequest {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsEditFileResponse {
    pub path: String,
    pub replacements: u64,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsApplyPatchRequest {
    pub patch: String,
    #[serde(default)]
    pub patch_format: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsApplyPatchResponse {
    pub files_changed: u64,
    pub changed_paths: Vec<String>,
    pub ops: FsPatchOpsSummary,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsPatchOpsSummary {
    pub add: u64,
    pub update: u64,
    pub delete: u64,
    #[serde(rename = "move")]
    pub move_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsMkdirRequest {
    pub path: String,
    #[serde(default)]
    pub parents: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsRemoveRequest {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsRemoveResponse {
    pub path: String,
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsExistsResponse {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsStatResponse {
    pub path: String,
    pub kind: FsEntryKind,
    pub size_bytes: u64,
    pub readonly: bool,
    #[serde(default)]
    pub mtime_ns: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsListDirResponse {
    pub path: String,
    pub entries: Vec<FsDirEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsGrepRequest {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub glob_filter: Option<String>,
    #[serde(default)]
    pub max_results: Option<u64>,
    #[serde(default)]
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsGrepResponse {
    pub matches: Vec<FsGrepMatch>,
    pub match_count: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsGrepMatch {
    pub path: String,
    pub line_number: u64,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsGlobRequest {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_results: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsGlobResponse {
    pub paths: Vec<String>,
    pub count: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FsDirEntry {
    pub name: String,
    pub path: String,
    pub kind: FsEntryKind,
    pub size_bytes: u64,
    pub readonly: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FsEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_target_uses_tagged_sum_shape() {
        let target = FabricSessionTarget::Sandbox(FabricSandboxTarget {
            image: "alpine:latest".to_owned(),
            runtime_class: Some("smolvm".to_owned()),
            workdir: None,
            env: BTreeMap::new(),
            network_mode: NetworkMode::Egress,
            mounts: Vec::new(),
            resources: ResourceLimits::default(),
        });

        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["kind"], "sandbox");
        assert_eq!(json["spec"]["image"], "alpine:latest");

        let decoded: FabricSessionTarget = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, FabricSessionTarget::Sandbox(_)));
    }

    #[test]
    fn host_provider_uses_tagged_sum_shape() {
        let provider = FabricHostProvider::Smolvm(SmolvmProviderInfo {
            runtime_version: None,
            supported_runtime_classes: vec!["smolvm".to_owned()],
            allowed_images: vec!["*".to_owned()],
            allowed_network_modes: vec![NetworkMode::Disabled, NetworkMode::Egress],
            resource_defaults: ResourceLimits::default(),
            resource_max: ResourceLimits::default(),
            capacity: ProviderCapacity::default(),
        });

        let json = serde_json::to_value(&provider).unwrap();
        assert_eq!(json["kind"], "smolvm");
        assert_eq!(json["spec"]["supported_runtime_classes"][0], "smolvm");

        let decoded: FabricHostProvider = serde_json::from_value(json).unwrap();
        assert!(matches!(decoded, FabricHostProvider::Smolvm(_)));
    }

    #[test]
    fn selector_and_signal_use_tagged_sum_shape() {
        let selector =
            HostSelector::Labels(BTreeMap::from([("pool".to_owned(), "dev".to_owned())]));
        let selector_json = serde_json::to_value(&selector).unwrap();
        assert_eq!(selector_json["kind"], "labels");
        assert_eq!(selector_json["spec"]["pool"], "dev");

        let signal = FabricSessionSignal::Close(CloseSignal {});
        let signal_json = serde_json::to_value(&signal).unwrap();
        assert_eq!(signal_json["kind"], "close");
        assert_eq!(signal_json["spec"], serde_json::json!({}));
    }

    #[test]
    fn fabric_bytes_preserves_text_and_binary_payloads() {
        let text = FabricBytes::from_bytes_auto(b"hello\n".to_vec());
        assert_eq!(text.as_text(), Some("hello\n"));
        assert_eq!(text.decode_bytes().unwrap(), b"hello\n");

        let binary_bytes = vec![0, 159, 255, b'\n'];
        let binary = FabricBytes::from_bytes_auto(binary_bytes.clone());
        assert!(matches!(binary, FabricBytes::Base64(_)));
        assert_eq!(binary.decode_bytes().unwrap(), binary_bytes);

        let explicit_binary = FabricBytes::from_bytes_base64(&binary_bytes);
        assert!(matches!(explicit_binary, FabricBytes::Base64(_)));
        assert_eq!(explicit_binary.decode_bytes().unwrap(), binary_bytes);
    }
}
