#[cfg(any(feature = "foundationdb-backend", test))]
use std::collections::BTreeMap;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use aos_cbor::Hash;
#[cfg(test)]
use sha2::Digest;

#[cfg(test)]
use aos_node::{CasConfig, CasUploadMarker, PersistCorruption};
use aos_node::{CasLayoutKind, CasRootRecord, CasStore, PersistError, UniverseId};
#[cfg(test)]
use std::io::Cursor;

#[cfg(any(feature = "foundationdb-backend", test))]
const CAS_VERSION: u8 = 1;
#[cfg(any(feature = "foundationdb-backend", test))]
const CHUNK_SIZE: usize = 64 * 1024;
#[cfg(feature = "foundationdb-backend")]
const PRESENT_PAGE_CHUNKS: usize = 1024;
#[cfg(feature = "foundationdb-backend")]
const PRESENT_PAGE_BYTES: usize = PRESENT_PAGE_CHUNKS / 8;
#[cfg(feature = "foundationdb-backend")]
const WRITE_BATCH_CHUNKS: usize = 8;
#[cfg(feature = "foundationdb-backend")]
const READ_BATCH_CHUNKS: usize = 128;

#[cfg(any(feature = "foundationdb-backend", test))]
fn direct_root(chunk_count: u32, size_bytes: u64) -> CasRootRecord {
    CasRootRecord {
        version: CAS_VERSION,
        chunk_size: CHUNK_SIZE as u32,
        chunk_count,
        size_bytes,
        layout_kind: CasLayoutKind::Direct,
    }
}

#[cfg(feature = "foundationdb-backend")]
fn page_for_chunk(index: u32) -> u32 {
    index / PRESENT_PAGE_CHUNKS as u32
}

#[cfg(feature = "foundationdb-backend")]
fn bit_for_chunk(index: u32) -> usize {
    (index % PRESENT_PAGE_CHUNKS as u32) as usize
}

#[cfg(feature = "foundationdb-backend")]
fn set_bit(bitmap: &mut [u8], bit: usize) {
    let byte = bit / 8;
    let mask = 1u8 << (bit % 8);
    bitmap[byte] |= mask;
}

#[cfg(feature = "foundationdb-backend")]
fn has_bit(bitmap: &[u8], bit: usize) -> bool {
    let byte = bit / 8;
    let mask = 1u8 << (bit % 8);
    bitmap
        .get(byte)
        .map(|value| value & mask == mask)
        .unwrap_or(false)
}

#[cfg(feature = "foundationdb-backend")]
fn verify_presence_pages(
    hash: Hash,
    chunk_count: u32,
    pages: &BTreeMap<u32, Vec<u8>>,
) -> Result<(), PersistError> {
    for index in 0..chunk_count {
        let page = page_for_chunk(index);
        let Some(bitmap) = pages.get(&page) else {
            return Err(PersistError::validation(format!(
                "CAS upload for {hash} is incomplete: missing presence page {page}"
            )));
        };
        if !has_bit(bitmap, bit_for_chunk(index)) {
            return Err(PersistError::validation(format!(
                "CAS upload for {hash} is incomplete: missing chunk {index}"
            )));
        }
    }
    Ok(())
}

#[cfg(any(feature = "foundationdb-backend", test))]
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

fn validate_read_range(root: &CasRootRecord, offset: u64, len: u64) -> Result<(), PersistError> {
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
    Ok(())
}

#[derive(Debug, Clone)]
struct CacheEntry {
    root: CasRootRecord,
    bytes: Arc<[u8]>,
    len: usize,
    stamp: u64,
}

#[derive(Debug, Default)]
struct CacheState {
    entries: HashMap<(UniverseId, Hash), CacheEntry>,
    order: VecDeque<((UniverseId, Hash), u64)>,
    total_bytes: usize,
    next_stamp: u64,
}

