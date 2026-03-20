use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct MemoryWorldPersistence {
    pub(super) state: Arc<Mutex<MemoryState>>,
    pub(super) cas: MemoryCasStore,
    pub(super) config: PersistenceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MemoryPersistenceSnapshot {
    pub(super) state: MemoryState,
    #[serde(with = "serde_bytes")]
    pub(super) cas_state: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct MemoryState {
    pub(super) universes: BTreeMap<UniverseId, UniverseRecord>,
    pub(super) universe_handles: BTreeMap<String, UniverseId>,
    pub(super) secret_bindings: BTreeMap<UniverseId, BTreeMap<String, SecretBindingRecord>>,
    pub(super) secret_versions: BTreeMap<UniverseId, BTreeMap<(String, u64), SecretVersionRecord>>,
    pub(super) secret_audit: BTreeMap<UniverseId, BTreeMap<(u64, String, u64), SecretAuditRecord>>,
    pub(super) workers: BTreeMap<String, WorkerHeartbeat>,
    pub(super) worlds: BTreeMap<(UniverseId, WorldId), WorldState>,
    pub(super) world_handles: BTreeMap<UniverseId, BTreeMap<String, WorldId>>,
    pub(super) ready_hints: BTreeMap<(u16, u16, UniverseId, WorldId), ReadyHint>,
    pub(super) lease_by_worker: BTreeMap<(String, UniverseId, WorldId), WorldLease>,
    pub(super) effect_seq_by_shard: BTreeMap<UniverseId, BTreeMap<u16, u64>>,
    pub(super) effects_pending: BTreeMap<UniverseId, BTreeMap<(u16, QueueSeq), EffectDispatchItem>>,
    pub(super) effects_inflight:
        BTreeMap<UniverseId, BTreeMap<(u16, QueueSeq), EffectInFlightItem>>,
    pub(super) effects_dedupe: BTreeMap<UniverseId, BTreeMap<Vec<u8>, EffectDedupeRecord>>,
    pub(super) effects_dedupe_gc: BTreeMap<UniverseId, BTreeMap<(u64, Vec<u8>), ()>>,
    pub(super) timers_due: BTreeMap<UniverseId, BTreeMap<(u16, u64, u64, Vec<u8>), TimerDueItem>>,
    pub(super) timers_inflight: BTreeMap<UniverseId, BTreeMap<Vec<u8>, MemoryTimerInFlightItem>>,
    pub(super) timers_dedupe: BTreeMap<UniverseId, BTreeMap<Vec<u8>, TimerDedupeRecord>>,
    pub(super) timers_dedupe_gc: BTreeMap<UniverseId, BTreeMap<(u64, Vec<u8>), ()>>,
    pub(super) portal_dedupe:
        BTreeMap<(UniverseId, WorldId), BTreeMap<Vec<u8>, PortalDedupeRecord>>,
    pub(super) portal_dedupe_gc: BTreeMap<UniverseId, BTreeMap<(u64, WorldId, Vec<u8>), ()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WorldState {
    pub(super) meta: WorldMeta,
    pub(super) journal_head: JournalHeight,
    pub(super) journal_entries: BTreeMap<JournalHeight, Vec<u8>>,
    pub(super) inbox_entries: BTreeMap<InboxSeq, InboxItem>,
    pub(super) inbox_cursor: Option<InboxSeq>,
    pub(super) next_inbox_seq: u64,
    pub(super) command_records: BTreeMap<String, StoredCommandRecord>,
    pub(super) snapshots: BTreeMap<JournalHeight, SnapshotRecord>,
    pub(super) active_baseline: Option<SnapshotRecord>,
    pub(super) segments: BTreeMap<JournalHeight, SegmentIndexRecord>,
    pub(super) notify_counter: u64,
    pub(super) lease: Option<WorldLease>,
    pub(super) pending_effects_count: u64,
    pub(super) next_timer_due_at_ns: Option<u64>,
    pub(super) ready_state: ReadyState,
    pub(super) head_projection: Option<HeadProjectionRecord>,
    pub(super) cell_state_projections:
        BTreeMap<String, BTreeMap<Vec<u8>, CellStateProjectionRecord>>,
    pub(super) workspace_projections: BTreeMap<String, WorkspaceRegistryProjectionRecord>,
}

impl Default for WorldState {
    fn default() -> Self {
        Self {
            meta: sample_world_meta(WorldId::from(Uuid::nil())),
            journal_head: 0,
            journal_entries: BTreeMap::new(),
            inbox_entries: BTreeMap::new(),
            inbox_cursor: None,
            next_inbox_seq: 0,
            command_records: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            active_baseline: None,
            segments: BTreeMap::new(),
            notify_counter: 0,
            lease: None,
            pending_effects_count: 0,
            next_timer_due_at_ns: None,
            ready_state: ReadyState::default(),
            head_projection: None,
            cell_state_projections: BTreeMap::new(),
            workspace_projections: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MemoryTimerInFlightItem {
    pub(super) due: TimerDueItem,
    pub(super) claim: TimerClaim,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredCommandRecord {
    pub(super) record: CommandRecord,
    pub(super) request_hash: String,
}
