use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Mutex};

use aos_cbor::Hash;
use sha2::Digest;

use crate::{
    CasConfig, CasLayoutKind, CasRootRecord, CasStore, CasUploadMarker, PersistCorruption,
    PersistError, UniverseId,
};

const CAS_VERSION: u8 = 1;
const CHUNK_SIZE: usize = 64 * 1024;

fn direct_root(chunk_count: u32, size_bytes: u64) -> CasRootRecord {
    CasRootRecord {
        version: CAS_VERSION,
        chunk_size: CHUNK_SIZE as u32,
        chunk_count,
        size_bytes,
        layout_kind: CasLayoutKind::Direct,
    }
}

fn chunk_count_for_size(size_bytes: u64) -> Result<u32, PersistError> {
    let count = if size_bytes == 0 {
        0
    } else {
        ((size_bytes - 1) / CHUNK_SIZE as u64) + 1
    };
    u32::try_from(count).map_err(|_| {
        PersistError::validation(format!(
            "CAS blob size {size_bytes} exceeds supported chunk count"
        ))
    })
}

fn expected_chunk_len(root: &CasRootRecord, index: u32) -> u64 {
    if index + 1 == root.chunk_count && root.size_bytes % CHUNK_SIZE as u64 != 0 {
        root.size_bytes % CHUNK_SIZE as u64
    } else {
        CHUNK_SIZE as u64
    }
}

#[derive(Debug, Clone, Default)]
struct MemoryCasState {
    roots: BTreeMap<(UniverseId, Hash), CasRootRecord>,
    chunks: BTreeMap<(UniverseId, Hash, u32), Vec<u8>>,
    upload_markers: BTreeMap<(UniverseId, Hash), CasUploadMarker>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MemoryCasSnapshot {
    roots: Vec<MemoryCasRootEntry>,
    chunks: Vec<MemoryCasChunkEntry>,
    upload_markers: Vec<MemoryCasUploadMarkerEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MemoryCasRootEntry {
    universe: UniverseId,
    hash: String,
    record: CasRootRecord,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MemoryCasChunkEntry {
    universe: UniverseId,
    hash: String,
    index: u32,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MemoryCasUploadMarkerEntry {
    universe: UniverseId,
    hash: String,
    marker: CasUploadMarker,
}

#[derive(Debug, Clone)]
pub struct MemoryCasStore {
    state: Arc<Mutex<MemoryCasState>>,
    config: CasConfig,
}

impl MemoryCasStore {
    pub fn new(config: CasConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryCasState::default())),
            config,
        }
    }

    pub fn from_state(config: CasConfig, state: Vec<u8>) -> Result<Self, PersistError> {
        let decoded: MemoryCasSnapshot =
            serde_cbor::from_slice(&state).map_err(|err| PersistError::backend(err.to_string()))?;
        let mut cas = MemoryCasState::default();
        for entry in decoded.roots {
            cas.roots.insert(
                (
                    entry.universe,
                    Hash::from_hex_str(&entry.hash).map_err(|err| {
                        PersistError::backend(format!(
                            "decode CAS root hash '{}': {err}",
                            entry.hash
                        ))
                    })?,
                ),
                entry.record,
            );
        }
        for entry in decoded.chunks {
            cas.chunks.insert(
                (
                    entry.universe,
                    Hash::from_hex_str(&entry.hash).map_err(|err| {
                        PersistError::backend(format!(
                            "decode CAS chunk hash '{}': {err}",
                            entry.hash
                        ))
                    })?,
                    entry.index,
                ),
                entry.bytes,
            );
        }
        for entry in decoded.upload_markers {
            cas.upload_markers.insert(
                (
                    entry.universe,
                    Hash::from_hex_str(&entry.hash).map_err(|err| {
                        PersistError::backend(format!(
                            "decode CAS upload marker hash '{}': {err}",
                            entry.hash
                        ))
                    })?,
                ),
                entry.marker,
            );
        }
        Ok(Self {
            state: Arc::new(Mutex::new(cas)),
            config,
        })
    }

    pub fn export_state(&self) -> Result<Vec<u8>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
        let snapshot = MemoryCasSnapshot {
            roots: guard
                .roots
                .iter()
                .map(|((universe, hash), record)| MemoryCasRootEntry {
                    universe: *universe,
                    hash: hash.to_hex(),
                    record: record.clone(),
                })
                .collect(),
            chunks: guard
                .chunks
                .iter()
                .map(|((universe, hash, index), bytes)| MemoryCasChunkEntry {
                    universe: *universe,
                    hash: hash.to_hex(),
                    index: *index,
                    bytes: bytes.clone(),
                })
                .collect(),
            upload_markers: guard
                .upload_markers
                .iter()
                .map(|((universe, hash), marker)| MemoryCasUploadMarkerEntry {
                    universe: *universe,
                    hash: hash.to_hex(),
                    marker: marker.clone(),
                })
                .collect(),
        };
        serde_cbor::to_vec(&snapshot).map_err(|err| PersistError::backend(err.to_string()))
    }