#[derive(Debug)]
pub struct CachingCasStore<T> {
    inner: T,
    max_bytes: usize,
    max_item_bytes: usize,
    state: Mutex<CacheState>,
}

impl<T> CachingCasStore<T> {
    pub fn new(inner: T, max_bytes: usize, max_item_bytes: usize) -> Self {
        Self {
            inner,
            max_bytes,
            max_item_bytes,
            state: Mutex::new(CacheState::default()),
        }
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    fn cache_get(&self, universe: UniverseId, hash: Hash) -> Option<(CasRootRecord, Arc<[u8]>)> {
        if self.max_bytes == 0 || self.max_item_bytes == 0 {
            return None;
        }
        let mut guard = self.state.lock().ok()?;
        let key = (universe, hash);
        let stamp = {
            let next = guard.next_stamp;
            guard.next_stamp = guard.next_stamp.wrapping_add(1);
            next
        };
        let (root, bytes) = {
            let entry = guard.entries.get_mut(&key)?;
            entry.stamp = stamp;
            (entry.root.clone(), Arc::clone(&entry.bytes))
        };
        guard.order.push_back((key, stamp));
        Some((root, bytes))
    }

    fn cache_insert(&self, universe: UniverseId, hash: Hash, root: CasRootRecord, bytes: Vec<u8>) {
        if self.max_bytes == 0
            || self.max_item_bytes == 0
            || bytes.len() > self.max_bytes
            || bytes.len() > self.max_item_bytes
        {
            return;
        }
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let key = (universe, hash);
        if let Some(existing) = guard.entries.remove(&key) {
            guard.total_bytes = guard.total_bytes.saturating_sub(existing.len);
        }
        let stamp = guard.next_stamp;
        guard.next_stamp = guard.next_stamp.wrapping_add(1);
        let len = bytes.len();
        guard.entries.insert(
            key,
            CacheEntry {
                root,
                bytes: Arc::<[u8]>::from(bytes),
                len,
                stamp,
            },
        );
        guard.order.push_back((key, stamp));
        guard.total_bytes = guard.total_bytes.saturating_add(len);
        while guard.total_bytes > self.max_bytes {
            let Some((evict_key, evict_stamp)) = guard.order.pop_front() else {
                break;
            };
            let should_remove = guard
                .entries
                .get(&evict_key)
                .map(|entry| entry.stamp == evict_stamp)
                .unwrap_or(false);
            if should_remove {
                if let Some(removed) = guard.entries.remove(&evict_key) {
                    guard.total_bytes = guard.total_bytes.saturating_sub(removed.len);
                }
            }
        }
    }
}

struct BufferingReader<'a> {
    inner: &'a mut dyn Read,
    buffer: Option<Vec<u8>>,
}

impl<'a> BufferingReader<'a> {
    fn new(inner: &'a mut dyn Read, max_bytes: usize, size_bytes: u64) -> Self {
        let buffer = if max_bytes > 0 && size_bytes <= max_bytes as u64 {
            Some(Vec::with_capacity(size_bytes as usize))
        } else {
            None
        };
        Self { inner, buffer }
    }

    fn finish(self, expected_len: u64) -> Option<Vec<u8>> {
        match self.buffer {
            Some(bytes) if bytes.len() as u64 == expected_len => Some(bytes),
            _ => None,
        }
    }
}

impl Read for BufferingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if let Some(bytes) = self.buffer.as_mut() {
            bytes.extend_from_slice(&buf[..read]);
        }
        Ok(read)
    }
}

struct TeeWriter<'a> {
    inner: &'a mut dyn Write,
    buffer: Option<Vec<u8>>,
    max_bytes: usize,
}

impl<'a> TeeWriter<'a> {
    fn new(inner: &'a mut dyn Write, max_bytes: usize) -> Self {
        let buffer = if max_bytes > 0 {
            Some(Vec::new())
        } else {
            None
        };
        Self {
            inner,
            buffer,
            max_bytes,
        }
    }

