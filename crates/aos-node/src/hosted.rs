use std::cmp::min;
use std::collections::{HashMap, VecDeque};
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use aos_cbor::{HASH_PREFIX, Hash, to_canonical_cbor};
use aos_effect_adapters::config::EffectAdapterConfig;
use aos_kernel::ManifestLoader;
use aos_kernel::StateReader;
use aos_kernel::journal::{Journal, JournalEntry, JournalError, JournalSeq, OwnedJournalEntry};
use aos_kernel::{Store, StoreError, StoreResult};
use aos_runtime::{HostError, JournalReplayOpen, WorldConfig, WorldHost};
use serde::{Serialize, de::DeserializeOwned};

use crate::{PersistError, SnapshotRecord, UniverseId, WorldId, WorldStore};

const HOT_READ_CHUNK_LIMIT: u32 = 512;
const BLOB_CACHE_ENTRY_LIMIT: usize = 8192;
const BLOB_CACHE_MAX_TOTAL_BYTES: usize = 1_048_576;
const BLOB_CACHE_MAX_ITEM_BYTES: usize = 1_048_576;

#[derive(Clone, Debug)]
pub struct SharedBlobCache {
    inner: Arc<Mutex<BlobCache>>,
}

impl SharedBlobCache {
    pub fn new(entry_limit: usize, max_total_bytes: usize, max_item_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BlobCache::new(
                entry_limit,
                max_total_bytes,
                max_item_bytes,
            ))),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BlobCache> {
        self.inner.lock().expect("hosted blob cache poisoned")
    }
}

pub struct HostedStore {
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    blob_cache: SharedBlobCache,
    stats: HostedStoreStats,
}

impl HostedStore {
    pub fn new(persistence: Arc<dyn WorldStore>, universe: UniverseId) -> Self {
        Self::with_shared_cache(
            persistence,
            universe,
            SharedBlobCache::new(
                BLOB_CACHE_ENTRY_LIMIT,
                BLOB_CACHE_MAX_TOTAL_BYTES,
                BLOB_CACHE_MAX_ITEM_BYTES,
            ),
        )
    }

    pub fn with_shared_cache(
        persistence: Arc<dyn WorldStore>,
        universe: UniverseId,
        blob_cache: SharedBlobCache,
    ) -> Self {
        Self {
            persistence,
            universe,
            blob_cache,
            stats: HostedStoreStats::default(),
        }
    }

    pub fn universe(&self) -> UniverseId {
        self.universe
    }

    pub fn stats_snapshot(&self) -> HostedStoreStatsSnapshot {
        self.stats.snapshot()
    }
}

impl Store for HostedStore {
    fn put_node<T: Serialize>(&self, value: &T) -> StoreResult<Hash> {
        let bytes = to_canonical_cbor(value)?;
        self.put_blob(&bytes)
    }

    fn get_node<T: DeserializeOwned>(&self, hash: Hash) -> StoreResult<T> {
        let bytes = self.get_blob(hash)?;
        serde_cbor::from_slice(&bytes).map_err(StoreError::from)
    }

    fn has_node(&self, hash: Hash) -> StoreResult<bool> {
        self.has_blob(hash)
    }

    fn put_blob(&self, bytes: &[u8]) -> StoreResult<Hash> {
        self.persistence
            .cas_put_verified(self.universe, bytes)
            .map_err(|err| persist_error_to_store_error(cas_path(hashless_hex(bytes)), err))
    }

    fn get_blob(&self, hash: Hash) -> StoreResult<Vec<u8>> {
        if let Some(bytes) = self.blob_cache.lock().get(hash) {
            self.stats.record_hit();
            return Ok(bytes);
        }
        let started = Instant::now();
        self.persistence
            .cas_get(self.universe, hash)
            .map(|bytes| {
                self.stats.record_miss(started.elapsed().as_millis() as u64);
                self.blob_cache.lock().insert(hash, &bytes);
                bytes
            })
            .map_err(|err| persist_error_to_store_error(cas_path(hash.to_hex()), err))
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        self.persistence
            .cas_has(self.universe, hash)
            .map_err(|err| persist_error_to_store_error(cas_path(hash.to_hex()), err))
    }
}

pub struct HostedJournal {
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
    next_seq: JournalSeq,
}

