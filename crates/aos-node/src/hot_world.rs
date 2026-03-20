use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use aos_cbor::{HASH_PREFIX, Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_effects::EffectReceipt;
use aos_kernel::Store;
use aos_kernel::journal::{Journal, JournalRecord, OwnedJournalEntry};
use aos_kernel::{KernelConfig, KernelError, LoadedManifest};
use aos_runtime::{
    ExternalEvent, HostError, JournalReplayOpen, RunMode, TimerScheduler, WorldConfig, WorldHost,
};
use aos_wasm_abi::DomainEvent;
use thiserror::Error;

use crate::hosted::{HostedStore, SharedBlobCache, open_hosted_world, snapshot_hosted_world};
use crate::{CborPayload, PersistError, UniverseId, WorldId, WorldStore};

pub use aos_runtime::JournalReplayOpen as HotWorldReplayOpen;

const SYS_TIMER_FIRED_SCHEMA: &str = "sys/TimerFired@1";

#[derive(Debug, Error)]
pub enum HotWorldError {
    #[error(transparent)]
    Persist(#[from] PersistError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error(transparent)]
    Cbor(#[from] serde_cbor::Error),
    #[error("invalid hash reference '{0}'")]
    InvalidHash(String),
    #[error("receipt intent hash must be 32 bytes, got {0}")]
    InvalidIntentHashLen(usize),
    #[error("unsupported hot-world ingress item '{0}'")]
    UnsupportedIngressItem(&'static str),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HotWorldDrainOutcome {
    pub effects_dispatched: usize,
    pub receipts_applied: usize,
    pub timers_fired: usize,
}

impl HotWorldDrainOutcome {
    pub fn progressed(self) -> bool {
        self.effects_dispatched > 0 || self.receipts_applied > 0 || self.timers_fired > 0
    }
}

pub struct HotWorld {
    pub host: WorldHost<HostedStore>,
    pub scheduler: TimerScheduler,
}

impl HotWorld {
    pub fn open(
        persistence: Arc<dyn WorldStore>,
        universe: UniverseId,
        world: WorldId,
        world_config: WorldConfig,
        adapter_config: EffectAdapterConfig,
        kernel_config: KernelConfig,
        shared_cache: Option<SharedBlobCache>,
    ) -> Result<Self, HostError> {
        let host = open_hosted_world(
            persistence,
            universe,
            world,
            world_config,
            adapter_config,
            kernel_config,
            shared_cache,
        )?;
        Ok(Self::from_host(host))
    }

    pub fn from_host(host: WorldHost<HostedStore>) -> Self {
        let mut scheduler = TimerScheduler::new();
        scheduler.rehydrate_from_pending(&host.kernel().pending_workflow_receipts_snapshot());
        Self { host, scheduler }
    }

    pub fn next_timer_due_at_ns(&self) -> Option<u64> {
        self.scheduler.next_due_at_ns()
    }

    pub async fn run_daemon_until_quiescent(&mut self) -> Result<HotWorldDrainOutcome, HostError> {
        let mut outcome = HotWorldDrainOutcome::default();
        loop {
            let cycle = self
                .host
                .run_cycle(RunMode::Daemon {
                    scheduler: &mut self.scheduler,
                })
                .await?;
            let timers_fired = self.host.fire_due_timers(&mut self.scheduler)?;
            outcome.effects_dispatched += cycle.effects_dispatched;
            outcome.receipts_applied += cycle.receipts_applied;
            outcome.timers_fired += timers_fired;
            if cycle.effects_dispatched == 0 && cycle.receipts_applied == 0 && timers_fired == 0 {
                break;
            }
        }
        Ok(outcome)
    }

    pub fn snapshot(
        &mut self,
        persistence: Arc<dyn WorldStore>,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), HostError> {
        snapshot_hosted_world(&mut self.host, &persistence, universe, world)
    }
}

impl Deref for HotWorld {
    type Target = WorldHost<HostedStore>;

    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

impl DerefMut for HotWorld {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.host
    }
}

pub fn open_hot_world<S: Store + 'static>(
    store: Arc<S>,
    loaded: LoadedManifest,
    journal: Box<dyn Journal>,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: KernelConfig,
    replay: Option<JournalReplayOpen>,
) -> Result<WorldHost<S>, HostError> {
    WorldHost::from_loaded_manifest_with_journal_replay(
        store,
        loaded,
        journal,
        world_config,
        adapter_config,
        kernel_config,
        replay,
    )
}

pub fn apply_ingress_item_to_hot_world(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    hot: &mut HotWorld,
    item: crate::InboxItem,
) -> Result<(), HotWorldError> {
    match item {
        crate::InboxItem::DomainEvent(event) => {
            let value = resolve_cbor_payload(persistence, universe, &event.value)?;
            hot.host
                .enqueue_external_without_journal(ExternalEvent::DomainEvent {
                    schema: event.schema,
                    value,
                    key: event.key,
                })?;
            Ok(())
        }
        crate::InboxItem::Receipt(receipt) => {
            let payload_cbor = resolve_cbor_payload(persistence, universe, &receipt.payload)?;
            hot.host
                .enqueue_external_without_journal(ExternalEvent::Receipt(EffectReceipt {
                    intent_hash: parse_intent_hash(&receipt.intent_hash)?,
                    adapter_id: receipt.adapter_id,
                    status: receipt.status,
                    payload_cbor,
                    cost_cents: receipt.cost_cents,
                    signature: receipt.signature,
                }))?;
            Ok(())
        }
        crate::InboxItem::TimerFired(timer) => {
            let value = resolve_cbor_payload(persistence, universe, &timer.payload)?;
            hot.host
                .enqueue_external_without_journal(ExternalEvent::DomainEvent {
                    schema: SYS_TIMER_FIRED_SCHEMA.into(),
                    value,
                    key: None,
                })?;
            Ok(())
        }
        crate::InboxItem::Inbox(_) => Err(HotWorldError::UnsupportedIngressItem("inbox")),
        crate::InboxItem::Control(_) => Err(HotWorldError::UnsupportedIngressItem("control")),
    }
}

pub fn encode_ingress_as_journal_entry(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    hot: &mut HotWorld,
    journal_seq: u64,
    item: crate::InboxItem,
) -> Result<Vec<u8>, HotWorldError> {
    match item {
        crate::InboxItem::DomainEvent(event) => {
            let value = resolve_cbor_payload(persistence, universe, &event.value)?;
            let event = DomainEvent {
                schema: event.schema,
                value,
                key: event.key,
            };
            let stamp = hot.host.kernel_mut().sample_ingress(journal_seq)?;
            let record = JournalRecord::DomainEvent(
                hot.host
                    .kernel()
                    .build_domain_event_record(&event, &stamp)?,
            );
            encode_journal_entry(journal_seq, record)
        }
        crate::InboxItem::Receipt(receipt) => {
            let payload_cbor = resolve_cbor_payload(persistence, universe, &receipt.payload)?;
            let stamp = hot.host.kernel_mut().sample_ingress(journal_seq)?;
            let record =
                JournalRecord::EffectReceipt(hot.host.kernel().build_effect_receipt_record(
                    &EffectReceipt {
                        intent_hash: parse_intent_hash(&receipt.intent_hash)?,
                        adapter_id: receipt.adapter_id,
                        status: receipt.status,
                        payload_cbor,
                        cost_cents: receipt.cost_cents,
                        signature: receipt.signature,
                    },
                    &stamp,
                )?);
            encode_journal_entry(journal_seq, record)
        }
        crate::InboxItem::TimerFired(timer) => {
            let value = resolve_cbor_payload(persistence, universe, &timer.payload)?;
            let event = DomainEvent {
                schema: SYS_TIMER_FIRED_SCHEMA.into(),
                value,
                key: None,
            };
            let stamp = hot.host.kernel_mut().sample_ingress(journal_seq)?;
            let record = JournalRecord::DomainEvent(
                hot.host
                    .kernel()
                    .build_domain_event_record(&event, &stamp)?,
            );
            encode_journal_entry(journal_seq, record)
        }
        crate::InboxItem::Inbox(_) => Err(HotWorldError::UnsupportedIngressItem("inbox")),
        crate::InboxItem::Control(_) => Err(HotWorldError::UnsupportedIngressItem("control")),
    }
}

pub fn resolve_cbor_payload(
    persistence: &dyn WorldStore,
    universe: UniverseId,
    payload: &CborPayload,
) -> Result<Vec<u8>, HotWorldError> {
    payload.validate()?;
    if let Some(inline) = &payload.inline_cbor {
        return Ok(inline.clone());
    }
    let Some(hash_ref) = payload.cbor_ref.as_deref() else {
        return Err(HotWorldError::InvalidHash("<missing>".into()));
    };
    let hash = parse_hash_ref(hash_ref)?;
    Ok(persistence.cas_get(universe, hash)?)
}

pub fn encode_journal_entry(seq: u64, record: JournalRecord) -> Result<Vec<u8>, HotWorldError> {
    let payload = serde_cbor::to_vec(&record)?;
    Ok(to_canonical_cbor(&OwnedJournalEntry {
        seq,
        kind: record.kind(),
        payload,
    })?)
}

pub fn parse_intent_hash(bytes: &[u8]) -> Result<[u8; 32], HotWorldError> {
    bytes
        .try_into()
        .map_err(|_| HotWorldError::InvalidIntentHashLen(bytes.len()))
}

pub fn parse_hash_ref(value: &str) -> Result<Hash, HotWorldError> {
    let normalized = if value.starts_with(HASH_PREFIX) {
        value.to_string()
    } else {
        format!("{HASH_PREFIX}{value}")
    };
    Hash::from_hex_str(&normalized).map_err(|_| HotWorldError::InvalidHash(value.to_string()))
}
