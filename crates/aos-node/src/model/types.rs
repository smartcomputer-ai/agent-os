use std::path::PathBuf;

use aos_cbor::Hash;
use aos_effects::ReceiptStatus;
use aos_kernel::StoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::identity::*;

pub type JournalHeight = u64;

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

/// Request to create a new world.
///
/// `source.kind = "seed"` restores an already-materialized promoted baseline.
/// `source.kind = "manifest"` bootstraps a fresh world from uploaded AIR
/// artifacts and lets the node synthesize the first authoritative baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorldRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<WorldId>,
    #[serde(default)]
    pub universe_id: UniverseId,
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
    #[serde(default)]
    pub forked_at_ns: u64,
    #[serde(default)]
    pub pending_effect_policy: ForkPendingEffectPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldRecord {
    pub world_id: WorldId,
    #[serde(default)]
    pub universe_id: UniverseId,
    #[serde(default)]
    pub created_at_ns: u64,
    pub manifest_hash: String,
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
pub struct WorldRuntimeInfo {
    pub world_id: WorldId,
    #[serde(default)]
    pub universe_id: UniverseId,
    #[serde(default)]
    pub created_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_baseline_height: Option<JournalHeight>,
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
    pub effect_op: String,
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
    pub universe_id: UniverseId,
    #[serde(default)]
    pub logical_time_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_horizon_height: Option<JournalHeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
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
    #[error("world {world_id} already exists")]
    WorldExists { world_id: WorldId },
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

    pub fn into_store_error(
        self,
        default_path: impl Into<PathBuf>,
        not_found_message: &'static str,
    ) -> StoreError {
        let default_path = default_path.into();
        match self {
            PersistError::NotFound(message) => StoreError::Io {
                path: PathBuf::from(message),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, not_found_message),
            },
            PersistError::Backend(message) => StoreError::Io {
                path: default_path,
                source: std::io::Error::other(message),
            },
            PersistError::Conflict(message) => StoreError::Io {
                path: default_path,
                source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, message),
            },
            PersistError::Validation(message) => StoreError::Io {
                path: default_path,
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message),
            },
            PersistError::Corrupt(message) => StoreError::Io {
                path: default_path,
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_string()),
            },
        }
    }
}