    fn finish(self) -> Option<Vec<u8>> {
        self.buffer
    }
}

impl Write for TeeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        if let Some(bytes) = self.buffer.as_mut() {
            if bytes.len().saturating_add(written) <= self.max_bytes {
                bytes.extend_from_slice(&buf[..written]);
            } else {
                self.buffer = None;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct MemoryCasState {
    roots: BTreeMap<(UniverseId, Hash), CasRootRecord>,
    chunks: BTreeMap<(UniverseId, Hash, u32), Vec<u8>>,
    upload_markers: BTreeMap<(UniverseId, Hash), CasUploadMarker>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct MemoryCasStore {
    state: Arc<Mutex<MemoryCasState>>,
    config: CasConfig,
}

#[cfg(test)]
impl MemoryCasStore {
    pub fn new(config: CasConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryCasState::default())),
            config,
        }
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

#[cfg(test)]
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
        validate_read_range(&root, offset, len)?;
        if len == 0 {
            return Ok(());
        }
        let start_chunk = (offset / root.chunk_size as u64) as u32;
        let end_chunk = ((offset + len - 1) / root.chunk_size as u64) as u32;
        let mut remaining = len;
        for index in start_chunk..=end_chunk {
            let chunk = self.chunk_bytes(universe, hash, &root, index)?;
            let chunk_start = index as u64 * root.chunk_size as u64;
            let begin = offset.saturating_sub(chunk_start) as usize;
            let available = chunk.len().saturating_sub(begin);
            let take = available.min(remaining as usize);
            writer
                .write_all(&chunk[begin..begin + take])
                .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
            remaining -= take as u64;
        }
        Ok(())
    }
}

impl<T> CasStore for CachingCasStore<T>
where
    T: CasStore,
{
    fn put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        let hash = self.inner.put_verified(universe, bytes)?;
        let root = self.inner.stat(universe, hash)?;
        self.cache_insert(universe, hash, root, bytes.to_vec());
        Ok(hash)
    }

    fn put_reader_known_hash(
        &self,
        universe: UniverseId,
        expected: Hash,
        size_bytes: u64,
        reader: &mut dyn Read,
    ) -> Result<Hash, PersistError> {
        let mut buffering_reader =
            BufferingReader::new(reader, self.max_bytes.min(self.max_item_bytes), size_bytes);
        let hash = self.inner.put_reader_known_hash(
            universe,
            expected,
            size_bytes,
            &mut buffering_reader,
        )?;
        if let Some(bytes) = buffering_reader.finish(size_bytes) {
            let root = self.inner.stat(universe, hash)?;
            self.cache_insert(universe, hash, root, bytes);
        }
        Ok(hash)
    }

    fn stat(&self, universe: UniverseId, hash: Hash) -> Result<CasRootRecord, PersistError> {
        if let Some((root, _)) = self.cache_get(universe, hash) {
            return Ok(root);
        }
        self.inner.stat(universe, hash)
    }

    fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        if self.cache_get(universe, hash).is_some() {
            return Ok(true);
        }
        self.inner.has(universe, hash)
    }

    fn get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        if let Some((_root, bytes)) = self.cache_get(universe, hash) {
            return Ok(bytes.as_ref().to_vec());
        }
        let mut bytes = Vec::new();
        let root = self.inner.read_to_writer(universe, hash, &mut bytes)?;
        self.cache_insert(universe, hash, root, bytes.clone());
        Ok(bytes)
    }

    fn read_to_writer(
        &self,
        universe: UniverseId,
        hash: Hash,
        writer: &mut dyn Write,
    ) -> Result<CasRootRecord, PersistError> {
        if let Some((root, bytes)) = self.cache_get(universe, hash) {
            writer
                .write_all(bytes.as_ref())
                .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
            return Ok(root);
        }
        let mut tee = TeeWriter::new(writer, self.max_bytes.min(self.max_item_bytes));
        let root = self.inner.read_to_writer(universe, hash, &mut tee)?;
        if let Some(bytes) = tee.finish() {
            self.cache_insert(universe, hash, root.clone(), bytes);
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
        if let Some((root, bytes)) = self.cache_get(universe, hash) {
            validate_read_range(&root, offset, len)?;
            if len == 0 {
                return Ok(());
            }
            let begin = offset as usize;
            let end = begin + len as usize;
            writer
                .write_all(&bytes[begin..end])
                .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
            return Ok(());
        }
        self.inner
            .read_range_to_writer(universe, hash, offset, len, writer)
    }
}

