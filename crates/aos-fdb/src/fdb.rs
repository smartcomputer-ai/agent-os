use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use foundationdb::options::MutationType;
use foundationdb::tuple::{Subspace, Versionstamp};
use foundationdb::{
    Database, FdbBindingError, FdbError, KeySelector, MaybeCommitted, RangeOption,
    RetryableTransaction,
};
use futures::executor::block_on;

use crate::object_store::{DynBlobObjectStore, filesystem_object_store};
use crate::protocol::{
    BlobStorage, CasMeta, CborPayload, InboxItem, InboxSeq, JournalHeight, PersistConflict,
    PersistCorruption, PersistError, PersistenceConfig, SegmentIndexRecord, SnapshotCommitRequest,
    SnapshotCommitResult, SnapshotRecord, UniverseId, WorldId, WorldPersistence, cas_object_key,
    validate_baseline_promotion_record, validate_snapshot_commit_request, validate_snapshot_record,
};

pub struct FdbRuntime {
    _network: foundationdb::api::NetworkAutoStop,
}

impl FdbRuntime {
    pub fn boot() -> Result<Self, PersistError> {
        let network = unsafe { foundationdb::boot() };
        Ok(Self { _network: network })
    }
}

pub struct FdbWorldPersistence {
    _runtime: Arc<FdbRuntime>,
    db: Arc<Database>,
    object_store: DynBlobObjectStore,
    config: PersistenceConfig,
}

impl FdbWorldPersistence {
    pub fn open_default(
        runtime: Arc<FdbRuntime>,
        object_store_root: impl AsRef<Path>,
        config: PersistenceConfig,
    ) -> Result<Self, PersistError> {
        Self::open(runtime, None::<&Path>, object_store_root, config)
    }

    pub fn open(
        runtime: Arc<FdbRuntime>,
        cluster_file: Option<impl AsRef<Path>>,
        object_store_root: impl AsRef<Path>,
        config: PersistenceConfig,
    ) -> Result<Self, PersistError> {
        let object_store = filesystem_object_store(object_store_root)?;
        Self::open_with_object_store(runtime, cluster_file, object_store, config)
    }

    pub fn open_with_object_store(
        runtime: Arc<FdbRuntime>,
        cluster_file: Option<impl AsRef<Path>>,
        object_store: DynBlobObjectStore,
        config: PersistenceConfig,
    ) -> Result<Self, PersistError> {
        let db = match cluster_file {
            Some(path) => Database::from_path(&path.as_ref().to_string_lossy()),
            None => Database::default(),
        }
        .map_err(map_fdb_error)?;
        Ok(Self {
            _runtime: runtime,
            db: Arc::new(db),
            object_store,
            config,
        })
    }

    fn run<T, F, Fut>(&self, closure: F) -> Result<T, PersistError>
    where
        F: Fn(RetryableTransaction, MaybeCommitted) -> Fut,
        Fut: Future<Output = Result<T, FdbBindingError>>,
    {
        block_on(self.db.run(closure)).map_err(map_fdb_binding_error)
    }

    fn validate_journal_batch(&self, entries: &[Vec<u8>]) -> Result<(), PersistError> {
        if entries.is_empty() {
            return Err(PersistError::validation(
                "journal append batch cannot be empty",
            ));
        }
        if entries.len() > self.config.journal.max_batch_entries {
            return Err(PersistError::validation(format!(
                "journal append batch entry count {} exceeds limit {}",
                entries.len(),
                self.config.journal.max_batch_entries
            )));
        }
        let total_bytes: usize = entries.iter().map(|entry| entry.len()).sum();
        if total_bytes > self.config.journal.max_batch_bytes {
            return Err(PersistError::validation(format!(
                "journal append batch bytes {} exceeds limit {}",
                total_bytes, self.config.journal.max_batch_bytes
            )));
        }
        Ok(())
    }

    fn root(&self) -> Subspace {
        Subspace::all()
    }

