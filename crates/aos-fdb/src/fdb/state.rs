use super::*;

pub struct FdbRuntime {
    pub(super) _network: foundationdb::api::NetworkAutoStop,
}

pub struct FdbWorldPersistence {
    pub(super) _runtime: Arc<FdbRuntime>,
    pub(super) db: Arc<Database>,
    pub(super) cas: CachingCasStore<FdbCasStore>,
    pub(super) config: PersistenceConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct FdbTimerInFlightItem {
    pub(super) due: TimerDueItem,
    pub(super) claim: TimerClaim,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredCommandRecord {
    pub(super) record: CommandRecord,
    pub(super) request_hash: String,
}