#[cfg(any(feature = "foundationdb-backend", test))]
fn expected_chunk_len(root: &CasRootRecord, index: u32) -> u64 {
    let chunk_size = root.chunk_size as u64;
    let chunk_start = index as u64 * chunk_size;
    let remaining = root.size_bytes.saturating_sub(chunk_start);
    remaining.min(chunk_size)
}

#[cfg(feature = "foundationdb-backend")]
mod fdb_impl {
    use std::collections::{BTreeMap, BTreeSet};
    use std::future::Future;
    use std::io::{Cursor, Read, Write};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aos_cbor::Hash;
    use foundationdb::{Database, FdbBindingError, MaybeCommitted, RetryableTransaction};
    use futures::executor::block_on;
    use sha2::Digest;

    use super::{
        CAS_VERSION, CHUNK_SIZE, PRESENT_PAGE_BYTES, READ_BATCH_CHUNKS, WRITE_BATCH_CHUNKS,
        bit_for_chunk, chunk_count_for_size, direct_root, expected_chunk_len, page_for_chunk,
        set_bit, validate_read_range, verify_presence_pages,
    };
    use crate::keyspace::FdbKeyspace;
    use aos_node::{
        CasConfig, CasRootRecord, CasStore, CasUploadMarker, PersistCorruption, PersistError,
        UniverseId,
    };

    #[derive(Clone)]
    pub struct FdbCasStore {
        db: Arc<Database>,
        config: CasConfig,
    }

    impl FdbCasStore {
        pub fn new(db: Arc<Database>, config: CasConfig) -> Self {
            Self { db, config }
        }

        fn run<T, F, Fut>(&self, closure: F) -> Result<T, PersistError>
        where
            F: Fn(RetryableTransaction, MaybeCommitted) -> Fut,
            Fut: Future<Output = Result<T, FdbBindingError>>,
        {
            block_on(self.db.run(closure)).map_err(map_fdb_binding_error)
        }

        fn meta_key(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
            FdbKeyspace::universe(universe)
                .cas_meta(hash)
                .pack_for_fdb()
        }

        fn chunk_key(
            &self,
            universe: UniverseId,
            hash: Hash,
            index: u32,
        ) -> Result<Vec<u8>, PersistError> {
            FdbKeyspace::universe(universe)
                .cas_chunk(hash, index as u64)
                .pack_for_fdb()
        }

        fn present_key(
            &self,
            universe: UniverseId,
            hash: Hash,
            page: u32,
        ) -> Result<Vec<u8>, PersistError> {
            FdbKeyspace::universe(universe)
                .cas_present(hash, page as u64)
                .pack_for_fdb()
        }

        fn upload_key(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
            FdbKeyspace::universe(universe)
                .cas_upload(hash)
                .pack_for_fdb()
        }