impl HostedJournal {
    pub fn open(
        persistence: Arc<dyn WorldStore>,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Self, JournalError> {
        let next_seq = persistence
            .journal_head(universe, world)
            .map_err(persist_error_to_journal_error)?;
        Ok(Self {
            persistence,
            universe,
            world,
            next_seq,
        })
    }

    fn decode_entry(
        &self,
        raw: &[u8],
        expected_seq: JournalSeq,
    ) -> Result<OwnedJournalEntry, JournalError> {
        let entry: OwnedJournalEntry = serde_cbor::from_slice(raw)
            .map_err(|err| JournalError::Corrupt(format!("decode hosted journal entry: {err}")))?;
        if entry.seq != expected_seq {
            return Err(JournalError::Corrupt(format!(
                "hosted journal entry sequence mismatch: expected {expected_seq}, found {}",
                entry.seq
            )));
        }
        Ok(entry)
    }

    fn load_hot_entries(
        &self,
        start: JournalSeq,
        end_exclusive: JournalSeq,
        out: &mut Vec<OwnedJournalEntry>,
    ) -> Result<(), JournalError> {
        let mut cursor = start;
        while cursor < end_exclusive {
            let remaining = end_exclusive - cursor;
            let limit = min(remaining, HOT_READ_CHUNK_LIMIT as u64) as u32;
            let batch = self
                .persistence
                .journal_read_range(self.universe, self.world, cursor, limit)
                .map_err(persist_error_to_journal_error)?;
            if batch.is_empty() {
                return Err(JournalError::Corrupt(format!(
                    "hosted journal gap while reading hot range at {cursor}"
                )));
            }
            for (height, raw) in batch {
                if height != cursor {
                    return Err(JournalError::Corrupt(format!(
                        "hosted journal range returned non-contiguous height {height} at cursor {cursor}"
                    )));
                }
                out.push(self.decode_entry(&raw, height)?);
                cursor += 1;
                if cursor >= end_exclusive {
                    break;
                }
            }
        }
        Ok(())
    }

    fn load_range(
        &self,
        from: JournalSeq,
        end_exclusive: JournalSeq,
    ) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        if from >= end_exclusive {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let mut cursor = from;
        let segment_limit = min(
            u32::MAX as usize,
            end_exclusive.saturating_sub(from) as usize,
        ) as u32;
        let mut segments = self
            .persistence
            .segment_index_read_from(self.universe, self.world, from, segment_limit)
            .map_err(persist_error_to_journal_error)?;
        segments.sort_by_key(|record| record.segment.start);

        for record in segments {
            if record.segment.end < cursor {
                continue;
            }
            if record.segment.start > cursor {
                self.load_hot_entries(cursor, min(record.segment.start, end_exclusive), &mut out)?;
                cursor = record.segment.start;
            }
            if cursor >= end_exclusive {
                break;
            }

            let entries = self
                .persistence
                .segment_read_entries(self.universe, self.world, record.segment)
                .map_err(persist_error_to_journal_error)?;
            for (height, raw) in entries {
                if height < cursor {
                    continue;
                }
                if height >= end_exclusive {
                    break;
                }
                out.push(self.decode_entry(&raw, height)?);
                cursor = height + 1;
            }
        }

        if cursor < end_exclusive {
            self.load_hot_entries(cursor, end_exclusive, &mut out)?;
        }
        Ok(out)
    }
}

impl Journal for HostedJournal {
    fn append(&mut self, entry: JournalEntry<'_>) -> Result<JournalSeq, JournalError> {
        let seq = self.next_seq;
        let raw = to_canonical_cbor(&OwnedJournalEntry {
            seq,
            kind: entry.kind,
            payload: entry.payload.to_vec(),
        })?;
        let first = self
            .persistence
            .journal_append_batch(self.universe, self.world, self.next_seq, &[raw])
            .map_err(persist_error_to_journal_error)?;
        if first != seq {
            return Err(JournalError::Corrupt(format!(
                "hosted journal append returned first height {first}, expected {seq}"
            )));
        }
        self.next_seq += 1;
        Ok(seq)
    }

    fn load_from(&self, from: JournalSeq) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        let head = self
            .persistence
            .journal_head(self.universe, self.world)
            .map_err(persist_error_to_journal_error)?;
        self.load_range(from, head)
    }

