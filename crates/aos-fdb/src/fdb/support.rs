use super::*;

impl FdbWorldPersistence {
    pub fn open_default(
        runtime: Arc<FdbRuntime>,
        config: PersistenceConfig,
    ) -> Result<Self, PersistError> {
        Self::open(runtime, None::<&Path>, config)
    }

    pub fn open(
        runtime: Arc<FdbRuntime>,
        cluster_file: Option<impl AsRef<Path>>,
        config: PersistenceConfig,
    ) -> Result<Self, PersistError> {
        let db = match cluster_file {
            Some(path) => Database::from_path(&path.as_ref().to_string_lossy()),
            None => Database::default(),
        }
        .map_err(map_fdb_error)?;
        let db = Arc::new(db);
        let cas = CachingCasStore::new(
            FdbCasStore::new(Arc::clone(&db), config.cas),
            config.cas.cache_bytes,
            config.cas.cache_item_max_bytes,
        );
        Ok(Self {
            _runtime: runtime,
            db,
            cas,
            config,
        })
    }

    pub fn cas(&self) -> &CachingCasStore<FdbCasStore> {
        &self.cas
    }

    #[doc(hidden)]
    pub fn debug_journal_hot_window(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        limit: u32,
    ) -> Result<Vec<(JournalHeight, usize)>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let entry_space = self.journal_entry_space(universe, world);
        let start_key = entry_space.pack(&(self.to_i64(from_inclusive, "journal height")?,));
        let (_, end_key) = entry_space.range();
        self.run(|trx, _| {
            let entry_space = entry_space.clone();
            let start_key = start_key.clone();
            let end_key = end_key.clone();
            async move {
                let mut range = RangeOption::from((start_key, end_key));
                range.mode = foundationdb::options::StreamingMode::WantAll;
                let mut out = Vec::new();
                loop {
                    let remaining = (limit as usize).saturating_sub(out.len());
                    if remaining == 0 {
                        break;
                    }
                    range.limit = Some(remaining);
                    let kvs = trx.get_range(&range, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let (height_i64,) =
                            entry_space.unpack::<(i64,)>(kv.key()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        let height = from_i64_static(height_i64, "journal height")?;
                        out.push((height, kv.value().len()));
                    }
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                Ok(out)
            }
        })
    }

    pub(super) fn run<T, F, Fut>(&self, closure: F) -> Result<T, PersistError>
    where
        F: Fn(RetryableTransaction, MaybeCommitted) -> Fut,
        Fut: Future<Output = Result<T, FdbBindingError>>,
    {
        block_on(self.db.run(closure)).map_err(map_fdb_binding_error)
    }

    pub(super) fn validate_journal_batch(&self, entries: &[Vec<u8>]) -> Result<(), PersistError> {
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

    pub(super) fn root(&self) -> Subspace {
        Subspace::all()
    }

    pub(super) fn universe_root(&self, universe: UniverseId) -> Subspace {
        self.root().subspace(&("u", universe.to_string()))
    }

    pub(super) fn universe_catalog_space(&self) -> Subspace {
        self.root().subspace(&("uu",))
    }

    pub(super) fn universe_meta_key(&self, universe: UniverseId) -> Vec<u8> {
        self.universe_catalog_space().pack(&(universe.to_string(),))
    }

    pub(super) fn universe_handle_root(&self) -> Subspace {
        self.root().subspace(&("uh",))
    }

    pub(super) fn universe_handle_key(&self, handle: &str) -> Vec<u8> {
        self.universe_handle_root().pack(&(handle,))
    }

    pub(super) fn world_root(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("w", world.to_string()))
    }

    pub(super) fn cas_meta_key(&self, universe: UniverseId, hash: Hash) -> Vec<u8> {
        self.universe_root(universe)
            .subspace(&("cas", "meta"))
            .pack(&(hash.to_hex(),))
    }

    pub(super) fn journal_head_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("journal", "head"))
    }

    pub(super) fn world_meta_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("meta",))
    }

    pub(super) fn world_handle_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe).subspace(&("wh",))
    }

    pub(super) fn world_handle_key(&self, universe: UniverseId, handle: &str) -> Vec<u8> {
        self.world_handle_root(universe).pack(&(handle,))
    }

    pub(super) fn journal_entry_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world).subspace(&("journal", "e"))
    }

    pub(super) fn snapshot_by_height_space(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Subspace {
        self.world_root(universe, world)
            .subspace(&("snapshot", "by_height"))
    }

    pub(super) fn baseline_active_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("baseline", "active"))
    }

    pub(super) fn projection_head_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("projection", "head"))
    }

    pub(super) fn projection_cell_root(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world)
            .subspace(&("projection", "cell"))
    }

    pub(super) fn projection_cell_workflow_space(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
    ) -> Subspace {
        self.projection_cell_root(universe, world)
            .subspace(&(workflow,))
    }

    pub(super) fn projection_cell_key(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Vec<u8> {
        self.projection_cell_workflow_space(universe, world, workflow)
            .pack(&(key_hash,))
    }

    pub(super) fn projection_workspace_root(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Subspace {
        self.world_root(universe, world)
            .subspace(&("projection", "workspace"))
    }

    pub(super) fn projection_workspace_key(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
    ) -> Vec<u8> {
        self.projection_workspace_root(universe, world)
            .pack(&(workspace,))
    }

    pub(super) fn inbox_entry_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.world_root(universe, world).subspace(&("inbox", "e"))
    }

    pub(super) fn inbox_cursor_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("inbox", "cursor"))
    }

    pub(super) fn notify_counter_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("notify", "counter"))
    }

    pub(super) fn command_record_key(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Vec<u8> {
        self.world_root(universe, world)
            .subspace(&("commands", "by_id"))
            .pack(&(command_id,))
    }

    pub(super) fn pending_effect_count_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("runtime", "pending_effect_count"))
    }

    pub(super) fn next_timer_due_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("runtime", "next_timer_due"))
    }

    pub(super) fn lease_current_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world).pack(&("lease", "current"))
    }

    pub(super) fn lease_by_worker_key(
        &self,
        worker_id: &str,
        universe: UniverseId,
        world: WorldId,
    ) -> Vec<u8> {
        self.root()
            .subspace(&("lease", "by_worker"))
            .subspace(&(worker_id,))
            .pack(&(universe.to_string(), world.to_string()))
    }

    pub(super) fn lease_by_worker_space(&self, worker_id: &str) -> Subspace {
        self.root()
            .subspace(&("lease", "by_worker"))
            .subspace(&(worker_id,))
    }

    pub(super) fn ready_state_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_root(universe, world)
            .pack(&("runtime", "ready_state"))
    }

    pub(super) fn ready_root(&self) -> Subspace {
        self.root().subspace(&("ready",))
    }

    pub(super) fn ready_key(&self, universe: UniverseId, priority: u16, world: WorldId) -> Vec<u8> {
        self.ready_root()
            .subspace(&(priority as i64,))
            .subspace(&(Self::ready_shard(world) as i64,))
            .pack(&(universe.to_string(), world.to_string()))
    }

    pub(super) fn worker_heartbeat_key(&self, worker_id: &str) -> Vec<u8> {
        self.root()
            .subspace(&("workers", "heartbeat"))
            .pack(&(worker_id,))
    }

    pub(super) fn worker_heartbeat_space(&self) -> Subspace {
        self.root().subspace(&("workers", "heartbeat"))
    }

    pub(super) fn secret_binding_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe).subspace(&("secret_bindings",))
    }

    pub(super) fn secret_binding_key(&self, universe: UniverseId, binding_id: &str) -> Vec<u8> {
        self.secret_binding_root(universe).pack(&(binding_id,))
    }

    pub(super) fn secret_version_root(&self, universe: UniverseId, binding_id: &str) -> Subspace {
        self.universe_root(universe)
            .subspace(&("secret_versions",))
            .subspace(&(binding_id,))
    }

    pub(super) fn secret_version_key(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Vec<u8>, PersistError> {
        Ok(self
            .secret_version_root(universe, binding_id)
            .pack(&(self.to_i64(version, "secret version")?,)))
    }

    pub(super) fn secret_audit_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe).subspace(&("secret_audit",))
    }

    pub(super) fn secret_audit_key(
        &self,
        universe: UniverseId,
        record: &SecretAuditRecord,
    ) -> Result<Vec<u8>, PersistError> {
        Ok(self.secret_audit_root(universe).pack(&(
            self.to_i64(record.ts_ns, "secret audit ts")?,
            record.binding_id.as_str(),
            self.to_i64(record.version.unwrap_or(0), "secret audit version")?,
        )))
    }

    pub(super) fn ready_shard(world: WorldId) -> u16 {
        let uuid = world.as_uuid();
        let bytes = uuid.as_bytes();
        u16::from_be_bytes([bytes[0], bytes[1]])
    }

    pub(super) fn resolve_cas_hash(reference: &str, field: &str) -> Result<Hash, PersistError> {
        Hash::from_hex_str(reference).map_err(|err| {
            PersistError::validation(format!(
                "invalid {field} hash reference '{reference}': {err}"
            ))
        })
    }

    pub(super) fn lineage_from_seed(created_at_ns: u64, seed: &WorldSeed) -> WorldLineage {
        match &seed.imported_from {
            Some(imported_from) => WorldLineage::Import {
                created_at_ns,
                source: imported_from.source.clone(),
                external_world_id: imported_from.external_world_id.clone(),
                external_snapshot_ref: imported_from.external_snapshot_ref.clone(),
            },
            None => WorldLineage::Genesis { created_at_ns },
        }
    }

    pub(super) fn normalize_command_record(
        &self,
        universe: UniverseId,
        mut record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        if let Some(payload) = record.result_payload.as_mut() {
            self.normalize_payload(universe, payload)?;
        }
        Ok(record)
    }

    pub(super) fn seed_for_fork_policy(
        &self,
        universe: UniverseId,
        baseline: &SnapshotRecord,
        policy: &crate::ForkPendingEffectPolicy,
    ) -> Result<WorldSeed, PersistError> {
        let snapshot_hash = Self::resolve_cas_hash(&baseline.snapshot_ref, "snapshot_ref")?;
        let snapshot_bytes = self.cas_get(universe, snapshot_hash)?;
        let snapshot_ref = match rewrite_snapshot_for_fork_policy(&snapshot_bytes, policy)? {
            Some(bytes) => self.cas_put_verified(universe, &bytes)?.to_hex(),
            None => baseline.snapshot_ref.clone(),
        };
        let mut seed = WorldSeed {
            baseline: baseline.clone(),
            seed_kind: crate::SeedKind::Genesis,
            imported_from: None,
        };
        seed.baseline.snapshot_ref = snapshot_ref;
        Ok(seed)
    }

    pub(super) fn resolve_snapshot_selector(
        &self,
        universe: UniverseId,
        world: WorldId,
        selector: &SnapshotSelector,
    ) -> Result<SnapshotRecord, PersistError> {
        match selector {
            SnapshotSelector::ActiveBaseline => self.snapshot_active_baseline(universe, world),
            SnapshotSelector::ByHeight { height } => {
                self.snapshot_at_height(universe, world, *height)
            }
            SnapshotSelector::ByRef { snapshot_ref } => {
                let space = self.snapshot_by_height_space(universe, world);
                self.run(|trx, _| {
                    let space = space.clone();
                    let snapshot_ref = snapshot_ref.clone();
                    async move {
                        let (begin, end) = space.range();
                        let mut range = RangeOption::from((begin, end));
                        range.mode = foundationdb::options::StreamingMode::WantAll;
                        let entries = trx.get_range(&range, 1_024, false).await?;
                        for kv in entries {
                            let record: SnapshotRecord = serde_cbor::from_slice(kv.value())
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            if record.snapshot_ref == snapshot_ref {
                                return Ok(record);
                            }
                        }
                        Err(custom_persist_error(PersistError::not_found(format!(
                            "snapshot {snapshot_ref}"
                        ))))
                    }
                })
            }
        }
    }

    pub(super) fn create_world_from_seed_with_lineage(
        &self,
        universe: UniverseId,
        world_id: WorldId,
        seed: &WorldSeed,
        handle: String,
        placement_pin: Option<String>,
        created_at_ns: u64,
        lineage: WorldLineage,
    ) -> Result<WorldRecord, PersistError> {
        validate_baseline_promotion_record(&seed.baseline)?;
        let snapshot_hash = Self::resolve_cas_hash(&seed.baseline.snapshot_ref, "snapshot_ref")?;
        let manifest_ref = seed
            .baseline
            .manifest_hash
            .as_deref()
            .ok_or_else(|| PersistError::validation("seed baseline requires manifest_hash"))?;
        let manifest_hash = Self::resolve_cas_hash(manifest_ref, "manifest_hash")?;
        let snapshot_bytes = self.cas_get(universe, snapshot_hash)?;
        for (state_hash, state_bytes) in state_blobs_from_snapshot(&snapshot_bytes)? {
            let stored = self.cas_put_verified(universe, &state_bytes)?;
            if stored != state_hash {
                return Err(PersistError::backend(format!(
                    "snapshot state hash mismatch: expected {}, stored {}",
                    state_hash.to_hex(),
                    stored.to_hex()
                )));
            }
        }
        let materialization =
            materialization_from_snapshot(&seed.baseline, &snapshot_bytes, created_at_ns)?;

        let universe_meta_key = self.universe_meta_key(universe);
        let universe_handle = default_universe_handle(universe);
        let universe_handle_key = self.universe_handle_key(&universe_handle);
        let meta_key = self.world_meta_key(universe, world_id);
        let catalog_key = self.world_catalog_key(universe, world_id);
        let handle_key = self.world_handle_key(universe, &handle);
        let head_key = self.journal_head_key(universe, world_id);
        let baseline_key = self.baseline_active_key(universe, world_id);
        let projection_head_key = self.projection_head_key(universe, world_id);
        let projection_cell_root = self.projection_cell_root(universe, world_id);
        let projection_workspace_records: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workspaces
            .iter()
            .map(|workspace| {
                let key = self.projection_workspace_key(universe, world_id, &workspace.workspace);
                let value = self.encode(workspace)?;
                Ok((key, value))
            })
            .collect::<Result<_, PersistError>>()?;
        let projection_workspace_root = self.projection_workspace_root(universe, world_id);
        let snapshot_key = self
            .snapshot_by_height_space(universe, world_id)
            .pack(&(self.to_i64(seed.baseline.height, "snapshot height")?,));
        let ready_state_key = self.ready_state_key(universe, world_id);
        let snapshot_meta_key = self.cas_meta_key(universe, snapshot_hash);
        let manifest_meta_key = self.cas_meta_key(universe, manifest_hash);

        let mut meta = sample_world_meta(world_id);
        meta.handle = handle.clone();
        meta.created_at_ns = created_at_ns;
        meta.placement_pin = placement_pin;
        meta.lineage = Some(lineage);
        meta.active_baseline_height = Some(seed.baseline.height);
        meta.manifest_hash = seed.baseline.manifest_hash.clone();

        let meta_bytes = self.encode(&meta)?;
        let universe_record_bytes = self.encode(&UniverseRecord {
            universe_id: universe,
            created_at_ns,
            meta: aos_node::UniverseMeta {
                handle: universe_handle.clone(),
            },
            admin: UniverseAdminLifecycle::default(),
        })?;
        let baseline_bytes = self.encode(&seed.baseline)?;
        let ready_state_bytes = self.encode(&ReadyState::default())?;
        let head_bytes = seed
            .baseline
            .height
            .saturating_add(1)
            .to_be_bytes()
            .to_vec();
        let projection_head_bytes = self.encode(&materialization.head)?;
        let projection_cells: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workflows
            .iter()
            .flat_map(|workflow| {
                workflow.cells.iter().map(|cell| {
                    let key = self.projection_cell_key(
                        universe,
                        world_id,
                        &workflow.workflow,
                        &cell.key_hash,
                    );
                    let value = self.encode(cell)?;
                    Ok((key, value))
                })
            })
            .collect::<Result<_, PersistError>>()?;

        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let universe_meta_key = universe_meta_key.clone();
            let universe_handle_key = universe_handle_key.clone();
            let catalog_key = catalog_key.clone();
            let handle_key = handle_key.clone();
            let head_key = head_key.clone();
            let baseline_key = baseline_key.clone();
            let projection_head_key = projection_head_key.clone();
            let projection_cell_root = projection_cell_root.clone();
            let projection_workspace_root = projection_workspace_root.clone();
            let snapshot_key = snapshot_key.clone();
            let ready_state_key = ready_state_key.clone();
            let snapshot_meta_key = snapshot_meta_key.clone();
            let manifest_meta_key = manifest_meta_key.clone();
            let meta_bytes = meta_bytes.clone();
            let universe_record_bytes = universe_record_bytes.clone();
            let baseline_bytes = baseline_bytes.clone();
            let ready_state_bytes = ready_state_bytes.clone();
            let head_bytes = head_bytes.clone();
            let projection_head_bytes = projection_head_bytes.clone();
            let projection_cells = projection_cells.clone();
            let projection_workspace_records = projection_workspace_records.clone();
            let baseline = seed.baseline.clone();
            let handle = handle.clone();
            let universe_handle = universe_handle.clone();
            async move {
                if trx.get(&snapshot_meta_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "snapshot {} in universe {}",
                        baseline.snapshot_ref, universe
                    ))));
                }
                if trx.get(&manifest_meta_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "manifest {} in universe {}",
                        baseline.manifest_hash.as_deref().unwrap_or_default(),
                        universe
                    ))));
                }
                if trx.get(&meta_key, false).await?.is_some()
                    || trx.get(&head_key, false).await?.is_some()
                    || trx.get(&baseline_key, false).await?.is_some()
                {
                    return Err(custom_persist_error(
                        PersistConflict::WorldExists { world_id }.into(),
                    ));
                }
                if let Some(existing) = trx.get(&handle_key, false).await? {
                    let existing_world = WorldId::from_str(
                        std::str::from_utf8(existing.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?,
                    )
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                    let existing_meta_key = self.world_meta_key(universe, existing_world);
                    let existing_deleted = trx
                        .get(&existing_meta_key, false)
                        .await?
                        .map(|bytes| {
                            serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                                .map(|meta| {
                                    matches!(meta.admin.status, aos_node::WorldAdminStatus::Deleted)
                                })
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })
                        })
                        .transpose()?
                        .unwrap_or(false);
                    if !existing_deleted {
                        return Err(custom_persist_error(
                            PersistConflict::WorldHandleExists {
                                universe_id: universe,
                                handle,
                                world_id: existing_world,
                            }
                            .into(),
                        ));
                    }
                }

                match trx.get(&universe_meta_key, false).await? {
                    Some(existing) => {
                        let record: UniverseRecord = serde_cbor::from_slice(existing.as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                            return Err(custom_persist_error(
                                PersistConflict::UniverseAdminBlocked {
                                    universe_id: universe,
                                    status: record.admin.status,
                                    action: "create_world".into(),
                                }
                                .into(),
                            ));
                        }
                    }
                    None => {
                        if let Some(existing) = trx.get(&universe_handle_key, false).await? {
                            let existing_universe = UniverseId::from_str(
                                std::str::from_utf8(existing.as_ref()).map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?,
                            )
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                            return Err(custom_persist_error(
                                PersistConflict::UniverseHandleExists {
                                    handle: universe_handle,
                                    universe_id: existing_universe,
                                }
                                .into(),
                            ));
                        }
                        trx.set(&universe_meta_key, &universe_record_bytes);
                        trx.set(&universe_handle_key, universe.to_string().as_bytes());
                    }
                }
                trx.set(&meta_key, &meta_bytes);
                trx.set(&catalog_key, &meta_bytes);
                trx.set(&handle_key, world_id.to_string().as_bytes());
                trx.set(&head_key, &head_bytes);
                trx.set(&snapshot_key, &baseline_bytes);
                trx.set(&baseline_key, &baseline_bytes);
                trx.set(&ready_state_key, &ready_state_bytes);
                trx.set(&projection_head_key, &projection_head_bytes);
                let (begin, end) = projection_cell_root.range();
                trx.clear_range(&begin, &end);
                for (key, value) in &projection_cells {
                    trx.set(key, value);
                }
                let (workspace_begin, workspace_end) = projection_workspace_root.range();
                trx.clear_range(&workspace_begin, &workspace_end);
                for (key, value) in &projection_workspace_records {
                    trx.set(key, value);
                }
                Ok(())
            }
        })?;

        Ok(WorldRecord {
            world_id,
            meta,
            active_baseline: seed.baseline.clone(),
            journal_head: seed.baseline.height.saturating_add(1),
        })
    }

    pub(super) fn effects_pending_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("effects", "pending"))
    }

    pub(super) fn effects_pending_space(&self, universe: UniverseId, shard: u16) -> Subspace {
        self.effects_pending_root(universe)
            .subspace(&(shard as i64,))
    }

    pub(super) fn effects_inflight_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("effects", "inflight"))
    }

    pub(super) fn effects_inflight_space(&self, universe: UniverseId, shard: u16) -> Subspace {
        self.effects_inflight_root(universe)
            .subspace(&(shard as i64,))
    }

    pub(super) fn effect_dedupe_key(&self, universe: UniverseId, intent_hash: &[u8]) -> Vec<u8> {
        self.universe_root(universe)
            .subspace(&("effects", "dedupe"))
            .pack(&(intent_hash,))
    }

    pub(super) fn effect_dedupe_gc_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("effects", "dedupe_gc"))
    }

    pub(super) fn effect_dedupe_gc_key(
        &self,
        universe: UniverseId,
        gc_bucket: u64,
        intent_hash: &[u8],
    ) -> Vec<u8> {
        self.effect_dedupe_gc_root(universe)
            .subspace(&(gc_bucket as i64,))
            .pack(&(intent_hash,))
    }

    pub(super) fn timers_due_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe).subspace(&("timers", "due"))
    }

    pub(super) fn timers_due_space(&self, universe: UniverseId, shard: u16) -> Subspace {
        self.timers_due_root(universe).subspace(&(shard as i64,))
    }

    pub(super) fn timer_inflight_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("timers", "inflight"))
    }

    pub(super) fn timer_inflight_key(
        &self,
        universe: UniverseId,
        shard: u16,
        intent_hash: &[u8],
    ) -> Vec<u8> {
        self.timer_inflight_root(universe)
            .subspace(&(shard as i64,))
            .pack(&(intent_hash,))
    }

    pub(super) fn timer_dedupe_key(&self, universe: UniverseId, intent_hash: &[u8]) -> Vec<u8> {
        self.universe_root(universe)
            .subspace(&("timers", "dedupe"))
            .pack(&(intent_hash,))
    }

    pub(super) fn timer_dedupe_gc_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("timers", "dedupe_gc"))
    }

    pub(super) fn timer_dedupe_gc_key(
        &self,
        universe: UniverseId,
        gc_bucket: u64,
        intent_hash: &[u8],
    ) -> Vec<u8> {
        self.timer_dedupe_gc_root(universe)
            .subspace(&(gc_bucket as i64,))
            .pack(&(intent_hash,))
    }

    pub(super) fn portal_dedupe_key(
        &self,
        universe: UniverseId,
        world: WorldId,
        message_id: &[u8],
    ) -> Vec<u8> {
        self.world_root(universe, world)
            .subspace(&("portal", "dedupe"))
            .pack(&(message_id,))
    }

    pub(super) fn portal_dedupe_gc_root(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("portal", "dedupe_gc"))
    }

    pub(super) fn portal_dedupe_gc_key(
        &self,
        universe: UniverseId,
        gc_bucket: u64,
        world: WorldId,
        message_id: &[u8],
    ) -> Vec<u8> {
        self.portal_dedupe_gc_root(universe)
            .subspace(&(gc_bucket as i64,))
            .subspace(&(world.to_string(),))
            .pack(&(message_id,))
    }

    pub(super) fn world_catalog_space(&self, universe: UniverseId) -> Subspace {
        self.universe_root(universe).subspace(&("worlds",))
    }

    pub(super) fn world_catalog_key(&self, universe: UniverseId, world: WorldId) -> Vec<u8> {
        self.world_catalog_space(universe)
            .pack(&(world.to_string(),))
    }

    pub(super) fn segment_index_space(&self, universe: UniverseId, world: WorldId) -> Subspace {
        self.universe_root(universe)
            .subspace(&("segments", world.to_string()))
    }

    pub(super) fn segment_index_key(
        &self,
        universe: UniverseId,
        world: WorldId,
        end_height: JournalHeight,
    ) -> Result<Vec<u8>, PersistError> {
        Ok(self
            .segment_index_space(universe, world)
            .pack(&(self.to_i64(end_height, "segment end height")?,)))
    }

    pub(super) fn normalize_payload(
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

    pub(super) fn normalize_inbox_item(
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

    pub(super) fn normalize_effect_dispatch_item(
        &self,
        universe: UniverseId,
        mut item: EffectDispatchItem,
    ) -> Result<EffectDispatchItem, PersistError> {
        if let Some(bytes) = item.params_inline_cbor.take() {
            if bytes.len() > self.config.inbox.inline_payload_threshold_bytes {
                let hash = self.cas_put_verified(universe, &bytes)?;
                item.params_ref = Some(hash.to_hex());
                item.params_size = Some(bytes.len() as u64);
                item.params_sha256 = Some(hash.to_hex());
            } else {
                item.params_inline_cbor = Some(bytes);
            }
        }
        Ok(item)
    }

    pub(super) fn encode<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, PersistError> {
        to_canonical_cbor(value)
            .map_err(|err| PersistError::backend(format!("encode canonical cbor: {err}")))
    }

    pub(super) fn to_i64(&self, value: u64, field: &str) -> Result<i64, PersistError> {
        i64::try_from(value).map_err(|_| {
            PersistError::validation(format!("{field} value {value} exceeds i64 tuple encoding"))
        })
    }

    pub(super) fn journal_hot_read_range(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_inclusive: JournalHeight,
        end_exclusive: JournalHeight,
    ) -> Result<Vec<(JournalHeight, Vec<u8>)>, PersistError> {
        if from_inclusive >= end_exclusive {
            return Ok(Vec::new());
        }
        let entry_space = self.journal_entry_space(universe, world);
        let start_key = entry_space.pack(&(self.to_i64(from_inclusive, "journal height")?,));
        let (_, end_key) = entry_space.range();
        self.run(|trx, _| {
            let entry_space = entry_space.clone();
            let start_key = start_key.clone();
            let end_key = end_key.clone();
            async move {
                let mut range = RangeOption::from((start_key, end_key));
                range.mode = foundationdb::options::StreamingMode::WantAll;
                let expected_len = (end_exclusive - from_inclusive) as usize;
                let mut out = Vec::with_capacity(expected_len);
                while out.len() < expected_len {
                    let remaining = expected_len - out.len();
                    range.limit = Some(remaining);
                    let kvs = trx.get_range(&range, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        return Err(custom_persist_error(
                            PersistCorruption::MissingJournalEntry {
                                height: from_inclusive + out.len() as u64,
                            }
                            .into(),
                        ));
                    };
                    for kv in kvs.iter() {
                        let expected_height = from_inclusive + out.len() as u64;
                        let (height_i64,) =
                            entry_space.unpack::<(i64,)>(kv.key()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        let actual_height = from_i64_static(height_i64, "journal height")?;
                        if actual_height != expected_height {
                            return Err(custom_persist_error(
                                PersistCorruption::MissingJournalEntry {
                                    height: expected_height,
                                }
                                .into(),
                            ));
                        }
                        out.push((actual_height, kv.value().to_vec()));
                    }
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                Ok(out)
            }
        })
    }

    pub(super) fn segment_index_read_all_from(
        &self,
        universe: UniverseId,
        world: WorldId,
        from_end_inclusive: JournalHeight,
    ) -> Result<Vec<SegmentIndexRecord>, PersistError> {
        const PAGE: u32 = 256;

        let mut cursor = from_end_inclusive;
        let mut out = Vec::new();
        loop {
            let batch = self.segment_index_read_from(universe, world, cursor, PAGE)?;
            if batch.is_empty() {
                break;
            }
            let next_cursor = batch
                .last()
                .map(|record| record.segment.end.saturating_add(1))
                .unwrap_or(cursor);
            out.extend(batch);
            if next_cursor <= cursor {
                return Err(PersistError::backend(
                    "segment index scan did not advance cursor",
                ));
            }
            cursor = next_cursor;
        }
        Ok(out)
    }

    pub(super) async fn ensure_live_lease(
        &self,
        trx: &RetryableTransaction,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
    ) -> Result<WorldLease, FdbBindingError> {
        let lease_key = self.lease_current_key(universe, world);
        let actual = trx.get(&lease_key, false).await?;
        let Some(actual) = actual else {
            return Err(custom_persist_error(
                PersistConflict::LeaseMismatch {
                    expected_worker_id: lease.holder_worker_id.clone(),
                    expected_epoch: lease.epoch,
                    actual_worker_id: None,
                    actual_epoch: None,
                }
                .into(),
            ));
        };
        let actual: WorldLease = serde_cbor::from_slice(actual.as_ref())
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        if actual.holder_worker_id != lease.holder_worker_id || actual.epoch != lease.epoch {
            return Err(custom_persist_error(
                PersistConflict::LeaseMismatch {
                    expected_worker_id: lease.holder_worker_id.clone(),
                    expected_epoch: lease.epoch,
                    actual_worker_id: Some(actual.holder_worker_id.clone()),
                    actual_epoch: Some(actual.epoch),
                }
                .into(),
            ));
        }
        if actual.expires_at_ns < now_ns {
            return Err(custom_persist_error(
                PersistConflict::LeaseHeld {
                    holder_worker_id: actual.holder_worker_id,
                    epoch: actual.epoch,
                    expires_at_ns: actual.expires_at_ns,
                }
                .into(),
            ));
        }
        Ok(actual)
    }

    pub(super) async fn has_live_worker_heartbeat(
        &self,
        trx: &RetryableTransaction,
        worker_id: &str,
        now_ns: u64,
    ) -> Result<bool, FdbBindingError> {
        let key = self.worker_heartbeat_key(worker_id);
        let Some(bytes) = trx.get(&key, false).await? else {
            return Ok(false);
        };
        let heartbeat: WorkerHeartbeat = serde_cbor::from_slice(bytes.as_ref())
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        Ok(heartbeat.expires_at_ns >= now_ns)
    }

    pub(super) fn verify_current_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
    ) -> Result<(), PersistError> {
        let actual = self.current_world_lease(universe, world)?;
        let Some(actual) = actual else {
            return Err(PersistConflict::LeaseMismatch {
                expected_worker_id: lease.holder_worker_id.clone(),
                expected_epoch: lease.epoch,
                actual_worker_id: None,
                actual_epoch: None,
            }
            .into());
        };
        if actual.holder_worker_id != lease.holder_worker_id || actual.epoch != lease.epoch {
            return Err(PersistConflict::LeaseMismatch {
                expected_worker_id: lease.holder_worker_id.clone(),
                expected_epoch: lease.epoch,
                actual_worker_id: Some(actual.holder_worker_id),
                actual_epoch: Some(actual.epoch),
            }
            .into());
        }
        if actual.expires_at_ns < now_ns {
            return Err(PersistConflict::LeaseHeld {
                holder_worker_id: actual.holder_worker_id,
                epoch: actual.epoch,
                expires_at_ns: actual.expires_at_ns,
            }
            .into());
        }
        Ok(())
    }

    pub(super) fn refresh_world_next_timer_due(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), PersistError> {
        let due_root = self.timers_due_root(universe);
        let (begin, end) = due_root.range();
        let next_due: Option<u64> = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut next_due: Option<u64> = None;
                for kv in kvs.iter() {
                    let item: TimerDueItem =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if item.world_id != world {
                        continue;
                    }
                    next_due = Some(match next_due {
                        Some(existing) => existing.min(item.deliver_at_ns),
                        None => item.deliver_at_ns,
                    });
                }
                Ok(next_due)
            }
        })?;
        let next_timer_due_key = self.next_timer_due_key(universe, world);
        self.run(|trx, _| {
            let next_timer_due_key = next_timer_due_key.clone();
            async move {
                if let Some(next_due) = next_due {
                    trx.set(&next_timer_due_key, &next_due.to_be_bytes());
                } else {
                    trx.clear(&next_timer_due_key);
                }
                Ok(())
            }
        })
    }

    pub(super) fn refresh_world_next_timer_due_and_ready(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<(), PersistError> {
        self.refresh_world_next_timer_due(universe, world)?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                .await?;
            Ok(())
        })
    }

    pub(super) async fn first_hot_journal_height_in_trx(
        &self,
        trx: &RetryableTransaction,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<u64>, FdbBindingError> {
        let entry_space = self.journal_entry_space(universe, world);
        let (begin, end) = entry_space.range();
        let mut range = RangeOption::from((begin, end));
        range.limit = Some(1);
        let kvs = trx.get_range(&range, 1, false).await?;
        let Some(kv) = kvs.iter().next() else {
            return Ok(None);
        };
        let (height_i64,) = entry_space
            .unpack::<(i64,)>(kv.key())
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        Ok(Some(from_i64_static(height_i64, "journal height")?))
    }

    pub(super) async fn first_hot_journal_height_in_tx(
        &self,
        trx: &Transaction,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<u64>, FdbBindingError> {
        let entry_space = self.journal_entry_space(universe, world);
        let (begin, end) = entry_space.range();
        let mut range = RangeOption::from((begin, end));
        range.limit = Some(1);
        let kvs = trx.get_range(&range, 1, false).await?;
        let Some(kv) = kvs.iter().next() else {
            return Ok(None);
        };
        let (height_i64,) = entry_space
            .unpack::<(i64,)>(kv.key())
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        Ok(Some(from_i64_static(height_i64, "journal height")?))
    }

    pub(super) async fn refresh_world_ready_state_in_trx(
        &self,
        trx: &RetryableTransaction,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<(), FdbBindingError> {
        let inbox_cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let pending_effect_count_key = self.pending_effect_count_key(universe, world);
        let next_timer_due_key = self.next_timer_due_key(universe, world);
        let ready_state_key = self.ready_state_key(universe, world);
        let ready_high_key = self.ready_key(universe, 0, world);
        let ready_low_key = self.ready_key(universe, 1, world);

        let inbox_cursor = trx
            .get(&inbox_cursor_key, false)
            .await?
            .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
        let (begin, end) = inbox_space.range();
        let mut range = RangeOption::from((begin, end));
        if let Some(after) = &inbox_cursor {
            let mut after_key = inbox_space.bytes().to_vec();
            after_key.extend_from_slice(after.as_bytes());
            range.begin = KeySelector::first_greater_than(after_key);
        }
        range.limit = Some(1);
        let has_pending_inbox = !trx.get_range(&range, 1, false).await?.is_empty();
        let pending_effect_count = match trx.get(&pending_effect_count_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let journal_head = match trx.get(&head_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let active_baseline_height = trx
            .get(&meta_key, false)
            .await?
            .map(|bytes| {
                serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                    .map(|meta| (meta.active_baseline_height, meta.admin.status))
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            })
            .transpose()?
            .unwrap_or((None, aos_node::WorldAdminStatus::Active));
        let (active_baseline_height, admin_status) = active_baseline_height;
        let first_hot_journal_height = self
            .first_hot_journal_height_in_trx(trx, universe, world)
            .await?;
        let next_timer_due_at_ns = trx
            .get(&next_timer_due_key, false)
            .await?
            .map(|bytes| decode_u64_static(bytes.as_ref()))
            .transpose()
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;

        let ready_state = ReadyState {
            has_pending_inbox,
            has_pending_effects: pending_effect_count > 0,
            next_timer_due_at_ns,
            has_pending_maintenance: admin_status.requires_maintenance_wakeup()
                || maintenance_due(
                    journal_head,
                    active_baseline_height,
                    first_hot_journal_height,
                    self.config.snapshot_maintenance,
                ),
        };
        let ready_state_bytes = serde_cbor::to_vec(&ready_state)
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        trx.set(&ready_state_key, &ready_state_bytes);
        trx.clear(&ready_high_key);
        trx.clear(&ready_low_key);

        if ready_state.is_ready() {
            let priority = ready_state.priority(now_ns);
            let hint = ReadyHint {
                world_id: world,
                priority,
                ready_state,
                updated_at_ns: now_ns,
            };
            let hint_bytes = serde_cbor::to_vec(&hint)
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
            trx.set(&self.ready_key(universe, priority, world), &hint_bytes);
        }

        Ok(())
    }

    pub(super) async fn refresh_world_ready_state_in_tx(
        &self,
        trx: &Transaction,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<(), FdbBindingError> {
        let inbox_cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let pending_effect_count_key = self.pending_effect_count_key(universe, world);
        let next_timer_due_key = self.next_timer_due_key(universe, world);
        let ready_state_key = self.ready_state_key(universe, world);
        let ready_high_key = self.ready_key(universe, 0, world);
        let ready_low_key = self.ready_key(universe, 1, world);

        let inbox_cursor = trx
            .get(&inbox_cursor_key, false)
            .await?
            .map(|bytes| InboxSeq::new(bytes.as_ref().to_vec()));
        let (begin, end) = inbox_space.range();
        let mut range = RangeOption::from((begin, end));
        if let Some(after) = &inbox_cursor {
            let mut after_key = inbox_space.bytes().to_vec();
            after_key.extend_from_slice(after.as_bytes());
            range.begin = KeySelector::first_greater_than(after_key);
        }
        range.limit = Some(1);
        let has_pending_inbox = !trx.get_range(&range, 1, false).await?.is_empty();
        let pending_effect_count = match trx.get(&pending_effect_count_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let journal_head = match trx.get(&head_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let active_baseline_height = trx
            .get(&meta_key, false)
            .await?
            .map(|bytes| {
                serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                    .map(|meta| meta.active_baseline_height)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            })
            .transpose()?
            .flatten();
        let first_hot_journal_height = self
            .first_hot_journal_height_in_tx(trx, universe, world)
            .await?;
        let next_timer_due_at_ns = trx
            .get(&next_timer_due_key, false)
            .await?
            .map(|bytes| decode_u64_static(bytes.as_ref()))
            .transpose()
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;

        let ready_state = ReadyState {
            has_pending_inbox,
            has_pending_effects: pending_effect_count > 0,
            next_timer_due_at_ns,
            has_pending_maintenance: maintenance_due(
                journal_head,
                active_baseline_height,
                first_hot_journal_height,
                self.config.snapshot_maintenance,
            ),
        };
        let ready_state_bytes = serde_cbor::to_vec(&ready_state)
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        trx.set(&ready_state_key, &ready_state_bytes);
        trx.clear(&ready_high_key);
        trx.clear(&ready_low_key);

        if ready_state.is_ready() {
            let priority = ready_state.priority(now_ns);
            let hint = ReadyHint {
                world_id: world,
                priority,
                ready_state,
                updated_at_ns: now_ns,
            };
            let hint_bytes = serde_cbor::to_vec(&hint)
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
            trx.set(&self.ready_key(universe, priority, world), &hint_bytes);
        }

        Ok(())
    }

    pub(super) async fn mark_world_pending_inbox_in_tx(
        &self,
        trx: &Transaction,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<(), FdbBindingError> {
        let pending_effect_count_key = self.pending_effect_count_key(universe, world);
        let next_timer_due_key = self.next_timer_due_key(universe, world);
        let head_key = self.journal_head_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let ready_state_key = self.ready_state_key(universe, world);
        let ready_high_key = self.ready_key(universe, 0, world);
        let ready_low_key = self.ready_key(universe, 1, world);

        let pending_effect_count = match trx.get(&pending_effect_count_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let journal_head = match trx.get(&head_key, false).await? {
            Some(bytes) => decode_u64_static(bytes.as_ref())
                .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?,
            None => 0,
        };
        let active_baseline_height = trx
            .get(&meta_key, false)
            .await?
            .map(|bytes| {
                serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                    .map(|meta| meta.active_baseline_height)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            })
            .transpose()?
            .flatten();
        let first_hot_journal_height = self
            .first_hot_journal_height_in_tx(trx, universe, world)
            .await?;
        let next_timer_due_at_ns = trx
            .get(&next_timer_due_key, false)
            .await?
            .map(|bytes| decode_u64_static(bytes.as_ref()))
            .transpose()
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;

        let ready_state = ReadyState {
            has_pending_inbox: true,
            has_pending_effects: pending_effect_count > 0,
            next_timer_due_at_ns,
            has_pending_maintenance: maintenance_due(
                journal_head,
                active_baseline_height,
                first_hot_journal_height,
                self.config.snapshot_maintenance,
            ),
        };
        let ready_state_bytes = serde_cbor::to_vec(&ready_state)
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        trx.set(&ready_state_key, &ready_state_bytes);
        trx.clear(&ready_high_key);
        trx.clear(&ready_low_key);

        let priority = ready_state.priority(now_ns);
        let hint = ReadyHint {
            world_id: world,
            priority,
            ready_state,
            updated_at_ns: now_ns,
        };
        let hint_bytes = serde_cbor::to_vec(&hint)
            .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
        trx.set(&self.ready_key(universe, priority, world), &hint_bytes);
        Ok(())
    }
}