        fn read_presence_pages(
            &self,
            universe: UniverseId,
            hash: Hash,
            chunk_count: u32,
        ) -> Result<BTreeMap<u32, Vec<u8>>, PersistError> {
            if chunk_count == 0 {
                return Ok(BTreeMap::new());
            }
            let last_page = page_for_chunk(chunk_count - 1);
            let mut pages = BTreeMap::new();
            for page in 0..=last_page {
                let page_key = self.present_key(universe, hash, page)?;
                let value = self.run(|trx, _| {
                    let page_key = page_key.clone();
                    async move {
                        Ok(trx
                            .get(&page_key, false)
                            .await?
                            .map(|v| v.as_ref().to_vec()))
                    }
                })?;
                if let Some(bytes) = value {
                    pages.insert(page, bytes);
                }
            }
            Ok(pages)
        }
    }

    impl CasStore for FdbCasStore {
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
            if self.has(universe, expected)? {
                return Ok(expected);
            }

            let started_at_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos() as u64)
                .unwrap_or(0);
            let upload_key = self.upload_key(universe, expected)?;
            let upload_marker = serde_cbor::to_vec(&CasUploadMarker {
                version: CAS_VERSION,
                writer_id: None,
                started_at_ns,
                last_touched_at_ns: None,
            })
            .map_err(|err| PersistError::backend(format!("encode CAS upload marker: {err}")))?;
            self.run(|trx, _| {
                let upload_key = upload_key.clone();
                let upload_marker = upload_marker.clone();
                async move {
                    trx.set(&upload_key, &upload_marker);
                    Ok(())
                }
            })?;

            let mut chunk_buffer = vec![0u8; CHUNK_SIZE];
            let mut total = 0u64;
            let mut next_index = 0u32;
            let mut hasher = sha2::Sha256::new();
            while total < size_bytes {
                let mut batch = Vec::new();
                while batch.len() < WRITE_BATCH_CHUNKS && total < size_bytes {
                    let to_read = ((size_bytes - total) as usize).min(CHUNK_SIZE);
                    reader
                        .read_exact(&mut chunk_buffer[..to_read])
                        .map_err(|err| {
                            PersistError::backend(format!("read CAS upload stream: {err}"))
                        })?;
                    hasher.update(&chunk_buffer[..to_read]);
                    batch.push((next_index, chunk_buffer[..to_read].to_vec()));
                    next_index += 1;
                    total += to_read as u64;
                }
                let mut page_updates: BTreeMap<u32, BTreeSet<usize>> = BTreeMap::new();
                for (index, _) in &batch {
                    page_updates
                        .entry(page_for_chunk(*index))
                        .or_default()
                        .insert(bit_for_chunk(*index));
                }
                self.run(|trx, _| {
                    let batch = batch.clone();
                    let page_updates = page_updates.clone();
                    async move {
                        for (index, bytes) in &batch {
                            let key = self
                                .chunk_key(universe, expected, *index)
                                .map_err(custom_persist_error)?;
                            trx.set(&key, bytes);
                        }
                        for (page, bits) in &page_updates {
                            let page_key = self
                                .present_key(universe, expected, *page)
                                .map_err(custom_persist_error)?;
                            let mut bitmap = trx
                                .get(&page_key, false)
                                .await?
                                .map(|value| value.as_ref().to_vec())
                                .unwrap_or_else(|| vec![0u8; PRESENT_PAGE_BYTES]);
                            if bitmap.len() < PRESENT_PAGE_BYTES {
                                bitmap.resize(PRESENT_PAGE_BYTES, 0);
                            }
                            for bit in bits {
                                set_bit(&mut bitmap, *bit);
                            }
                            trx.set(&page_key, &bitmap);
                        }
                        Ok(())
                    }
                })?;
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

            let pages = self.read_presence_pages(universe, expected, chunk_count)?;
            verify_presence_pages(expected, chunk_count, &pages)?;
            let meta_key = self.meta_key(universe, expected)?;
            let meta_bytes = serde_cbor::to_vec(&direct_root(chunk_count, size_bytes))
                .map_err(|err| PersistError::backend(format!("encode CAS root: {err}")))?;
            self.run(|trx, _| {
                let meta_key = meta_key.clone();
                let meta_bytes = meta_bytes.clone();
                let upload_key = upload_key.clone();
                async move {
                    if trx.get(&meta_key, false).await?.is_none() {
                        trx.set(&meta_key, &meta_bytes);
                    }
                    trx.clear(&upload_key);
                    Ok(())
                }
            })?;
            Ok(expected)
        }