    fn load_batch_from(
        &self,
        from: JournalSeq,
        limit: usize,
    ) -> Result<Vec<OwnedJournalEntry>, JournalError> {
        let head = self
            .persistence
            .journal_head(self.universe, self.world)
            .map_err(persist_error_to_journal_error)?;
        let end_exclusive = min(head, from.saturating_add(limit as u64));
        self.load_range(from, end_exclusive)
    }

    fn next_seq(&self) -> JournalSeq {
        self.next_seq
    }

    fn set_next_seq(&mut self, next_seq: JournalSeq) {
        self.next_seq = next_seq;
    }
}

pub fn open_hosted_world(
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: aos_kernel::KernelConfig,
    shared_cache: Option<SharedBlobCache>,
) -> Result<WorldHost<HostedStore>, HostError> {
    let active = persistence
        .snapshot_active_baseline(universe, world)
        .map_err(|err| HostError::Store(err.to_string()))?;
    let seeded_baseline = if let Some(height) = world_config.forced_replay_seed_height {
        Some(hosted_snapshot_at_height(
            &persistence,
            universe,
            world,
            height,
        )?)
    } else {
        hosted_latest_snapshot(&persistence, universe, world)?
    };
    let manifest_hash = seeded_baseline
        .as_ref()
        .and_then(|record| record.manifest_hash.as_deref())
        .or(active.manifest_hash.as_deref())
        .ok_or_else(|| HostError::Manifest("replay seed missing manifest_hash".into()))
        .and_then(|value| parse_hash_like(value, "manifest_hash").map_err(HostError::Manifest))?;
    open_hosted_from_manifest_hash_with_seed(
        persistence,
        universe,
        world,
        manifest_hash,
        world_config,
        adapter_config,
        kernel_config,
        seeded_baseline,
        shared_cache,
    )
}

pub fn open_hosted_from_manifest_hash(
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
    manifest_hash: Hash,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: aos_kernel::KernelConfig,
    shared_cache: Option<SharedBlobCache>,
) -> Result<WorldHost<HostedStore>, HostError> {
    let seeded_baseline = hosted_latest_snapshot(&persistence, universe, world)?;
    open_hosted_from_manifest_hash_with_seed(
        persistence,
        universe,
        world,
        manifest_hash,
        world_config,
        adapter_config,
        kernel_config,
        seeded_baseline,
        shared_cache,
    )
}

fn open_hosted_from_manifest_hash_with_seed(
    persistence: Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
    manifest_hash: Hash,
    world_config: WorldConfig,
    adapter_config: EffectAdapterConfig,
    kernel_config: aos_kernel::KernelConfig,
    seeded_baseline: Option<aos_kernel::journal::SnapshotRecord>,
    shared_cache: Option<SharedBlobCache>,
) -> Result<WorldHost<HostedStore>, HostError> {
    let store = Arc::new(match shared_cache {
        Some(cache) => HostedStore::with_shared_cache(Arc::clone(&persistence), universe, cache),
        None => HostedStore::new(Arc::clone(&persistence), universe),
    });
    let loaded = ManifestLoader::load_from_hash(store.as_ref(), manifest_hash)
        .map_err(|err| HostError::Manifest(err.to_string()))?;
    let journal = HostedJournal::open(Arc::clone(&persistence), universe, world)
        .map_err(|err| HostError::External(err.to_string()))?;
    let replay = match persistence.snapshot_active_baseline(universe, world) {
        Ok(active) => Some(JournalReplayOpen {
            active_baseline: aos_kernel::journal::SnapshotRecord {
                snapshot_ref: active.snapshot_ref,
                height: active.height,
                logical_time_ns: active.logical_time_ns,
                receipt_horizon_height: active.receipt_horizon_height,
                manifest_hash: active.manifest_hash,
            },
            replay_seed: seeded_baseline,
        }),
        Err(PersistError::NotFound(_)) => {
            if seeded_baseline.is_some() {
                return Err(HostError::Store(
                    "snapshot replay seed exists without an active baseline".into(),
                ));
            }
            None
        }
        Err(err) => return Err(HostError::Store(err.to_string())),
    };
    let host = WorldHost::from_loaded_manifest_with_journal_replay(
        store.clone(),
        loaded,
        Box::new(journal),
        world_config,
        adapter_config,
        kernel_config,
        replay,
    )?;
    let store_stats = host.store().stats_snapshot();
    tracing::info!(
        universe_id = %universe,
        world_id = %world,
        store_cache_hits = store_stats.cache_hits,
        store_cache_misses = store_stats.cache_misses,
        store_cas_get_ms = store_stats.cas_get_ms,
        "hosted store stats after world open"
    );
    Ok(host)
}

