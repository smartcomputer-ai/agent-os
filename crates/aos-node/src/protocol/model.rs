use std::collections::BTreeMap;

use aos_cbor::Hash;
use aos_effects::ReceiptStatus;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::config::*;
use super::identity::*;

mod serde_bytes_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_bytes::{ByteBuf, Bytes};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(bytes) => serializer.serialize_some(Bytes::new(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<ByteBuf>::deserialize(deserializer).map(|opt| opt.map(|buf| buf.into_vec()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CborPayload {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub inline_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cbor_sha256: Option<String>,
}

impl CborPayload {
    pub fn inline(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            inline_cbor: Some(bytes.into()),
            cbor_ref: None,
            cbor_size: None,
            cbor_sha256: None,
        }
    }

    pub fn externalized(hash: Hash, size: u64) -> Self {
        Self {
            inline_cbor: None,
            cbor_ref: Some(hash.to_hex()),
            cbor_size: Some(size),
            cbor_sha256: Some(hash.to_hex()),
        }
    }

    pub fn inline_len(&self) -> usize {
        self.inline_cbor
            .as_ref()
            .map(|bytes| bytes.len())
            .unwrap_or(0)
    }

    pub fn validate(&self) -> Result<(), PersistError> {
        let has_inline = self.inline_cbor.is_some();
        let has_external =
            self.cbor_ref.is_some() || self.cbor_size.is_some() || self.cbor_sha256.is_some();
        if has_inline && has_external {
            return Err(PersistError::validation(
                "payload cannot contain both inline bytes and externalized metadata",
            ));
        }
        if !has_inline {
            match (&self.cbor_ref, self.cbor_size, &self.cbor_sha256) {
                (Some(_), Some(_), Some(_)) => {}
                (None, None, None) => {
                    return Err(PersistError::validation(
                        "payload must contain inline bytes or full externalized metadata",
                    ));
                }
                _ => {
                    return Err(PersistError::validation(
                        "externalized payload requires cbor_ref, cbor_size, and cbor_sha256",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldMeta {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_baseline_height: Option<JournalHeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_pin: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<WorldLineage>,
    #[serde(default)]
    pub admin: WorldAdminLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseMeta {
    pub handle: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldAdminStatus {
    #[default]
    Active,
    Pausing,
    Paused,
    Archiving,
    Archived,
    Deleting,
    Deleted,
}

impl WorldAdminStatus {
    pub fn accepts_direct_ingress(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn accepts_command_ingress(self) -> bool {
        !matches!(self, Self::Archived | Self::Deleted)
    }

    pub fn allows_new_leases(self) -> bool {
        !matches!(self, Self::Archived | Self::Deleted)
    }

    pub fn blocks_world_operations(self) -> bool {
        matches!(self, Self::Archived | Self::Deleted)
    }

    pub fn should_release_when_quiescent(self) -> bool {
        !matches!(self, Self::Active)
    }

    pub fn requires_maintenance_wakeup(self) -> bool {
        matches!(self, Self::Archiving | Self::Deleting)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorldAdminLifecycle {
    #[serde(default)]
    pub status: WorldAdminStatus,
    #[serde(default)]
    pub updated_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl WorldAdminLifecycle {
    pub fn new(status: WorldAdminStatus, updated_at_ns: u64) -> Self {
        Self {
            status,
            updated_at_ns,
            operation_id: None,
            reason: None,
        }
    }
}

/// Persisted provenance for how a hosted world came into existence.
///
/// This is stored alongside mutable world metadata, not as a CAS root. The
/// immutable replay roots remain the manifest and snapshot blobs referenced by
/// the active baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorldLineage {
    Genesis {
        created_at_ns: u64,
    },
    Import {
        created_at_ns: u64,
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        external_world_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        external_snapshot_ref: Option<String>,
    },
    Fork {
        forked_at_ns: u64,
        src_universe_id: UniverseId,
        src_world_id: WorldId,
        src_snapshot_ref: String,
        src_height: JournalHeight,
    },
}

/// Optional provenance attached to imported seeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedSeedSource {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_world_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_snapshot_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SeedKind {
    #[default]
    Genesis,
    Import,
}

/// A world seed is an admin-plane descriptor that points at already-uploaded
/// immutable CAS content. It is not itself stored in CAS by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSeed {
    pub baseline: SnapshotRecord,
    #[serde(default)]
    pub seed_kind: SeedKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportedSeedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseRecord {
    pub universe_id: UniverseId,
    #[serde(default)]
    pub created_at_ns: u64,
    pub meta: UniverseMeta,
    #[serde(default)]
    pub admin: UniverseAdminLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretBindingSourceKind {
    NodeSecretStore,
    WorkerEnv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecretBindingStatus {
    #[default]
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretBindingRecord {
    pub binding_id: String,
    pub source_kind: SecretBindingSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_placement_pin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<u64>,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default)]
    pub updated_at_ns: u64,
    #[serde(default)]
    pub status: SecretBindingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecretVersionStatus {
    #[default]
    Active,
    Superseded,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretVersionRecord {
    pub binding_id: String,
    pub version: u64,
    pub digest: String,
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub dek_wrapped: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub nonce: Vec<u8>,
    pub enc_alg: String,
    pub kek_id: String,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(default)]
    pub status: SecretVersionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutSecretVersionRequest {
    pub binding_id: String,
    pub digest: String,
    #[serde(with = "serde_bytes")]
    pub ciphertext: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub dek_wrapped: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub nonce: Vec<u8>,
    pub enc_alg: String,
    pub kek_id: String,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretAuditAction {
    BindingUpserted,
    BindingDisabled,
    VersionPut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretAuditRecord {
    #[serde(default)]
    pub ts_ns: u64,
    pub action: SecretAuditAction,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UniverseAdminStatus {
    #[default]
    Active,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UniverseAdminLifecycle {
    #[serde(default)]
    pub status: UniverseAdminStatus,
    #[serde(default)]
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreateUniverseRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universe_id: Option<UniverseId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseCreateResult {
    pub record: UniverseRecord,
}

/// Low-level request to create a new hosted world from an existing baseline snapshot.
///
/// The referenced snapshot and manifest must already exist in the universe CAS.
/// Creation persists mutable world metadata and baseline indexes; it does not
/// append runtime journal entries or acquire a lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorldSeedRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<WorldId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    pub seed: WorldSeed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_pin: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
}

/// Request to create a new hosted world.
///
/// `source.kind = "seed"` restores an already-materialized promoted baseline.
/// `source.kind = "manifest"` bootstraps a fresh hosted world from uploaded AIR
/// artifacts and lets the hosted node synthesize the first authoritative baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorldRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<WorldId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_pin: Option<String>,
    #[serde(default)]
    pub created_at_ns: u64,
    pub source: CreateWorldSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateWorldSource {
    Seed { seed: WorldSeed },
    Manifest { manifest_hash: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotSelector {
    ActiveBaseline,
    ByHeight { height: JournalHeight },
    ByRef { snapshot_ref: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ForkPendingEffectPolicy {
    #[default]
    ClearAllPendingExternalState,
}

/// Request to fork a new world from an existing world's snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkWorldRequest {
    pub src_world_id: WorldId,
    pub src_snapshot: SnapshotSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_world_id: Option<WorldId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_pin: Option<String>,
    #[serde(default)]
    pub forked_at_ns: u64,
    #[serde(default)]
    pub pending_effect_policy: ForkPendingEffectPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldRecord {
    pub world_id: WorldId,
    pub meta: WorldMeta,
    pub active_baseline: SnapshotRecord,
    pub journal_head: JournalHeight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldCreateResult {
    pub record: WorldRecord,
}

pub type WorldForkResult = WorldCreateResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHeartbeat {
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<String>,
    #[serde(default)]
    pub last_seen_ns: u64,
    #[serde(default)]
    pub expires_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldLease {
    pub holder_worker_id: String,
    pub epoch: u64,
    pub expires_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldRuntimeInfo {
    pub world_id: WorldId,
    pub meta: WorldMeta,
    #[serde(default)]
    pub notify_counter: u64,
    #[serde(default)]
    pub has_pending_inbox: bool,
    #[serde(default)]
    pub has_pending_effects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_timer_due_at_ns: Option<u64>,
    #[serde(default)]
    pub has_pending_maintenance: bool,
    #[serde(default)]
    pub lease: Option<WorldLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeWorldRuntimeInfo {
    pub universe_id: UniverseId,
    pub info: WorldRuntimeInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadProjectionRecord {
    pub journal_head: JournalHeight,
    pub manifest_hash: String,
    #[serde(default)]
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellStateProjectionRecord {
    pub journal_head: JournalHeight,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowCellStateProjection {
    pub workflow: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<CellStateProjectionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceVersionProjectionRecord {
    pub root_hash: String,
    pub owner: String,
    #[serde(default)]
    pub created_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRegistryProjectionRecord {
    pub journal_head: JournalHeight,
    pub workspace: String,
    pub latest_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<u64, WorkspaceVersionProjectionRecord>,
    #[serde(default)]
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryProjectionMaterialization {
    pub head: HeadProjectionRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<WorkflowCellStateProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceRegistryProjectionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellStateProjectionDelete {
    pub workflow: String,
    #[serde(with = "serde_bytes")]
    pub key_hash: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProjectionDelete {
    pub workspace: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryProjectionDelta {
    pub head: HeadProjectionRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_upserts: Vec<CellStateProjectionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cell_deletes: Vec<CellStateProjectionDelete>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_upserts: Vec<WorkspaceRegistryProjectionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_deletes: Vec<WorkspaceProjectionDelete>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReadyState {
    #[serde(default)]
    pub has_pending_inbox: bool,
    #[serde(default)]
    pub has_pending_effects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_timer_due_at_ns: Option<u64>,
    #[serde(default)]
    pub has_pending_maintenance: bool,
}

impl ReadyState {
    pub fn is_ready(&self) -> bool {
        self.has_pending_inbox
            || self.has_pending_effects
            || self.next_timer_due_at_ns.is_some()
            || self.has_pending_maintenance
    }

    pub fn priority(&self, now_ns: u64) -> u16 {
        if self.has_pending_inbox
            || self.has_pending_effects
            || self
                .next_timer_due_at_ns
                .is_some_and(|deliver_at_ns| deliver_at_ns <= now_ns)
        {
            0
        } else {
            1
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyHint {
    pub world_id: WorldId,
    pub priority: u16,
    #[serde(default)]
    pub ready_state: ReadyState,
    #[serde(default)]
    pub updated_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxItem {
    DomainEvent(DomainEventIngress),
    Receipt(ReceiptIngress),
    Inbox(ExternalInboxIngress),
    TimerFired(TimerFiredIngress),
    Control(CommandIngress),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEventIngress {
    pub schema: String,
    pub value: CborPayload,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub key: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptIngress {
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub effect_kind: String,
    pub adapter_id: String,
    pub status: ReceiptStatus,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cents: Option<u64>,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalInboxIngress {
    pub inbox_name: String,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerFiredIngress {
    pub timer_id: String,
    pub payload: CborPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandIngress {
    pub command_id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub payload: CborPayload,
    #[serde(default)]
    pub submitted_at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command_id: String,
    pub command: String,
    pub status: CommandStatus,
    #[serde(default)]
    pub submitted_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_payload: Option<CborPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub snapshot_ref: String,
    pub height: JournalHeight,
    #[serde(default)]
    pub logical_time_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_horizon_height: Option<JournalHeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCommitRequest {
    pub expected_head: JournalHeight,
    #[serde(with = "serde_bytes")]
    pub snapshot_bytes: Vec<u8>,
    pub record: SnapshotRecord,
    #[serde(with = "serde_bytes")]
    pub snapshot_journal_entry: Vec<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub baseline_journal_entry: Option<Vec<u8>>,
    #[serde(default)]
    pub promote_baseline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCommitResult {
    pub snapshot_hash: Hash,
    pub first_height: JournalHeight,
    pub next_head: JournalHeight,
    pub baseline_promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentIndexRecord {
    pub segment: SegmentId,
    pub body_ref: String,
    pub checksum: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentExportRequest {
    pub segment: SegmentId,
    pub hot_tail_margin: JournalHeight,
    pub delete_chunk_entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentExportResult {
    pub record: SegmentIndexRecord,
    pub exported_entries: u64,
    pub deleted_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinReason {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectDispatchItem {
    pub shard: ShardId,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub effect_kind: String,
    pub cap_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_bytes_opt"
    )]
    pub params_inline_cbor: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_sha256: Option<String>,
    #[serde(default, with = "serde_bytes")]
    pub idempotency_key: Vec<u8>,
    pub origin_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_context_hash: Option<String>,
    #[serde(default)]
    pub enqueued_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectInFlightItem {
    pub dispatch: EffectDispatchItem,
    #[serde(default)]
    pub claim_until_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Pending,
    InFlight,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectDedupeRecord {
    pub status: DispatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_after_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerDueItem {
    pub shard: ShardId,
    pub universe_id: UniverseId,
    pub world_id: WorldId,
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    pub time_bucket: TimeBucket,
    pub deliver_at_ns: u64,
    #[serde(with = "serde_bytes")]
    pub payload_cbor: Vec<u8>,
    #[serde(default)]
    pub enqueued_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerClaim {
    #[serde(with = "serde_bytes")]
    pub intent_hash: Vec<u8>,
    #[serde(default)]
    pub claim_until_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveredStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerDedupeRecord {
    pub status: DeliveredStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_after_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortalDedupeRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enqueued_seq: Option<InboxSeq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_after_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalSendStatus {
    Enqueued,
    AlreadyEnqueued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalSendResult {
    pub status: PortalSendStatus,
    pub enqueued_seq: Option<InboxSeq>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistError {
    #[error("conflict: {0}")]
    Conflict(#[from] PersistConflict),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("corruption: {0}")]
    Corrupt(#[from] PersistCorruption),
    #[error("backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistConflict {
    #[error("universe {universe_id} already exists")]
    UniverseExists { universe_id: UniverseId },
    #[error("universe handle '{handle}' is already assigned to universe {universe_id}")]
    UniverseHandleExists {
        handle: String,
        universe_id: UniverseId,
    },
    #[error(
        "universe {universe_id} is in admin status {status:?} and cannot perform action '{action}'"
    )]
    UniverseAdminBlocked {
        universe_id: UniverseId,
        status: UniverseAdminStatus,
        action: String,
    },
    #[error(
        "universe {universe_id} cannot be deleted while world {world_id} is in admin status {status:?}"
    )]
    UniverseDeleteBlockedByWorld {
        universe_id: UniverseId,
        world_id: WorldId,
        status: WorldAdminStatus,
    },
    #[error("world {world_id} already exists")]
    WorldExists { world_id: WorldId },
    #[error(
        "world handle '{handle}' is already assigned to world {world_id} in universe {universe_id}"
    )]
    WorldHandleExists {
        universe_id: UniverseId,
        handle: String,
        world_id: WorldId,
    },
    #[error("journal head advanced: expected {expected}, actual {actual}")]
    HeadAdvanced {
        expected: JournalHeight,
        actual: JournalHeight,
    },
    #[error("inbox cursor compare-and-swap failed: expected {expected:?}, actual {actual:?}")]
    InboxCursorAdvanced {
        expected: Option<InboxSeq>,
        actual: Option<InboxSeq>,
    },
    #[error("snapshot index at height {height} already exists")]
    SnapshotExists { height: JournalHeight },
    #[error("snapshot at height {height} differs from promotion record")]
    SnapshotMismatch { height: JournalHeight },
    #[error("baseline at height {height} already points at a different snapshot")]
    BaselineMismatch { height: JournalHeight },
    #[error("segment index for end height {end_height} already exists")]
    SegmentExists { end_height: JournalHeight },
    #[error(
        "world lease currently held by worker '{holder_worker_id}' at epoch {epoch} until {expires_at_ns}"
    )]
    LeaseHeld {
        holder_worker_id: String,
        epoch: u64,
        expires_at_ns: u64,
    },
    #[error(
        "world lease mismatch: expected worker '{expected_worker_id}' epoch {expected_epoch}, actual worker {actual_worker_id:?} epoch {actual_epoch:?}"
    )]
    LeaseMismatch {
        expected_worker_id: String,
        expected_epoch: u64,
        actual_worker_id: Option<String>,
        actual_epoch: Option<u64>,
    },
    #[error("world {world_id} is in admin status {status:?} and cannot perform action '{action}'")]
    WorldAdminBlocked {
        world_id: WorldId,
        status: WorldAdminStatus,
        action: String,
    },
    #[error("command {command_id} already exists with a different request payload")]
    CommandRequestMismatch { command_id: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersistCorruption {
    #[error("journal entry missing at height {height}")]
    MissingJournalEntry { height: JournalHeight },
    #[error("CAS body hash mismatch for {expected}: loaded {actual}")]
    CasBodyHashMismatch { expected: Hash, actual: Hash },
    #[error("published CAS chunk missing for {hash} at index {index}")]
    MissingCasChunk { hash: Hash, index: u32 },
    #[error("CAS size mismatch for {hash}: expected {expected}, loaded {actual}")]
    CasSizeMismatch {
        hash: Hash,
        expected: u64,
        actual: u64,
    },
    #[error("segment body missing from CAS for {segment:?} at hash {hash}")]
    MissingSegmentBody { segment: SegmentId, hash: Hash },
    #[error("segment checksum mismatch for {segment:?}: expected {expected}, loaded {actual}")]
    SegmentChecksumMismatch {
        segment: SegmentId,
        expected: String,
        actual: String,
    },
    #[error("malformed segment object for {segment:?}: {detail}")]
    MalformedSegment { segment: SegmentId, detail: String },
}

impl PersistError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend(message.into())
    }
}