        fn stat(&self, universe: UniverseId, hash: Hash) -> Result<CasRootRecord, PersistError> {
            let meta_key = self.meta_key(universe, hash)?;
            self.run(|trx, _| {
                let meta_key = meta_key.clone();
                async move {
                    let value = trx.get(&meta_key, false).await?.ok_or_else(|| {
                        custom_persist_error(PersistError::not_found(format!("cas object {hash}")))
                    })?;
                    serde_cbor::from_slice(value.as_ref())
                        .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
                }
            })
        }

        fn has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
            let meta_key = self.meta_key(universe, hash)?;
            self.run(|trx, _| {
                let meta_key = meta_key.clone();
                async move { Ok(trx.get(&meta_key, false).await?.is_some()) }
            })
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
            let mut index = 0u32;
            while index < root.chunk_count {
                let end = (index + READ_BATCH_CHUNKS as u32).min(root.chunk_count);
                let mut batch = Vec::new();
                for chunk_index in index..end {
                    let key = self.chunk_key(universe, hash, chunk_index)?;
                    batch.push((chunk_index, key));
                }
                let fetched: Vec<(u32, Option<Vec<u8>>)> = self.run(|trx, _| {
                    let batch = batch.clone();
                    async move {
                        let mut values = Vec::with_capacity(batch.len());
                        for (chunk_index, key) in &batch {
                            let value = trx.get(key, false).await?.map(|v| v.as_ref().to_vec());
                            values.push((*chunk_index, value));
                        }
                        Ok(values)
                    }
                })?;
                for (chunk_index, maybe_bytes) in fetched {
                    let bytes = maybe_bytes.ok_or(PersistCorruption::MissingCasChunk {
                        hash,
                        index: chunk_index,
                    })?;
                    let expected_len = expected_chunk_len(&root, chunk_index);
                    if bytes.len() as u64 != expected_len {
                        return Err(PersistCorruption::CasSizeMismatch {
                            hash,
                            expected: expected_len,
                            actual: bytes.len() as u64,
                        }
                        .into());
                    }
                    if let Some(hasher) = verify.as_mut() {
                        hasher.update(&bytes);
                    }
                    writer
                        .write_all(&bytes)
                        .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
                    written += bytes.len() as u64;
                }
                index = end;
            }
            if written != root.size_bytes {
                return Err(PersistCorruption::CasSizeMismatch {
                    hash,
                    expected: root.size_bytes,
                    actual: written,
                }
                .into());
            }
            if let Some(hasher) = verify {
                let digest = hasher.finalize();
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
            validate_read_range(&root, offset, len)?;
            if len == 0 {
                return Ok(());
            }
            let start_chunk = (offset / root.chunk_size as u64) as u32;
            let end_chunk = ((offset + len - 1) / root.chunk_size as u64) as u32;
            let mut remaining = len;
            for index in start_chunk..=end_chunk {
                let key = self.chunk_key(universe, hash, index)?;
                let maybe_bytes = self.run(|trx, _| {
                    let key = key.clone();
                    async move { Ok(trx.get(&key, false).await?.map(|v| v.as_ref().to_vec())) }
                })?;
                let bytes =
                    maybe_bytes.ok_or(PersistCorruption::MissingCasChunk { hash, index })?;
                let expected_len = expected_chunk_len(&root, index);
                if bytes.len() as u64 != expected_len {
                    return Err(PersistCorruption::CasSizeMismatch {
                        hash,
                        expected: expected_len,
                        actual: bytes.len() as u64,
                    }
                    .into());
                }
                let chunk_start = index as u64 * root.chunk_size as u64;
                let begin = offset.saturating_sub(chunk_start) as usize;
                let available = bytes.len().saturating_sub(begin);
                let take = available.min(remaining as usize);
                writer
                    .write_all(&bytes[begin..begin + take])
                    .map_err(|err| PersistError::backend(format!("write CAS stream: {err}")))?;
                remaining -= take as u64;
            }
            Ok(())
        }
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
}