pub fn snapshot_hosted_world(
    host: &mut WorldHost<HostedStore>,
    persistence: &Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
) -> Result<(), HostError> {
    host.snapshot()?;
    sync_hosted_snapshot_state(host, persistence, universe, world)
}

pub fn sync_hosted_snapshot_state(
    host: &mut WorldHost<HostedStore>,
    persistence: &Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
) -> Result<(), HostError> {
    let active_record = persistence.snapshot_active_baseline(universe, world).ok();
    let journal_start = active_record
        .as_ref()
        .map(|record| record.height.saturating_add(1))
        .unwrap_or(0);
    let journal = host
        .kernel()
        .dump_journal_from(journal_start)
        .map_err(HostError::Kernel)?;
    let mut snapshot_records = active_record.into_iter().collect::<Vec<_>>();
    snapshot_records.extend(latest_snapshot_records(&journal));
    for record in &mut snapshot_records {
        if record.receipt_horizon_height.is_none() {
            record.receipt_horizon_height = Some(record.height);
        }
    }
    let latest_record = snapshot_records.last().cloned();
    let active_height = host.kernel().get_journal_head().active_baseline_height;
    let active_record = active_height.and_then(|height| {
        snapshot_records
            .iter()
            .rev()
            .find(|record| record.height == height)
            .cloned()
    });
    let promoted_record = latest_record
        .clone()
        .filter(|record| record.receipt_horizon_height == Some(record.height))
        .or(active_record.clone());

    let mut records_to_index = Vec::new();
    if let Some(record) = latest_record.clone() {
        records_to_index.push(record);
    }
    if let Some(record) = active_record.clone() {
        if records_to_index
            .last()
            .is_none_or(|latest| latest.height != record.height)
        {
            records_to_index.push(record);
        }
    }

    for record in &records_to_index {
        if let Err(err) = persistence.snapshot_index(universe, world, record.clone()) {
            let existing = persistence
                .snapshot_at_height(universe, world, record.height)
                .ok();
            return Err(HostError::Store(format!(
                "{err}; desired_snapshot={record:?}; indexed_snapshot_at_height={existing:?}"
            )));
        }
    }

    if let Some(record) = promoted_record {
        persistence
            .snapshot_promote_baseline(universe, world, record.clone())
            .map_err(|err| HostError::Store(err.to_string()))?;
        host.kernel_mut()
            .promote_active_baseline_record(&aos_kernel::journal::SnapshotRecord {
                snapshot_ref: record.snapshot_ref,
                height: record.height,
                logical_time_ns: record.logical_time_ns,
                receipt_horizon_height: record.receipt_horizon_height,
                manifest_hash: record.manifest_hash,
            })
            .map_err(HostError::Kernel)?;
    }

    Ok(())
}

fn hosted_latest_snapshot(
    persistence: &Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
) -> Result<Option<aos_kernel::journal::SnapshotRecord>, HostError> {
    let latest = match persistence.snapshot_latest(universe, world) {
        Ok(record) => record,
        Err(PersistError::NotFound(_)) => return Ok(None),
        Err(err) => return Err(HostError::Store(err.to_string())),
    };
    Ok(Some(aos_kernel::journal::SnapshotRecord {
        snapshot_ref: latest.snapshot_ref,
        height: latest.height,
        logical_time_ns: latest.logical_time_ns,
        receipt_horizon_height: latest.receipt_horizon_height,
        manifest_hash: latest.manifest_hash,
    }))
}

fn hosted_snapshot_at_height(
    persistence: &Arc<dyn WorldStore>,
    universe: UniverseId,
    world: WorldId,
    height: u64,
) -> Result<aos_kernel::journal::SnapshotRecord, HostError> {
    let record = persistence
        .snapshot_at_height(universe, world, height)
        .map_err(|err| HostError::Store(err.to_string()))?;
    Ok(aos_kernel::journal::SnapshotRecord {
        snapshot_ref: record.snapshot_ref,
        height: record.height,
        logical_time_ns: record.logical_time_ns,
        receipt_horizon_height: record.receipt_horizon_height,
        manifest_hash: record.manifest_hash,
    })
}

