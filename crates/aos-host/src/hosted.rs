use std::cmp::min;
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::sync::Arc;

use aos_cbor::{HASH_PREFIX, Hash, to_canonical_cbor};
use aos_fdb::{PersistError, SnapshotRecord, UniverseId, WorldId, WorldPersistence};
use aos_kernel::journal::{Journal, JournalEntry, JournalError, JournalSeq, OwnedJournalEntry};
use aos_store::{Store, StoreError, StoreResult};
use serde::{Serialize, de::DeserializeOwned};

const HOT_READ_CHUNK_LIMIT: u32 = 512;

pub struct HostedStore {
    persistence: Arc<dyn WorldPersistence>,
    universe: UniverseId,
}

impl HostedStore {
    pub fn new(persistence: Arc<dyn WorldPersistence>, universe: UniverseId) -> Self {
        Self {
            persistence,
            universe,
        }
    }

    pub fn universe(&self) -> UniverseId {
        self.universe
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
        self.persistence
            .cas_get(self.universe, hash)
            .map_err(|err| persist_error_to_store_error(cas_path(hash.to_hex()), err))
    }

    fn has_blob(&self, hash: Hash) -> StoreResult<bool> {
        self.persistence
            .cas_has(self.universe, hash)
            .map_err(|err| persist_error_to_store_error(cas_path(hash.to_hex()), err))
    }
}

pub struct HostedJournal {
    persistence: Arc<dyn WorldPersistence>,
    universe: UniverseId,
    world: WorldId,
    next_seq: JournalSeq,
}

impl HostedJournal {
    pub fn open(
        persistence: Arc<dyn WorldPersistence>,
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
        if from >= head {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let mut cursor = from;
        let mut segments = self
            .persistence
            .segment_index_read_from(self.universe, self.world, from, u32::MAX)
            .map_err(persist_error_to_journal_error)?;
        segments.sort_by_key(|record| record.segment.start);

        for record in segments {
            if record.segment.end < cursor {
                continue;
            }
            if record.segment.start > cursor {
                self.load_hot_entries(cursor, min(record.segment.start, head), &mut out)?;
                cursor = record.segment.start;
            }
            if cursor >= head {
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
                if height >= head {
                    return Err(JournalError::Corrupt(format!(
                        "hosted journal segment entry {height} exceeds head {head}"
                    )));
                }
                out.push(self.decode_entry(&raw, height)?);
                cursor = height + 1;
            }
        }

        if cursor < head {
            self.load_hot_entries(cursor, head, &mut out)?;
        }

        Ok(out)
    }

    fn next_seq(&self) -> JournalSeq {
        self.next_seq
    }
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