    fn universe_root(&self, universe: UniverseId) -> Subspace {
        self.root().subspace(&("u", universe.to_string()))
    }

    fn world_root(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("w", world.to_string()))
    }

    fn cas_meta_key(&self, universe: UniverseId, hash: Hash) -> Vec<u8> {
        self.universe_root(universe)
            .subspace(&("cas", "meta"))
            .pack(&(hash.to_hex(),))
    }

    fn journal_head_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("journal", "head"))
    }

    fn journal_entry_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world).subspace(&("journal", "e"))
    }

    fn snapshot_by_height_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world)
            .subspace(&("snapshot", "by_height"))
    }

    fn baseline_active_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("baseline", "active"))
    }

    fn inbox_entry_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world).subspace(&("inbox", "e"))
    }

    fn inbox_cursor_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("inbox", "cursor"))
    }

    fn notify_counter_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("notify", "counter"))
    }

    fn segment_index_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("segments", world.to_string()))
    }

    fn normalize_payload(
        &self,
        universe: UniverseId,
        payload: &mut CborPayload,
    ) -> Result<(), PersistError> {
        payload.validate()?;
        if let Some(bytes) = payload.inline_cbor.take() {
            if bytes.len() > self.config.inbox.inline_payload_threshold_bytes {
                let hash = self.cas_put_verified(universe, &bytes)?;
                *payload = CborPayload::externalized(hash, bytes.len() as u64);
            } else {
                payload.inline_cbor = Some(bytes);
            }
        }
        Ok(())
    }

    fn normalize_inbox_item(
        &self,
        universe: UniverseId,
        mut item: InboxItem,
    ) -> Result<InboxItem, PersistError> {
        match &mut item {
            InboxItem::DomainEvent(ingress) => {
                self.normalize_payload(universe, &mut ingress.value)?
            }
            InboxItem::Receipt(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
            InboxItem::Inbox(ingress) => self.normalize_payload(universe, &mut ingress.payload)?,
            InboxItem::TimerFired(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
            InboxItem::Control(ingress) => {
                self.normalize_payload(universe, &mut ingress.payload)?
            }
        }
        Ok(item)
    }

    fn encode<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, PersistError> {
        to_canonical_cbor(value)
            .map_err(|err| PersistError::backend(format!("encode canonical cbor: {err}")))
    }

    fn to_i64(&self, value: u64, field: &str) -> Result<i64, PersistError> {
        i64::try_from(value).map_err(|_| {
            PersistError::validation(format!("{field} value {value} exceeds i64 tuple encoding"))
        })
    }
}

impl WorldPersistence for FdbWorldPersistence {
    fn cas_put_verified(&self, universe: UniverseId, bytes: &[u8]) -> Result<Hash, PersistError> {
        let hash = Hash::of_bytes(bytes);
        let meta_key = self.cas_meta_key(universe, hash);
        let meta = if bytes.len() <= self.config.cas.inline_threshold_bytes {
            CasMeta {
                size: bytes.len() as u64,
                storage: BlobStorage::Inline,
                object_key: None,
                inline_bytes: Some(bytes.to_vec()),
            }
        } else {
            let object_key = cas_object_key(universe, hash);
            self.object_store.put_if_absent(&object_key, bytes)?;
            CasMeta {
                size: bytes.len() as u64,
                storage: BlobStorage::ObjectStore,
                object_key: Some(object_key),
                inline_bytes: None,
            }
        };
        let meta_bytes = self.encode(&meta)?;
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let meta_bytes = meta_bytes.clone();
            async move {
                if trx.get(&meta_key, false).await?.is_none() {
                    trx.set(&meta_key, &meta_bytes);
                }
                Ok(())
            }
        })?;
        Ok(hash)
    }

    fn cas_get(&self, universe: UniverseId, hash: Hash) -> Result<Vec<u8>, PersistError> {
        let meta_key = self.cas_meta_key(universe, hash);
        let meta: CasMeta = self.run(|trx, _| {
            let meta_key = meta_key.clone();
            async move {
                let value = trx.get(&meta_key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!("cas object {hash}")))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })?;
        let bytes = match meta.storage {
            BlobStorage::Inline => meta
                .inline_bytes
                .ok_or(PersistCorruption::MissingInlineCasBytes { hash })?,
            BlobStorage::ObjectStore => {
                let object_key = meta
                    .object_key
                    .ok_or(PersistCorruption::MissingCasObjectKey { hash })?;
                match self.object_store.get(&object_key) {
                    Ok(bytes) => bytes,
                    Err(PersistError::NotFound(_)) => {
                        return Err(
                            PersistCorruption::MissingCasObjectBody { hash, object_key }.into()
                        );
                    }
                    Err(err) => return Err(err),
                }
            }
        };
        if self.config.cas.verify_reads {
            let actual = Hash::of_bytes(&bytes);
            if actual != hash {
                return Err(PersistCorruption::CasBodyHashMismatch {
                    expected: hash,
                    actual,
                }
                .into());
            }
        }
        Ok(bytes)
    }

    fn cas_has(&self, universe: UniverseId, hash: Hash) -> Result<bool, PersistError> {
        let meta_key = self.cas_meta_key(universe, hash);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            async move { Ok(trx.get(&meta_key, false).await?.is_some()) }
        })
    }

    fn journal_append_batch(
        &self,
        universe: UniverseId,
        world: WorldId,
        expected_head: JournalHeight,
        entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.validate_journal_batch(entries)?;
        let head_key = self.journal_head_key(universe, world);
        let entry_space = self.journal_entry_space(universe, world);
        let entries = entries.to_vec();
        self.run(|trx, _| {
            let head_key = head_key.clone();
            let entry_space = entry_space.clone();
            let entries = entries.clone();
            async move {
                let head_bytes = trx.get(&head_key, false).await?;
                let actual_head = match head_bytes {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }
                let mut height = actual_head;
                for entry in &entries {
                    let key = entry_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                Ok(actual_head)
            }
        })
    }

    fn journal_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        let head = self.journal_head(universe, world)?;
        if from_inclusive >= head || limit == 0 {
            return Ok(Vec::new());
        }
        let entry_space = self.journal_entry_space(universe, world);
        let start_key = entry_space.pack(&(self.to_i64(from_inclusive, "journal height")?,));
        let (_, end_key) = entry_space.range();
        let expected_count = (head - from_inclusive).min(limit as u64) as usize;
        self.run(|trx, _| {
            let entry_space = entry_space.clone();
            let start_key = start_key.clone();
            let end_key = end_key.clone();
            async move {
                let mut range = RangeOption::from((start_key, end_key));
                range.limit = Some(expected_count);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut expected_height = from_inclusive;
                let mut entries = Vec::with_capacity(expected_count);
                for kv in kvs.iter() {
                    let (height_i64,) = entry_space.unpack::<(i64,)>(kv.key()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    let height = from_i64_static(height_i64, "journal height")?;
                    if height != expected_height {
                        return Err(custom_persist_error(
                            PersistCorruption::MissingJournalEntry {
                                height: expected_height,
                            }
                            .into(),
                        ));
                    }
                    entries.push((height, kv.value().to_vec()));
                    expected_height += 1;
                }
                if entries.len() != expected_count {
                    return Err(custom_persist_error(
                        PersistCorruption::MissingJournalEntry {
                            height: expected_height,
                        }
                        .into(),
                    ));
                }
                Ok(entries)
            }
        })
    }

    fn journal_head(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<JournalHeight, PersistError> {
        let head_key = self.journal_head_key(universe, world);
        self.run(|trx, _| {
            let head_key = head_key.clone();
            async move {
                let value = trx.get(&head_key, false).await?;
                Ok(match value {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                })
            }
        })
    }

    fn inbox_enqueue(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        let item = self.normalize_inbox_item(universe, item)?;
        let value = self.encode(&item)?;
        let inbox_space = self.inbox_entry_space(universe, world);
        let notify_key = self.notify_counter_key(universe, world);

        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let versionstamp = trx.get_versionstamp();
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));

            let op_result: Result<(), TxRetryError> = block_on(async {
                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                Ok(())
            });

            match op_result {
                Ok(()) => match block_on(trx.commit()) {
                    Ok(_) => {
                        let committed = block_on(versionstamp).map_err(map_fdb_error)?;
                        let tr_version: [u8; 10] = committed.as_ref().try_into().map_err(|_| {
                            PersistError::backend(
                                "foundationdb returned non-10-byte transaction versionstamp",
                            )
                        })?;
                        let complete = Versionstamp::complete(tr_version, 0);
                        let packed = inbox_space.pack(&(complete,));
                        let prefix_len = inbox_space.bytes().len();
                        return Ok(InboxSeq::new(packed[prefix_len..].to_vec()));
                    }
                    Err(err) => {
                        block_on(err.on_error()).map_err(map_fdb_error)?;
                    }
                },
                Err(TxRetryError::Fdb(err)) => {
                    block_on(trx.on_error(err)).map_err(map_fdb_error)?;
                }
                Err(TxRetryError::Persist(err)) => return Err(err),
            }
        }
    }

    fn inbox_read_after(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_exclusive: Option<InboxSeq>,
        limit: u32,
    ) -> Result<Vec<(InboxSeq, InboxItem)>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inbox_space = self.inbox_entry_space(universe, world);
        let (begin, end) = inbox_space.range();
        let prefix_len = inbox_space.bytes().len();
        self.run(|trx, _| {
            let inbox_space = inbox_space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let after_exclusive = after_exclusive.clone();
            async move {
                let mut range = RangeOption::from((begin.clone(), end));
                if let Some(after) = after_exclusive {
                    let mut after_key = inbox_space.bytes().to_vec();
                    after_key.extend_from_slice(after.as_bytes());
                    range.begin = KeySelector::first_greater_than(after_key);
                }
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut items = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let key = kv.key();
                    if !key.starts_with(inbox_space.bytes()) {
                        continue;
                    }
                    let seq = InboxSeq::new(key[prefix_len..].to_vec());
                    let item: InboxItem =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    items.push((seq, item));
                }
                Ok(items)
            }
        })
    }

    fn inbox_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<InboxSeq>, PersistError> {
        let cursor_key = self.inbox_cursor_key(universe, world);
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            async move {
                Ok(trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec())))
            }
        })
    }

    fn inbox_commit_cursor(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
    ) -> Result<(), PersistError> {
        let cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            let inbox_space = inbox_space.clone();
            let old_cursor = old_cursor.clone();
            let new_cursor = new_cursor.clone();
            async move {
                let actual_cursor = trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
                if actual_cursor != old_cursor {
                    return Err(custom_persist_error(
                        PersistConflict::InboxCursorAdvanced {
                            expected: old_cursor,
                            actual: actual_cursor,
                        }
                        .into(),
                    ));
                }
                if let Some(current) = &actual_cursor {
                    if new_cursor < *current {
                        return Err(custom_persist_error(PersistError::validation(
                            "inbox cursor cannot regress",
                        )));
                    }
                }
                let inbox_key = build_inbox_key(&inbox_space, &new_cursor);
                if trx.get(&inbox_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "inbox sequence {new_cursor} does not exist"
                    ))));
                }
                trx.set(&cursor_key, new_cursor.as_bytes());
                Ok(())
            }
        })
    }

    fn drain_inbox_to_journal(
        &self,
        universe: UniverseId,
        world: WorldId,
        old_cursor: Option<InboxSeq>,
        new_cursor: InboxSeq,
        expected_head: JournalHeight,
        journal_entries: &[Vec<u8>],
    ) -> Result<JournalHeight, PersistError> {
        self.validate_journal_batch(journal_entries)?;
        let cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let journal_space = self.journal_entry_space(universe, world);
        let journal_entries = journal_entries.to_vec();
        self.run(|trx, _| {
            let cursor_key = cursor_key.clone();
            let inbox_space = inbox_space.clone();
            let head_key = head_key.clone();
            let journal_space = journal_space.clone();
            let old_cursor = old_cursor.clone();
            let new_cursor = new_cursor.clone();
            let journal_entries = journal_entries.clone();
            async move {
                let actual_cursor = trx
                    .get(&cursor_key, false)
                    .await?
                    .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
                if actual_cursor != old_cursor {
                    return Err(custom_persist_error(
                        PersistConflict::InboxCursorAdvanced {
                            expected: old_cursor,
                            actual: actual_cursor,
                        }
                        .into(),
                    ));
                }
                if let Some(current) = &actual_cursor {
                    if new_cursor < *current {
                        return Err(custom_persist_error(PersistError::validation(
                            "inbox cursor cannot regress",
                        )));
                    }
                }
                let new_cursor_key = build_inbox_key(&inbox_space, &new_cursor);
                if trx.get(&new_cursor_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "inbox sequence {new_cursor} does not exist"
                    ))));
                }
                let actual_head = match trx.get(&head_key, false).await? {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }
                let mut height = actual_head;
                for entry in &journal_entries {
                    let key = journal_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                trx.set(&cursor_key, new_cursor.as_bytes());
                Ok(actual_head)
            }
        })
    }

    fn snapshot_index(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        let space = self.snapshot_by_height_space(universe, world);
        let key = space.pack(&(self.to_i64(record.height, "snapshot height")?,));
        let value = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            let record = record.clone();
            async move {
                if let Some(existing) = trx.get(&key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record == record {
                        return Ok(());
                    }
                    return Err(custom_persist_error(
                        PersistConflict::SnapshotExists {
                            height: record.height,
                        }
                        .into(),
                    ));
                }
                trx.set(&key, &value);
                Ok(())
            }
        })
    }

    fn snapshot_commit(
        &self,
        universe: UniverseId,
        world: WorldId,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        validate_snapshot_commit_request(&request)?;
        let snapshot_hash = self.cas_put_verified(universe, &request.snapshot_bytes)?;
        let expected_ref = snapshot_hash.to_hex();
        if request.record.snapshot_ref != expected_ref {
            return Err(PersistError::validation(format!(
                "snapshot_ref {} must equal CAS hash {}",
                request.record.snapshot_ref, expected_ref
            )));
        }

        let mut journal_entries = vec![request.snapshot_journal_entry.clone()];
        if let Some(baseline_entry) = &request.baseline_journal_entry {
            journal_entries.push(baseline_entry.clone());
        }
        self.validate_journal_batch(&journal_entries)?;

        let snapshot_key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(request.record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let journal_space = self.journal_entry_space(universe, world);
        let record = request.record.clone();
        let record_bytes = self.encode(&record)?;
        let expected_head = request.expected_head;
        let promote_baseline = request.promote_baseline;

        self.run(|trx, _| {
            let snapshot_key = snapshot_key.clone();
            let baseline_key = baseline_key.clone();
            let head_key = head_key.clone();
            let journal_space = journal_space.clone();
            let record = record.clone();
            let record_bytes = record_bytes.clone();
            let journal_entries = journal_entries.clone();
            async move {
                let actual_head = match trx.get(&head_key, false).await? {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                if actual_head != expected_head {
                    return Err(custom_persist_error(
                        PersistConflict::HeadAdvanced {
                            expected: expected_head,
                            actual: actual_head,
                        }
                        .into(),
                    ));
                }

                if let Some(existing) = trx.get(&snapshot_key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record != record {
                        return Err(custom_persist_error(
                            PersistConflict::SnapshotExists {
                                height: record.height,
                            }
                            .into(),
                        ));
                    }
                } else {
                    trx.set(&snapshot_key, &record_bytes);
                }

                if promote_baseline {
                    if let Some(active_bytes) = trx.get(&baseline_key, false).await? {
                        let active: SnapshotRecord = serde_cbor::from_slice(active_bytes.as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if record.height < active.height {
                            return Err(custom_persist_error(PersistError::validation(format!(
                                "baseline cannot regress from {} to {}",
                                active.height, record.height
                            ))));
                        }
                        if record.height == active.height && active != record {
                            return Err(custom_persist_error(
                                PersistConflict::BaselineMismatch {
                                    height: record.height,
                                }
                                .into(),
                            ));
                        }
                    }
                }

                let first_height = actual_head;
                let mut height = actual_head;
                for entry in &journal_entries {
                    let key = journal_space.pack(&(to_i64_static(height, "journal height")?,));
                    trx.set(&key, entry);
                    height += 1;
                }
                trx.set(&head_key, &height.to_be_bytes());
                if promote_baseline {
                    trx.set(&baseline_key, &record_bytes);
                }

                Ok(SnapshotCommitResult {
                    snapshot_hash,
                    first_height,
                    next_head: height,
                    baseline_promoted: promote_baseline,
                })
            }
        })
    }

    fn snapshot_at_height(
        &self,
        universe: UniverseId,
        world: WorldId,
        height: JournalHeight,
    ) -> Result<SnapshotRecord, PersistError> {
        let space = self.snapshot_by_height_space(universe, world);
        let key = space.pack(&(self.to_i64(height, "snapshot height")?,));
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                let value = trx.get(&key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!(
                        "snapshot at height {height}"
                    )))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn snapshot_active_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<SnapshotRecord, PersistError> {
        let key = self.baseline_active_key(universe, world);
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                let value = trx.get(&key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found("active baseline"))
                })?;
                serde_cbor::from_slice(value.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn snapshot_promote_baseline(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_baseline_promotion_record(&record)?;
        let snapshot_key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let record_bytes = self.encode(&record)?;
        self.run(|trx, _| {
            let snapshot_key = snapshot_key.clone();
            let baseline_key = baseline_key.clone();
            let record_bytes = record_bytes.clone();
            let record = record.clone();
            async move {
                let indexed = trx.get(&snapshot_key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!(
                        "snapshot at height {}",
                        record.height
                    )))
                })?;
                let indexed_record: SnapshotRecord = serde_cbor::from_slice(indexed.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if indexed_record != record {
                    return Err(custom_persist_error(
                        PersistConflict::SnapshotMismatch {
                            height: record.height,
                        }
                        .into(),
                    ));
                }
                if let Some(active_bytes) = trx.get(&baseline_key, false).await? {
                    let active: SnapshotRecord = serde_cbor::from_slice(active_bytes.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if record.height < active.height {
                        return Err(custom_persist_error(PersistError::validation(format!(
                            "baseline cannot regress from {} to {}",
                            active.height, record.height
                        ))));
                    }
                    if record.height == active.height && active != record {
                        return Err(custom_persist_error(
                            PersistConflict::BaselineMismatch {
                                height: record.height,
                            }
                            .into(),
                        ));
                    }
                }
                trx.set(&baseline_key, &record_bytes);
                Ok(())
            }
        })
    }

    fn segment_index_put(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        let key = self
            .segment_index_space(universe, world)
            .pack(&(self.to_i64(record.segment.end, "segment end height")?,));
        let value = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            async move {
                if trx.get(&key, false).await?.is_some() {
                    return Err(custom_persist_error(
                        PersistConflict::SegmentExists {
                            end_height: record.segment.end,
                        }
                        .into(),
                    ));
                }
                trx.set(&key, &value);
                Ok(())
            }
        })
    }

    fn segment_index_read_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.segment_index_space(universe, world);
        let start_key = space.pack(&(self.to_i64(from_end_inclusive, "segment end height")?,));
        let (_, end_key) = space.range();
        self.run(|trx, _| {
            let start_key = start_key.clone();
            let end_key = end_key.clone();
            async move {
                let mut range = RangeOption::from((start_key, end_key));
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut records = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    records.push(serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?);
                }
                Ok(records)
            }
        })
    }
}

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