pub(crate) fn parse_hash_like(value: &str, field: &str) -> Result<Hash, String> {
    let trimmed = value.trim();
    let normalized = if trimmed.starts_with(HASH_PREFIX) {
        trimmed.to_string()
    } else {
        format!("{HASH_PREFIX}{trimmed}")
    };
    Hash::from_hex_str(&normalized).map_err(|err| format!("invalid {field} '{value}': {err}"))
}

pub(crate) fn latest_snapshot_records(entries: &[OwnedJournalEntry]) -> Vec<SnapshotRecord> {
    let mut records = Vec::new();
    for entry in entries {
        let record =
            match serde_cbor::from_slice::<aos_kernel::journal::JournalRecord>(&entry.payload) {
                Ok(aos_kernel::journal::JournalRecord::Snapshot(record)) => Some(SnapshotRecord {
                    snapshot_ref: record.snapshot_ref,
                    height: record.height,
                    logical_time_ns: record.logical_time_ns,
                    receipt_horizon_height: record.receipt_horizon_height,
                    manifest_hash: record.manifest_hash,
                }),
                Ok(_) => None,
                Err(_) => serde_cbor::from_slice::<SnapshotRecord>(&entry.payload).ok(),
            };
        if let Some(record) = record {
            records.push(record);
        }
    }
    records
}

#[derive(Default)]
pub struct HostedStoreStats {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cas_get_ms: AtomicU64,
}

impl HostedStoreStats {
    fn record_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self, elapsed_ms: u64) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        self.cas_get_ms.fetch_add(elapsed_ms, Ordering::Relaxed);
    }

    fn snapshot(&self) -> HostedStoreStatsSnapshot {
        HostedStoreStatsSnapshot {
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            cas_get_ms: self.cas_get_ms.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct HostedStoreStatsSnapshot {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cas_get_ms: u64,
}

#[derive(Debug)]
struct BlobCache {
    entry_limit: usize,
    max_total_bytes: usize,
    max_item_bytes: usize,
    total_bytes: usize,
    map: HashMap<Hash, Vec<u8>>,
    order: VecDeque<Hash>,
}

impl BlobCache {
    fn new(entry_limit: usize, max_total_bytes: usize, max_item_bytes: usize) -> Self {
        Self {
            entry_limit,
            max_total_bytes,
            max_item_bytes,
            total_bytes: 0,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, hash: Hash) -> Option<Vec<u8>> {
        let entry = self.map.get(&hash).cloned();
        if entry.is_some() {
            self.promote(hash);
        }
        entry
    }

    fn insert(&mut self, hash: Hash, bytes: &[u8]) {
        if bytes.len() > self.max_item_bytes || self.entry_limit == 0 || self.max_total_bytes == 0 {
            return;
        }
        if let Some(previous) = self.map.get(&hash) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.len());
            self.map.insert(hash, bytes.to_vec());
            self.total_bytes = self.total_bytes.saturating_add(bytes.len());
            self.promote(hash);
            self.evict_to_fit();
            return;
        }
        self.order.push_back(hash);
        self.map.insert(hash, bytes.to_vec());
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        self.evict_to_fit();
    }

    fn evict_to_fit(&mut self) {
        while self.map.len() > self.entry_limit || self.total_bytes > self.max_total_bytes {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some(bytes) = self.map.remove(&evicted) {
                self.total_bytes = self.total_bytes.saturating_sub(bytes.len());
            }
        }
    }

    fn promote(&mut self, hash: Hash) {
        self.order.retain(|existing| *existing != hash);
        self.order.push_back(hash);
    }
}

fn cas_path(hash_hex: String) -> PathBuf {
    PathBuf::from(format!("hosted/cas/{hash_hex}"))
}

fn hashless_hex(bytes: &[u8]) -> String {
    Hash::of_bytes(bytes).to_hex()
}

fn persist_error_to_store_error(path: PathBuf, err: PersistError) -> StoreError {
    let kind = if matches!(err, PersistError::NotFound(_)) {
        ErrorKind::NotFound
    } else {
        ErrorKind::Other
    };
    StoreError::Io {
        path,
        source: IoError::new(kind, err.to_string()),
    }
}

fn persist_error_to_journal_error(err: PersistError) -> JournalError {
    JournalError::Corrupt(err.to_string())
}