    fn chunk_bytes(
        &self,
        universe: UniverseId,
        hash: Hash,
        root: &CasRootRecord,
        index: u32,
    ) -> Result<Vec<u8>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
        let bytes = guard
            .chunks
            .get(&(universe, hash, index))
            .cloned()
            .ok_or(PersistCorruption::MissingCasChunk { hash, index })?;
        let expected = expected_chunk_len(root, index);
        if bytes.len() as u64 != expected {
            return Err(PersistCorruption::CasSizeMismatch {
                hash,
                expected,
                actual: bytes.len() as u64,
            }
            .into());
        }
        Ok(bytes)
    }

    #[cfg(test)]
    pub(crate) fn debug_replace_chunk(
        &self,
        universe: UniverseId,
        hash: Hash,
        index: u32,
        bytes: Vec<u8>,
    ) {
        let mut guard = self.state.lock().unwrap();
        guard.chunks.insert((universe, hash, index), bytes);
    }
}

impl CasStore for MemoryCasStore {
    fn put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        let hash = Hash::of_bytes(bytes);
        let mut reader = Cursor::new(bytes);
        self.put_reader_known_hash(universe, hash, bytes.len() as u64, &mut reader)
    }

    fn put_reader_known_hash(
        &self,
        universe: UniverseId,
        expected: Hash,
        size_bytes: u64,
        reader: &mut dyn Read,
    ) -> Result<Hash, PersistError> {
        let chunk_count = chunk_count_for_size(size_bytes)?;
        {
            let guard = self
                .state
                .lock()
                .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
            if guard.roots.contains_key(&(universe, expected)) {
                return Ok(expected);
            }
        }

        let mut chunks = Vec::with_capacity(chunk_count as usize);
        let mut hasher = sha2::Sha256::new();
        let mut total = 0u64;
        let mut buffer = vec![0u8; CHUNK_SIZE];
        while total < size_bytes {
            let to_read = ((size_bytes - total) as usize).min(CHUNK_SIZE);
            reader
                .read_exact(&mut buffer[..to_read])
                .map_err(|err| PersistError::backend(format!("read CAS upload stream: {err}")))?;
            hasher.update(&buffer[..to_read]);
            chunks.push(buffer[..to_read].to_vec());
            total += to_read as u64;
        }
        let mut extra = [0u8; 1];
        if reader
            .read(&mut extra)
            .map_err(|err| PersistError::backend(format!("read CAS upload stream: {err}")))?
            != 0
        {
            return Err(PersistError::validation(format!(
                "CAS upload for {expected} exceeded declared size {size_bytes}"
            )));
        }
        if total != size_bytes {
            return Err(PersistError::validation(format!(
                "CAS upload for {expected} read {total} bytes but expected {size_bytes}"
            )));
        }
        let digest = hasher.finalize();
        let actual = Hash::from(<[u8; 32]>::from(digest));
        if actual != expected {
            return Err(PersistError::validation(format!(
                "CAS upload hash mismatch: expected {expected}, computed {actual}"
            )));
        }

        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
        if guard.roots.contains_key(&(universe, expected)) {
            return Ok(expected);
        }
        let root = direct_root(chunk_count, size_bytes);
        guard.upload_markers.insert(
            (universe, expected),
            CasUploadMarker {
                version: CAS_VERSION,
                writer_id: None,
                started_at_ns: 0,
                last_touched_at_ns: None,
            },
        );
        for (index, chunk) in chunks.into_iter().enumerate() {
            guard
                .chunks
                .insert((universe, expected, index as u32), chunk);
        }
        guard.roots.insert((universe, expected), root);
        guard.upload_markers.remove(&(universe, expected));
        Ok(expected)
    }

    fn stat(&self, universe: UniverseId, hash: Hash) -> Result<CasRootRecord, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
        guard
            .roots
            .get(&(universe, hash))
            .cloned()
            .ok_or_else(|| PersistError::not_found(format!("cas object {hash}")))
    }

    fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory CAS mutex poisoned"))?;
        Ok(guard.roots.contains_key(&(universe, hash)))
    }

    fn read_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        writer: &mut dyn Write,
    ) -> Result<CasRootRecord, PersistError> {
        let root = self.stat(universe, hash)?;
        let mut verify = if self.config.verify_reads {
            Some(sha2::Sha256::new())
        } else {
            None
        };
        let mut written = 0u64;
        for index in 0..root.chunk_count {
            let chunk = self.chunk_bytes(universe, hash, &root, index)?;
            if let Some(hasher) = verify.as_mut() {
                hasher.update(&chunk);
            }
            writer
                .write_all(&chunk)
                .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
            written += chunk.len() as u64;
        }
        if written != root.size_bytes {
            return Err(PersistCorruption::CasSizeMismatch {
                hash,
                expected: root.size_bytes,
                actual: written,
            }
            .into());
        }
        if self.config.verify_reads {
            let digest = verify.expect("verify hasher").finalize();
            let actual = Hash::from(<[u8; 32]>::from(digest));
            if actual != hash {
                return Err(PersistCorruption::CasBodyHashMismatch {
                    expected: hash,
                    actual,
                }
                .into());
            }
        }
        Ok(root)
    }

    fn read_range_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        offset: u64,
        len: u64,
        writer: &mut dyn Write,
    ) -> Result<(), PersistError> {
        let root = self.stat(universe, hash)?;
        if offset > root.size_bytes {
            return Err(PersistError::validation(format!(
                "read offset {offset} exceeds CAS blob size {}",
                root.size_bytes
            )));
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| PersistError::validation("read range overflows u64"))?;
        if end > root.size_bytes {
            return Err(PersistError::validation(format!(
                "read end {end} exceeds CAS blob size {}",
                root.size_bytes
            )));
        }
        if len == 0 {
            return Ok(());
        }

        let first_chunk = (offset / CHUNK_SIZE as u64) as u32;
        let last_chunk = ((end - 1) / CHUNK_SIZE as u64) as u32;
        let mut written = 0u64;
        for index in first_chunk..=last_chunk {
            let chunk = self.chunk_bytes(universe, hash, &root, index)?;
            let chunk_start = index as u64 * CHUNK_SIZE as u64;
            let start = offset.saturating_sub(chunk_start) as usize;
            let end = ((end - chunk_start) as usize).min(chunk.len());
            writer
                .write_all(&chunk[start..end])
                .map_err(|err| PersistError::backend(format!("write CAS range: {err}")))?;
            written += (end - start) as u64;
        }
        if written != len {
            return Err(PersistCorruption::CasSizeMismatch {
                hash,
                expected: len,
                actual: written,
            }
            .into());
        }
        Ok(())
    }
}