#[cfg(feature = "foundationdb-backend")]
pub use fdb_impl::FdbCasStore;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    fn universe() -> UniverseId {
        uuid::Uuid::nil().into()
    }

    #[test]
    fn memory_cas_round_trips_one_shot_and_range_reads() {
        let store = MemoryCasStore::new(CasConfig {
            verify_reads: true,
            ..CasConfig::default()
        });
        let bytes = b"hello world through memory cas";
        let hash = store.put_verified(universe(), bytes).unwrap();

        assert_eq!(hash, Hash::of_bytes(bytes));
        assert_eq!(
            store.stat(universe(), hash).unwrap().size_bytes,
            bytes.len() as u64
        );
        assert_eq!(store.get(universe(), hash).unwrap(), bytes);

        let mut out = Vec::new();
        store
            .read_range_to_writer(universe(), hash, 6, 5, &mut out)
            .unwrap();
        assert_eq!(out, b"world");
    }

    #[test]
    fn memory_cas_streaming_write_verifies_hash() {
        let store = MemoryCasStore::new(CasConfig::default());
        let bytes = b"stream me";
        let expected = Hash::of_bytes(bytes);
        let mut reader = Cursor::new(bytes.as_slice());
        let hash = store
            .put_reader_known_hash(universe(), expected, bytes.len() as u64, &mut reader)
            .unwrap();
        assert_eq!(hash, expected);

        let mut bad_reader = Cursor::new(bytes.as_slice());
        let err = store
            .put_reader_known_hash(
                universe(),
                Hash::of_bytes(b"other"),
                bytes.len() as u64,
                &mut bad_reader,
            )
            .unwrap_err();
        assert!(matches!(err, PersistError::Validation(_)));
    }

    #[test]
    fn memory_cas_same_hash_concurrent_writers_both_succeed() {
        let store = MemoryCasStore::new(CasConfig::default());
        let bytes = vec![9u8; 64 * 1024 + 17];
        let expected = Hash::of_bytes(&bytes);
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = Arc::clone(&barrier);
                let bytes = bytes.clone();
                std::thread::spawn(move || {
                    let mut reader = Cursor::new(bytes.as_slice());
                    barrier.wait();
                    store.put_reader_known_hash(
                        universe(),
                        expected,
                        bytes.len() as u64,
                        &mut reader,
                    )
                })
            })
            .collect();

        barrier.wait();

        for handle in handles {
            assert_eq!(handle.join().unwrap().unwrap(), expected);
        }
        assert_eq!(store.get(universe(), expected).unwrap(), bytes);
    }

    #[test]
    fn memory_cas_chunks_and_marker_are_not_visible_before_root_publish() {
        let store = MemoryCasStore::new(CasConfig::default());
        let bytes = b"publish boundary";
        let hash = Hash::of_bytes(bytes);

        {
            let mut guard = store.state.lock().unwrap();
            guard.upload_markers.insert(
                (universe(), hash),
                CasUploadMarker {
                    version: CAS_VERSION,
                    writer_id: Some("writer-a".into()),
                    started_at_ns: 123,
                    last_touched_at_ns: Some(456),
                },
            );
            guard.chunks.insert((universe(), hash, 0), bytes.to_vec());
        }

        assert!(!store.has(universe(), hash).unwrap());
        assert!(matches!(
            store.stat(universe(), hash),
            Err(PersistError::NotFound(_))
        ));
        assert!(matches!(
            store.get(universe(), hash),
            Err(PersistError::NotFound(_))
        ));
    }

    #[test]
    fn memory_cas_retry_after_publish_boundary_returns_success() {
        let store = MemoryCasStore::new(CasConfig::default());
        let bytes = vec![3u8; 64 * 1024 + 5];
        let hash = Hash::of_bytes(&bytes);
        let root = direct_root(
            chunk_count_for_size(bytes.len() as u64).unwrap(),
            bytes.len() as u64,
        );

        {
            let mut guard = store.state.lock().unwrap();
            guard.upload_markers.insert(
                (universe(), hash),
                CasUploadMarker {
                    version: CAS_VERSION,
                    writer_id: Some("writer-a".into()),
                    started_at_ns: 123,
                    last_touched_at_ns: None,
                },
            );
            for (index, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
                guard
                    .chunks
                    .insert((universe(), hash, index as u32), chunk.to_vec());
            }
            guard.roots.insert((universe(), hash), root);
        }

        let mut reader = Cursor::new(bytes.as_slice());
        let retried = store
            .put_reader_known_hash(universe(), hash, bytes.len() as u64, &mut reader)
            .unwrap();

        assert_eq!(retried, hash);
        assert_eq!(store.get(universe(), hash).unwrap(), bytes);
    }

    #[test]
    fn caching_cas_store_populates_on_write_and_serves_subsequent_reads() {
        let inner = MemoryCasStore::new(CasConfig::default());
        let cache = CachingCasStore::new(inner.clone(), 1024 * 1024, 1024 * 1024);
        let bytes = vec![5u8; 80_000];
        let hash = cache.put_verified(universe(), &bytes).unwrap();

        inner.debug_replace_chunk(universe(), hash, 0, vec![0u8; CHUNK_SIZE]);

        assert_eq!(cache.get(universe(), hash).unwrap(), bytes);
    }

    #[test]
    fn caching_cas_store_populates_on_read_through_and_serves_cached_range_reads() {
        let inner = MemoryCasStore::new(CasConfig::default());
        let bytes = b"cache read through bytes".to_vec();
        let hash = inner.put_verified(universe(), &bytes).unwrap();
        let cache = CachingCasStore::new(inner.clone(), 1024 * 1024, 1024 * 1024);

        assert_eq!(cache.get(universe(), hash).unwrap(), bytes);
        inner.debug_replace_chunk(universe(), hash, 0, b"tampered".to_vec());

        let mut out = Vec::new();
        cache
            .read_range_to_writer(universe(), hash, 6, 4, &mut out)
            .unwrap();
        assert_eq!(out, b"read");
    }

    #[test]
    fn caching_cas_store_skips_entries_larger_than_budget() {
        let inner = MemoryCasStore::new(CasConfig::default());
        let cache = CachingCasStore::new(inner.clone(), 8, 8);
        let bytes = b"this will not fit".to_vec();
        let hash = cache.put_verified(universe(), &bytes).unwrap();

        inner.debug_replace_chunk(universe(), hash, 0, b"tampered".to_vec());
        let err = cache.get(universe(), hash).unwrap_err();
        assert!(matches!(
            err,
            PersistError::Corrupt(PersistCorruption::CasSizeMismatch { .. })
        ));
    }

    #[test]
    fn caching_cas_store_skips_entries_larger_than_item_cap() {
        let inner = MemoryCasStore::new(CasConfig::default());
        let cache = CachingCasStore::new(inner.clone(), 1024 * 1024, 8);
        let bytes = b"this will not fit".to_vec();
        let hash = cache.put_verified(universe(), &bytes).unwrap();

        inner.debug_replace_chunk(universe(), hash, 0, b"tampered".to_vec());
        let err = cache.get(universe(), hash).unwrap_err();
        assert!(matches!(
            err,
            PersistError::Corrupt(PersistCorruption::CasSizeMismatch { .. })
        ));
    }
}
