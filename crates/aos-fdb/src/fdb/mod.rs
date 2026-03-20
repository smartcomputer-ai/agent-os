use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use foundationdb::Transaction;
use foundationdb::options::MutationType;
use foundationdb::tuple::{Subspace, Versionstamp};
use foundationdb::{
    Database, FdbBindingError, FdbError, KeySelector, MaybeCommitted, RangeOption,
    RetryableTransaction,
};
use futures::executor::block_on;
use uuid::Uuid;

use crate::cas::{CachingCasStore, FdbCasStore};
use crate::fork_snapshot::rewrite_snapshot_for_fork_policy;
use crate::projection::{materialization_from_snapshot, state_blobs_from_snapshot};
use crate::segment::{
    decode_segment_entries, encode_segment_entries, segment_checksum,
    validate_segment_export_request,
};
use aos_node::{
    CasStore, CborPayload, CellStateProjectionRecord, CommandIngress, CommandRecord, CommandStore,
    CreateUniverseRequest, CreateWorldSeedRequest, DeliveredStatus, DispatchStatus,
    EffectDedupeRecord, EffectDispatchItem, EffectInFlightItem, ForkWorldRequest,
    HeadProjectionRecord, HostedCoordinationStore, HostedEffectQueueStore, HostedPortalStore,
    HostedTimerQueueStore, InboxItem, InboxSeq, JournalHeight, NodeCatalog, NodeWorldRuntimeInfo,
    PersistConflict, PersistCorruption, PersistError, PersistenceConfig, PortalDedupeRecord,
    PortalSendResult, PortalSendStatus, ProjectionStore, PutSecretVersionRequest,
    QueryProjectionDelta, QueryProjectionMaterialization, QueueSeq, ReadyHint, ReadyState,
    ReceiptIngress, SecretAuditRecord, SecretBindingRecord, SecretStore, SecretVersionRecord,
    SegmentExportRequest, SegmentExportResult, SegmentId, SegmentIndexRecord, ShardId,
    SnapshotCommitRequest, SnapshotCommitResult, SnapshotRecord, SnapshotSelector, TimerClaim,
    TimerDedupeRecord, TimerDueItem, UniverseAdminLifecycle, UniverseAdminStatus,
    UniverseCreateResult, UniverseId, UniverseRecord, UniverseStore, WorkerHeartbeat,
    WorkspaceRegistryProjectionRecord, WorldAdminLifecycle, WorldAdminStore, WorldCreateResult,
    WorldForkResult, WorldId, WorldIngressStore, WorldLease, WorldLineage, WorldMeta, WorldRecord,
    WorldRuntimeInfo, WorldSeed, WorldStore, can_upgrade_snapshot_record, default_universe_handle,
    default_world_handle, gc_bucket_for, maintenance_due, normalize_handle, sample_world_meta,
    validate_baseline_promotion_record, validate_create_world_seed_request,
    validate_fork_world_request, validate_query_projection_delta,
    validate_query_projection_materialization, validate_snapshot_commit_request,
    validate_snapshot_record,
};

mod admin;
mod catalog;
mod hosted;
mod state;
mod world;

pub use self::state::{FdbRuntime, FdbWorldPersistence};
use self::state::{FdbTimerInFlightItem, StoredCommandRecord};

impl FdbRuntime {
    pub fn boot() -> Result<Self, PersistError> {
        let network = unsafe { foundationdb::boot() };
        Ok(Self { _network: network })
    }
}

mod runtime;
mod support;
fn build_inbox_key(space: &Subspace, seq: &InboxSeq) -> Vec<u8> {
    let mut key = space.bytes().to_vec();
    key.extend_from_slice(seq.as_bytes());
    key
}

fn custom_persist_error(err: PersistError) -> FdbBindingError {
    FdbBindingError::CustomError(Box::new(err))
}

fn map_fdb_binding_error(err: FdbBindingError) -> PersistError {
    match err {
        FdbBindingError::CustomError(inner) => match inner.downcast::<PersistError>() {
            Ok(persist) => *persist,
            Err(other) => PersistError::backend(other.to_string()),
        },
        other => PersistError::backend(other.to_string()),
    }
}

fn map_fdb_error(err: FdbError) -> PersistError {
    PersistError::backend(err.to_string())
}

enum TxRetryError {
    Fdb(FdbError),
    Persist(PersistError),
}

fn decode_u64_static(bytes: &[u8]) -> Result<u64, FdbBindingError> {
    let array: [u8; 8] = bytes.try_into().map_err(|_| {
        custom_persist_error(PersistError::backend("expected 8-byte integer value"))
    })?;
    Ok(u64::from_be_bytes(array))
}

fn to_i64_static(value: u64, field: &str) -> Result<i64, FdbBindingError> {
    i64::try_from(value).map_err(|_| {
        custom_persist_error(PersistError::validation(format!(
            "{field} value {value} exceeds i64 tuple encoding"
        )))
    })
}

fn from_i64_static(value: i64, field: &str) -> Result<u64, FdbBindingError> {
    u64::try_from(value).map_err(|_| {
        custom_persist_error(PersistError::backend(format!(
            "{field} tuple value {value} is negative"
        )))
    })
}
