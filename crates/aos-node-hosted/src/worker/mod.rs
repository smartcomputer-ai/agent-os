mod leased;
mod maintenance;
mod runner;
mod supervisor;
#[cfg(test)]
mod tests;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use aos_cbor::{HASH_PREFIX, Hash};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effect_types::{
    GovApplyParams, GovApplyReceipt, GovApproveParams, GovApproveReceipt, GovDecision,
    GovLedgerChange, GovLedgerDelta, GovLedgerKind, GovModuleEffectAllowlist, GovPatchInput,
    GovPendingWorkflowReceipt, GovPredictedEffect, GovProposeParams, GovProposeReceipt,
    GovShadowParams, GovShadowReceipt, GovWorkflowInstancePreview, HashRef,
};
use aos_effects::builtins::{
    PortalSendMode, PortalSendParams, PortalSendReceipt, TimerSetParams, TimerSetReceipt,
};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_fdb::{
    CborPayload, CellStateProjectionDelete, CellStateProjectionRecord, CommandErrorBody,
    CommandIngress, CommandStatus, DomainEventIngress, EffectDispatchItem, HeadProjectionRecord,
    HostedRuntimeStore, InboxItem, NodeWorldRuntimeInfo, PersistConflict, PersistError,
    PortalSendStatus, QueryProjectionDelta, ReceiptIngress, SecretStore, SegmentExportRequest,
    TimerDueItem, UniverseId, UniverseStore, WorkerHeartbeat, WorkspaceProjectionDelete,
    WorkspaceRegistryProjectionRecord, WorkspaceVersionProjectionRecord, WorldAdminLifecycle,
    WorldAdminStatus, WorldId, WorldLease, WorldRuntimeInfo, WorldStore,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{
    CellProjectionDelta, CellProjectionDeltaState, KernelConfig, KernelError, StateReader,
};
use aos_kernel::{Store, StoreError};
use aos_node::{
    HotWorld, HotWorldError, SharedBlobCache, apply_ingress_item_to_hot_world,
    encode_ingress_as_journal_entry, parse_hash_ref as shared_parse_hash_ref,
    parse_intent_hash as shared_parse_intent_hash,
    resolve_cbor_payload as shared_resolve_cbor_payload,
};
use aos_runtime::{HostError, WorldConfig, now_wallclock_ns};
use serde::Deserialize;
use thiserror::Error;
use tokio::runtime::Builder;

use self::leased::LeasedWorldPersistence;
use self::maintenance::MaintenanceScheduler;
use crate::config;
use crate::secret::{HostedSecretConfig, HostedSecretResolver};

const TIMER_BUCKET_NS: u64 = 1_000_000_000;
#[cfg(test)]
const SYS_TIMER_FIRED_SCHEMA: &str = "sys/TimerFired@1";
const CMD_GOV_PROPOSE: &str = "gov-propose";
const CMD_GOV_SHADOW: &str = "gov-shadow";
const CMD_GOV_APPROVE: &str = "gov-approve";
const CMD_GOV_APPLY: &str = "gov-apply";
const CMD_WORLD_PAUSE: &str = "world-pause";
const CMD_WORLD_ARCHIVE: &str = "world-archive";
const CMD_WORLD_DELETE: &str = "world-delete";

fn open_keepalive_interval(lease_ttl: Duration, lease_renew_interval: Duration) -> Duration {
    let ttl_half = Duration::from_nanos((lease_ttl.as_nanos() / 2).max(1) as u64);
    lease_renew_interval
        .min(ttl_half)
        .max(Duration::from_millis(250))
}

fn join_open_keepalive(
    handle: thread::JoinHandle<Result<(), PersistError>>,
) -> Result<(), WorkerError> {
    match handle.join() {
        Ok(result) => result.map_err(WorkerError::from),
        Err(_) => Err(WorkerError::Runtime(std::io::Error::other(
            "lease keepalive thread panicked",
        ))),
    }
}

fn spawn_world_keepalive<P>(
    worker: &FdbWorker,
    runtime: Arc<P>,
    universe: UniverseId,
    world: WorldId,
    lease_cell: Arc<Mutex<WorldLease>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<(), PersistError>>
where
    P: HostedRuntimeStore + SecretStore + 'static,
{
    let keepalive_runtime = Arc::clone(&runtime);
    let keepalive_lease = Arc::clone(&lease_cell);
    let keepalive_interval =
        open_keepalive_interval(worker.config.lease_ttl, worker.config.lease_renew_interval);
    let heartbeat_interval = open_keepalive_interval(
        worker.config.heartbeat_ttl,
        worker.config.lease_renew_interval,
    );
    let lease_ttl_ns = duration_ns(worker.config.lease_ttl);
    let heartbeat_ttl_ns = duration_ns(worker.config.heartbeat_ttl);
    let keepalive_worker_id = worker.config.worker_id.clone();
    let keepalive_worker_pins: Vec<String> = worker.config.worker_pins.iter().cloned().collect();
    thread::spawn(move || {
        let mut last_attempt = Instant::now();
        let mut last_heartbeat = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if last_heartbeat.elapsed() >= heartbeat_interval {
                let now_ns = now_wallclock_ns();
                keepalive_runtime.heartbeat_worker(WorkerHeartbeat {
                    worker_id: keepalive_worker_id.clone(),
                    pins: keepalive_worker_pins.clone(),
                    last_seen_ns: now_ns,
                    expires_at_ns: now_ns.saturating_add(heartbeat_ttl_ns),
                })?;
                last_heartbeat = Instant::now();
            }
            if last_attempt.elapsed() >= keepalive_interval {
                let current = keepalive_lease
                    .lock()
                    .map(|lease| lease.clone())
                    .map_err(|_| PersistError::backend("world lease mutex poisoned"))?;
                let renewed = keepalive_runtime.renew_world_lease(
                    universe,
                    world,
                    &current,
                    now_wallclock_ns(),
                    lease_ttl_ns,
                )?;
                *keepalive_lease
                    .lock()
                    .map_err(|_| PersistError::backend("world lease mutex poisoned"))? = renewed;
                last_attempt = Instant::now();
            }
            thread::sleep(Duration::from_millis(100));
        }
    })
}

#[derive(Debug, Deserialize, Default)]
struct WorkspaceHistoryState {
    latest: u64,
    versions: BTreeMap<u64, WorkspaceCommitMetaState>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceCommitMetaState {
    root_hash: String,
    owner: String,
    created_at: u64,
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    HotWorld(#[from] HotWorldError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
    #[error("unsupported hosted inbox item '{0}'")]
    UnsupportedInboxItem(&'static str),
    #[error("invalid idempotency key length {0}")]
    InvalidIdempotencyKeyLen(usize),
    #[error("invalid universe id '{0}'")]
    InvalidUniverseId(String),
    #[error("invalid world id '{0}'")]
    InvalidWorldId(String),
    #[error("portal.send missing required field '{0}'")]
    InvalidPortalParams(&'static str),
    #[error("unsupported portal delivery mode '{0}'")]
    UnsupportedPortalMode(&'static str),
}

fn is_world_isolatable_error(err: &WorkerError) -> bool {
    matches!(
        err,
        WorkerError::Host(_)
            | WorkerError::Kernel(_)
            | WorkerError::Cbor(_)
            | WorkerError::Json(_)
            | WorkerError::Store(_)
            | WorkerError::HotWorld(_)
            | WorkerError::UnsupportedInboxItem(_)
            | WorkerError::InvalidIdempotencyKeyLen(_)
            | WorkerError::InvalidUniverseId(_)
            | WorkerError::InvalidWorldId(_)
            | WorkerError::InvalidPortalParams(_)
            | WorkerError::UnsupportedPortalMode(_)
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SupervisorOutcome {
    pub worlds_started: usize,
    pub worlds_released: usize,
    pub worlds_fenced: usize,
    pub active_worlds: usize,
}

#[derive(Clone)]
pub struct FdbWorker {
    pub config: config::FdbWorkerConfig,
    pub world_config: WorldConfig,
    pub adapter_config: EffectAdapterConfig,
    pub kernel_config: KernelConfig,
    hosted_blob_cache: SharedBlobCache,
}

impl FdbWorker {
    pub fn new(config: config::FdbWorkerConfig) -> Self {
        let entry_limit = config
            .cas_cache_bytes
            .saturating_div(config.cas_cache_item_max_bytes.max(1))
            .clamp(1024, 65_536);
        let shared_cache = SharedBlobCache::new(
            entry_limit,
            config.cas_cache_bytes,
            config.cas_cache_item_max_bytes,
        );
        let world_config = WorldConfig::from_env();
        Self {
            config,
            world_config,
            adapter_config: EffectAdapterConfig::from_env(),
            kernel_config: KernelConfig::default(),
            hosted_blob_cache: shared_cache,
        }
    }

    pub fn with_runtime<P>(&self, runtime: Arc<P>) -> WorkerSupervisor<P>
    where
        P: HostedRuntimeStore + SecretStore + UniverseStore + 'static,
    {
        WorkerSupervisor {
            worker: self.clone(),
            runtime,
            maintenance: MaintenanceScheduler::default(),
            active_worlds: HashMap::new(),
            warm_worlds: HashMap::new(),
            faulted_worlds: HashMap::new(),
        }
    }

    pub fn with_runtime_for_universes<P, I>(
        &self,
        runtime: Arc<P>,
        universes: I,
    ) -> WorkerSupervisor<P>
    where
        P: HostedRuntimeStore + SecretStore + UniverseStore + 'static,
        I: IntoIterator<Item = UniverseId>,
    {
        let mut worker = self.clone();
        worker.config.universe_filter = universes.into_iter().collect();
        WorkerSupervisor {
            worker,
            runtime,
            maintenance: MaintenanceScheduler::default(),
            active_worlds: HashMap::new(),
            warm_worlds: HashMap::new(),
            faulted_worlds: HashMap::new(),
        }
    }

    pub fn open_world<P>(
        &self,
        persistence: Arc<dyn WorldStore>,
        secret_persistence: Arc<P>,
        universe: UniverseId,
        world: WorldId,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        mut kernel_config: KernelConfig,
    ) -> Result<HotWorld, HostError>
    where
        P: SecretStore + 'static,
    {
        if kernel_config.secret_resolver.is_none() {
            let config = HostedSecretConfig::from_env()
                .map_err(|err| HostError::External(format!("hosted secret config: {err}")))?;
            kernel_config.secret_resolver = Some(Arc::new(HostedSecretResolver::new(
                secret_persistence,
                universe,
                config,
            )));
        }
        let hot = HotWorld::open(
            persistence,
            universe,
            world,
            world_config,
            adapter_config,
            kernel_config,
            Some(self.hosted_blob_cache.clone()),
        )?;
        Ok(hot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActiveWorldRef {
    pub universe_id: UniverseId,
    pub world_id: WorldId,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveWorldDebugState {
    pub pending_receipt_intent_hashes: Vec<String>,
    pub pending_receipts: Vec<PendingReceiptDebugState>,
    pub queued_effect_intent_hashes: Vec<String>,
    pub queued_effects: Vec<QueuedEffectDebugState>,
    pub workflow_instances: Vec<ActiveWorkflowDebugState>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveWorkflowDebugState {
    pub instance_id: String,
    pub inflight_intent_hashes: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingReceiptDebugState {
    pub intent_hash: String,
    pub origin_module_id: String,
    pub origin_instance_id: String,
    pub effect_kind: String,
    pub emitted_at_seq: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QueuedEffectDebugState {
    pub intent_hash: String,
    pub effect_kind: String,
    pub cap_name: String,
}

pub struct WorkerSupervisor<P> {
    worker: FdbWorker,
    runtime: Arc<P>,
    maintenance: MaintenanceScheduler,
    active_worlds: HashMap<ActiveWorldRef, WorldRunner<P>>,
    warm_worlds: HashMap<ActiveWorldRef, WorldRunner<P>>,
    faulted_worlds: HashMap<ActiveWorldRef, FaultedWorldState>,
}

#[derive(Debug, Clone)]
struct FaultedWorldState {
    attempts: u32,
    next_retry_ns: u64,
}

struct WorldRunner<P> {
    worker: FdbWorker,
    runtime: Arc<P>,
    universe: UniverseId,
    world: WorldId,
    lease: Option<WorldLease>,
    lease_cell: Arc<Mutex<WorldLease>>,
    keepalive_stop: Arc<AtomicBool>,
    keepalive_handle: Option<thread::JoinHandle<Result<(), PersistError>>>,
    host: HotWorld,
    idle_since_ns: Option<u64>,
    suspended_since_ns: Option<u64>,
    last_renew_ns: u64,
    last_materialized_head: Option<u64>,
}

enum RunnerStep {
    KeepRunning,
    Released(ReleaseDisposition),
    Fenced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseDisposition {
    Warm,
    Drop,
}

#[derive(Debug, Default)]
struct ControlCommandOutcome {
    journal_height: Option<u64>,
    manifest_hash: Option<String>,
    result_payload: Option<CborPayload>,
}

#[derive(Debug, Default, Deserialize, serde::Serialize)]
struct LifecycleCommandParams {
    #[serde(default)]
    reason: Option<String>,
}
fn resolve_payload(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    payload: &CborPayload,
) -> Result<Vec<u8>, WorkerError> {
    shared_resolve_cbor_payload(persistence, universe, payload).map_err(WorkerError::from)
}

fn resolve_dispatch_params(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    item: &EffectDispatchItem,
) -> Result<Vec<u8>, WorkerError> {
    let payload = CborPayload {
        inline_cbor: item.params_inline_cbor.clone(),
        cbor_ref: item.params_ref.clone(),
        cbor_size: item.params_size,
        cbor_sha256: item.params_sha256.clone(),
    };
    resolve_payload(persistence, universe, &payload)
}

fn parse_intent_hash(bytes: &[u8]) -> Result<[u8; 32], WorkerError> {
    shared_parse_intent_hash(bytes).map_err(WorkerError::from)
}

fn parse_idempotency_key(bytes: &[u8]) -> Result<[u8; 32], WorkerError> {
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| WorkerError::InvalidIdempotencyKeyLen(len))
}

fn receipt_to_ingress(effect_kind: String, receipt: EffectReceipt) -> ReceiptIngress {
    ReceiptIngress {
        intent_hash: receipt.intent_hash.to_vec(),
        effect_kind,
        adapter_id: receipt.adapter_id,
        status: receipt.status,
        payload: CborPayload::inline(receipt.payload_cbor),
        cost_cents: receipt.cost_cents,
        signature: receipt.signature,
        correlation_id: None,
    }
}

fn command_success_outcome<T: serde::Serialize>(
    value: &T,
) -> Result<ControlCommandOutcome, WorkerError> {
    Ok(ControlCommandOutcome {
        result_payload: Some(CborPayload::inline(serde_cbor::to_vec(value)?)),
        ..ControlCommandOutcome::default()
    })
}

fn command_error_body(err: &WorkerError) -> CommandErrorBody {
    let code = match err {
        WorkerError::Persist(PersistError::NotFound(_)) => "not_found",
        WorkerError::Persist(PersistError::Conflict(_)) => "conflict",
        WorkerError::Persist(PersistError::Validation(_)) => "validation_failed",
        WorkerError::Kernel(KernelError::ProposalNotFound(_)) => "not_found",
        WorkerError::Kernel(KernelError::ProposalAlreadyApplied(_))
        | WorkerError::Kernel(KernelError::ProposalStateInvalid { .. })
        | WorkerError::Kernel(KernelError::ManifestApplyBlockedInFlight { .. }) => "conflict",
        WorkerError::Json(_) | WorkerError::Cbor(_) => "invalid_request",
        _ => "command_failed",
    };
    CommandErrorBody {
        code: code.into(),
        message: err.to_string(),
    }
}

fn hash_ref_from_hex(hex: &str) -> Result<HashRef, WorkerError> {
    let value = if hex.starts_with(HASH_PREFIX) {
        hex.to_string()
    } else {
        format!("{HASH_PREFIX}{hex}")
    };
    HashRef::new(value).map_err(|_| WorkerError::from(HotWorldError::InvalidHash(hex.into())))
}

fn parse_hash_ref(value: &str) -> Result<Hash, WorkerError> {
    shared_parse_hash_ref(value).map_err(WorkerError::from)
}

fn next_admin_lifecycle(
    current: &WorldAdminLifecycle,
    world_id: WorldId,
    target_status: WorldAdminStatus,
    command_id: &str,
    reason: Option<String>,
    now_ns: u64,
) -> Result<WorldAdminLifecycle, WorkerError> {
    let same_operation = current.operation_id.as_deref() == Some(command_id);
    let already_done = matches!(
        (target_status, current.status),
        (WorldAdminStatus::Pausing, WorldAdminStatus::Paused)
            | (WorldAdminStatus::Archiving, WorldAdminStatus::Archived)
            | (WorldAdminStatus::Deleting, WorldAdminStatus::Deleted)
    );
    if same_operation && (current.status == target_status || already_done) {
        return Ok(current.clone());
    }

    let allowed = match target_status {
        WorldAdminStatus::Pausing => matches!(current.status, WorldAdminStatus::Active),
        WorldAdminStatus::Archiving => matches!(
            current.status,
            WorldAdminStatus::Active
                | WorldAdminStatus::Paused
                | WorldAdminStatus::Pausing
                | WorldAdminStatus::Archived
        ),
        WorldAdminStatus::Deleting => !matches!(current.status, WorldAdminStatus::Deleted),
        _ => false,
    };
    if !allowed {
        return Err(PersistError::Conflict(PersistConflict::WorldAdminBlocked {
            world_id,
            status: current.status,
            action: format!("transition to {target_status:?}"),
        })
        .into());
    }

    Ok(WorldAdminLifecycle {
        status: target_status,
        updated_at_ns: now_ns,
        operation_id: Some(command_id.into()),
        reason,
    })
}

fn finalize_quiescent_admin(current: &WorldAdminLifecycle) -> Option<WorldAdminLifecycle> {
    let status = match current.status {
        WorldAdminStatus::Pausing => WorldAdminStatus::Paused,
        WorldAdminStatus::Archiving => WorldAdminStatus::Archived,
        WorldAdminStatus::Deleting => WorldAdminStatus::Deleted,
        _ => return None,
    };
    let mut admin = current.clone();
    admin.status = status;
    admin.updated_at_ns = now_wallclock_ns();
    Some(admin)
}

fn parse_universe_id(value: &str) -> Result<UniverseId, WorkerError> {
    UniverseId::from_str(value).map_err(|_| WorkerError::InvalidUniverseId(value.to_string()))
}

fn parse_world_id(value: &str) -> Result<WorldId, WorkerError> {
    WorldId::from_str(value).map_err(|_| WorkerError::InvalidWorldId(value.to_string()))
}

fn rendezvous_score(universe_id: UniverseId, world_id: WorldId, worker_id: &str) -> u64 {
    let bytes = format!("{worker_id}:{universe_id}:{world_id}").into_bytes();
    let hash = Hash::of_bytes(&bytes);
    u64::from_be_bytes(hash.as_bytes()[0..8].try_into().expect("8-byte score"))
}

fn shard_for_hash(intent_hash: &[u8; 32], shard_count: u32) -> u16 {
    let shard_count = shard_count.max(1);
    (u32::from_be_bytes(intent_hash[0..4].try_into().expect("4-byte shard hash")) % shard_count)
        as u16
}

fn time_bucket_for(deliver_at_ns: u64) -> u64 {
    deliver_at_ns / TIMER_BUCKET_NS
}

fn effective_world_pin(info: &WorldRuntimeInfo) -> String {
    info.meta
        .placement_pin
        .clone()
        .unwrap_or_else(|| "default".into())
}

fn worker_is_eligible_for_pin(worker: &WorkerHeartbeat, pin: &str) -> bool {
    worker.pins.iter().any(|candidate| candidate == pin)
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    duration.as_nanos() as u64
}
