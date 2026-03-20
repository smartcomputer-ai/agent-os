use super::*;

impl FdbWorldPersistence {
    pub(super) fn snapshot_maintenance_config(&self) -> crate::SnapshotMaintenanceConfig {
        self.config.snapshot_maintenance
    }

    pub(super) fn heartbeat_worker(&self, heartbeat: WorkerHeartbeat) -> Result<(), PersistError> {
        let key = self.worker_heartbeat_key(&heartbeat.worker_id);
        let value = self.encode(&heartbeat)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            async move {
                trx.set(&key, &value);
                Ok(())
            }
        })
    }

    pub(super) fn list_active_workers(
        &self,
        now_ns: u64,
        limit: u32,
    ) -> Result<Vec<WorkerHeartbeat>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.worker_heartbeat_space();
        let (begin, end) = space.range();
        self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let mut workers = Vec::new();
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(limit.max(64) as usize);
                loop {
                    let kvs = trx.get_range(&range, 1, false).await?;
                    if kvs.is_empty() {
                        break;
                    }
                    let mut last_key = None::<Vec<u8>>;
                    for kv in kvs.iter() {
                        last_key = Some(kv.key().to_vec());
                        let heartbeat: WorkerHeartbeat =
                            serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if heartbeat.expires_at_ns >= now_ns {
                            workers.push(heartbeat);
                            if workers.len() >= limit as usize {
                                break;
                            }
                        }
                    }
                    if workers.len() >= limit as usize {
                        break;
                    }
                    let Some(last_key) = last_key else {
                        break;
                    };
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                workers.sort_by(|left, right| left.worker_id.cmp(&right.worker_id));
                workers.truncate(limit as usize);
                Ok(workers)
            }
        })
    }

    pub(super) fn world_runtime_info(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        let meta_key = self.world_meta_key(universe, world);
        let notify_counter_key = self.notify_counter_key(universe, world);
        let cursor_key = self.inbox_cursor_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let lease_key = self.lease_current_key(universe, world);
        let ready_state_key = self.ready_state_key(universe, world);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let notify_counter_key = notify_counter_key.clone();
            let cursor_key = cursor_key.clone();
            let inbox_space = inbox_space.clone();
            let lease_key = lease_key.clone();
            let ready_state_key = ready_state_key.clone();
            async move {
                let meta_bytes = trx.get(&meta_key, false).await?.ok_or_else(|| {
                    custom_persist_error(PersistError::not_found(format!(
                        "world {world} in universe {universe}"
                    )))
                })?;
                let meta: WorldMeta = serde_cbor::from_slice(meta_bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                let notify_counter = match trx.get(&notify_counter_key, false).await? {
                    Some(bytes) => decode_u64_static(bytes.as_ref())?,
                    None => 0,
                };
                let inbox_cursor = trx
                    .get(&cursor_key, false)
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
                let recomputed_pending_inbox = !trx.get_range(&range, 1, false).await?.is_empty();
                let ready_state = trx
                    .get(&ready_state_key, false)
                    .await?
                    .map(|bytes| serde_cbor::from_slice::<ReadyState>(bytes.as_ref()))
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?
                    .unwrap_or(ReadyState {
                        has_pending_inbox: recomputed_pending_inbox,
                        ..ReadyState::default()
                    });
                let lease = trx
                    .get(&lease_key, false)
                    .await?
                    .map(|bytes| serde_cbor::from_slice(bytes.as_ref()))
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                let _ = now_ns;
                Ok(WorldRuntimeInfo {
                    world_id: world,
                    meta,
                    notify_counter,
                    has_pending_inbox: ready_state.has_pending_inbox,
                    has_pending_effects: ready_state.has_pending_effects,
                    next_timer_due_at_ns: ready_state.next_timer_due_at_ns,
                    has_pending_maintenance: ready_state.has_pending_maintenance,
                    lease,
                })
            }
        })
    }

    pub(super) fn world_runtime_info_by_handle(
        &self,
        universe: UniverseId,
        handle: &str,
        now_ns: u64,
    ) -> Result<WorldRuntimeInfo, PersistError> {
        let handle = normalize_handle(handle)?;
        let handle_key = self.world_handle_key(universe, &handle);
        let world =
            self.run(|trx, _| {
                let handle_key = handle_key.clone();
                let handle = handle.clone();
                async move {
                    let Some(bytes) = trx.get(&handle_key, false).await? else {
                        return Err(custom_persist_error(PersistError::not_found(format!(
                            "world handle '{handle}' in universe {universe}"
                        ))));
                    };
                    WorldId::from_str(std::str::from_utf8(bytes.as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
                }
            })?;
        let info = self.world_runtime_info(universe, world, now_ns)?;
        if matches!(info.meta.admin.status, aos_node::WorldAdminStatus::Deleted) {
            return Err(PersistError::not_found(format!(
                "world handle '{handle}' in universe {universe}"
            )));
        }
        Ok(info)
    }

    pub(super) fn list_worlds(
        &self,
        universe: UniverseId,
        now_ns: u64,
        after: Option<WorldId>,
        limit: u32,
    ) -> Result<Vec<WorldRuntimeInfo>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.world_catalog_space(universe);
        let (begin, end) = space.range();
        let entries: Vec<(WorldId, WorldMeta)> = self.run(|trx, _| {
            let space = space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let after = after;
            async move {
                let mut range = RangeOption::from((begin, end));
                if let Some(after) = after {
                    range.begin =
                        KeySelector::first_greater_than(space.pack(&(after.to_string(),)));
                }
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut worlds = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let (world_id_str,) = space.unpack::<(String,)>(kv.key()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    let world_id = WorldId::from_str(&world_id_str).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    let meta: WorldMeta =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    worlds.push((world_id, meta));
                }
                Ok(worlds)
            }
        })?;

        let mut worlds = Vec::with_capacity(entries.len());
        for (world_id, meta) in entries {
            let mut info = self.world_runtime_info(universe, world_id, now_ns)?;
            info.meta = meta;
            worlds.push(info);
        }
        worlds.sort_by_key(|info| info.world_id);
        Ok(worlds)
    }

    pub(super) fn list_ready_worlds(
        &self,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.ready_root();
        let (begin, end) = space.range();
        let universe_filter: Option<Vec<String>> =
            universe_filter.map(|ids| ids.iter().map(std::string::ToString::to_string).collect());
        let world_ids: Vec<(UniverseId, WorldId)> = self.run(|trx, _| {
            let space = space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let universe_filter = universe_filter.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(limit.max(64) as usize);
                let mut worlds = Vec::new();
                loop {
                    let kvs = trx.get_range(&range, 1, false).await?;
                    if kvs.is_empty() {
                        break;
                    }
                    let mut last_key = None::<Vec<u8>>;
                    for kv in kvs.iter() {
                        last_key = Some(kv.key().to_vec());
                        let (priority, shard, universe_id_str, world_id_str) = space
                            .unpack::<(i64, i64, String, String)>(kv.key())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        let _ = (priority, shard);
                        if universe_filter.as_ref().is_some_and(|filter| {
                            !filter.iter().any(|candidate| candidate == &universe_id_str)
                        }) {
                            continue;
                        }
                        let universe = UniverseId::from_str(&universe_id_str).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        let world_id = WorldId::from_str(&world_id_str).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        worlds.push((universe, world_id));
                        if worlds.len() >= limit as usize {
                            return Ok(worlds);
                        }
                    }
                    let Some(last_key) = last_key else {
                        break;
                    };
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                Ok(worlds)
            }
        })?;

        let mut worlds = Vec::with_capacity(world_ids.len());
        for (universe, world_id) in world_ids {
            let info = self.world_runtime_info(universe, world_id, now_ns)?;
            if info.meta.admin.status.allows_new_leases() {
                worlds.push(NodeWorldRuntimeInfo {
                    universe_id: universe,
                    info,
                });
            }
        }
        Ok(worlds)
    }

    pub(super) fn head_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<HeadProjectionRecord>, PersistError> {
        let head_key = self.projection_head_key(universe, world);
        self.run(|trx, _| {
            let head_key = head_key.clone();
            async move {
                trx.get(&head_key, false)
                    .await?
                    .map(|bytes| serde_cbor::from_slice::<HeadProjectionRecord>(bytes.as_ref()))
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    pub(super) fn cell_state_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        key_hash: &[u8],
    ) -> Result<Option<CellStateProjectionRecord>, PersistError> {
        let key = self.projection_cell_key(universe, world, workflow, key_hash);
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                trx.get(&key, false)
                    .await?
                    .map(|bytes| {
                        serde_cbor::from_slice::<CellStateProjectionRecord>(bytes.as_ref())
                    })
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    pub(super) fn list_cell_state_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        workflow: &str,
        after_key_hash: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<Vec<CellStateProjectionRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.projection_cell_workflow_space(universe, world, workflow);
        let (begin, end) = space.range();
        self.run(|trx, _| {
            let space = space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let after_key_hash = after_key_hash.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                if let Some(after_key_hash) = after_key_hash {
                    let after_key = space.pack(&(after_key_hash.as_slice(),));
                    range.begin = KeySelector::first_greater_than(after_key);
                }
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut records = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let record: CellStateProjectionRecord =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    records.push(record);
                }
                Ok(records)
            }
        })
    }

    pub(super) fn workspace_projection(
        &self,
        universe: UniverseId,
        world: WorldId,
        workspace: &str,
    ) -> Result<Option<WorkspaceRegistryProjectionRecord>, PersistError> {
        let key = self.projection_workspace_key(universe, world, workspace);
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                trx.get(&key, false)
                    .await?
                    .map(|bytes| {
                        serde_cbor::from_slice::<WorkspaceRegistryProjectionRecord>(bytes.as_ref())
                    })
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    pub(super) fn list_workspace_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        after_workspace: Option<String>,
        limit: u32,
    ) -> Result<Vec<WorkspaceRegistryProjectionRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.projection_workspace_root(universe, world);
        let (begin, end) = space.range();
        self.run(|trx, _| {
            let space = space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let after_workspace = after_workspace.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                if let Some(after_workspace) = after_workspace {
                    let after_key = space.pack(&(after_workspace.as_str(),));
                    range.begin = KeySelector::first_greater_than(after_key);
                }
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut records = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let record: WorkspaceRegistryProjectionRecord =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    records.push(record);
                }
                Ok(records)
            }
        })
    }

    pub(super) fn bootstrap_query_projections(
        &self,
        universe: UniverseId,
        world: WorldId,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        validate_query_projection_materialization(&materialization)?;
        let head_key = self.projection_head_key(universe, world);
        let cell_root = self.projection_cell_root(universe, world);
        let workspace_root = self.projection_workspace_root(universe, world);
        let head_bytes = self.encode(&materialization.head)?;
        let cell_records: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workflows
            .iter()
            .flat_map(|workflow| {
                workflow.cells.iter().map(|cell| {
                    let key = self.projection_cell_key(
                        universe,
                        world,
                        &workflow.workflow,
                        &cell.key_hash,
                    );
                    let value = self.encode(cell)?;
                    Ok((key, value))
                })
            })
            .collect::<Result<_, PersistError>>()?;
        let workspace_records: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workspaces
            .iter()
            .map(|workspace| {
                let key = self.projection_workspace_key(universe, world, &workspace.workspace);
                let value = self.encode(workspace)?;
                Ok((key, value))
            })
            .collect::<Result<_, PersistError>>()?;
        self.run(|trx, _| {
            let head_key = head_key.clone();
            let cell_root = cell_root.clone();
            let workspace_root = workspace_root.clone();
            let head_bytes = head_bytes.clone();
            let cell_records = cell_records.clone();
            let workspace_records = workspace_records.clone();
            async move {
                trx.set(&head_key, &head_bytes);
                let (cell_begin, cell_end) = cell_root.range();
                trx.clear_range(&cell_begin, &cell_end);
                for (key, value) in &cell_records {
                    trx.set(key, value);
                }
                let (workspace_begin, workspace_end) = workspace_root.range();
                trx.clear_range(&workspace_begin, &workspace_end);
                for (key, value) in &workspace_records {
                    trx.set(key, value);
                }
                Ok(())
            }
        })
    }

    pub(super) fn enqueue_ingress(
        &self,
        universe: UniverseId,
        world: WorldId,
        item: InboxItem,
    ) -> Result<InboxSeq, PersistError> {
        let meta = self.world_runtime_info(universe, world, 0)?.meta;
        let allowed = match &item {
            InboxItem::Control(_) => meta.admin.status.accepts_command_ingress(),
            _ => meta.admin.status.accepts_direct_ingress(),
        };
        if !allowed {
            return Err(PersistConflict::WorldAdminBlocked {
                world_id: world,
                status: meta.admin.status,
                action: "enqueue_ingress".into(),
            }
            .into());
        }
        self.inbox_enqueue(universe, world, item)
    }

    pub(super) fn command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        command_id: &str,
    ) -> Result<Option<CommandRecord>, PersistError> {
        let key = self.command_record_key(universe, world, command_id);
        self.run(|trx, _| {
            let key = key.clone();
            async move {
                let Some(bytes) = trx.get(&key, false).await? else {
                    return Ok(None);
                };
                let stored: StoredCommandRecord = serde_cbor::from_slice(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                Ok(Some(stored.record))
            }
        })
    }

    pub(super) fn submit_command(
        &self,
        universe: UniverseId,
        world: WorldId,
        ingress: crate::CommandIngress,
        initial_record: CommandRecord,
    ) -> Result<CommandRecord, PersistError> {
        let request_hash = aos_cbor::Hash::of_cbor(&(
            ingress.command.as_str(),
            ingress.actor.as_deref(),
            &ingress.payload,
        ))
        .map_err(|err| PersistError::backend(err.to_string()))?
        .to_hex();
        let item = self.normalize_inbox_item(universe, InboxItem::Control(ingress.clone()))?;
        let initial_record = self.normalize_command_record(universe, initial_record)?;
        let record_key = self.command_record_key(universe, world, &ingress.command_id);
        let stored_bytes = serde_cbor::to_vec(&StoredCommandRecord {
            record: initial_record.clone(),
            request_hash: request_hash.clone(),
        })
        .map_err(|err| PersistError::backend(err.to_string()))?;
        let value =
            serde_cbor::to_vec(&item).map_err(|err| PersistError::backend(err.to_string()))?;
        let inbox_space = self.inbox_entry_space(universe, world);
        let notify_key = self.notify_counter_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let meta_bytes = serde_cbor::to_vec(&sample_world_meta(world))
            .map_err(|err| PersistError::backend(err.to_string()))?;

        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));

            let op_result: Result<CommandRecord, TxRetryError> = block_on(async {
                let meta = match trx.get(&meta_key, false).await {
                    Ok(Some(bytes)) => serde_cbor::from_slice::<WorldMeta>(bytes.as_ref())
                        .map_err(|err| {
                            TxRetryError::Persist(PersistError::backend(err.to_string()))
                        })?,
                    Ok(None) => {
                        trx.set(&meta_key, &meta_bytes);
                        trx.set(&catalog_key, &meta_bytes);
                        sample_world_meta(world)
                    }
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                if !meta.admin.status.accepts_command_ingress() {
                    return Err(TxRetryError::Persist(
                        PersistConflict::WorldAdminBlocked {
                            world_id: world,
                            status: meta.admin.status,
                            action: "submit_command".into(),
                        }
                        .into(),
                    ));
                }

                if let Some(existing) = trx
                    .get(&record_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                {
                    let stored: StoredCommandRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            TxRetryError::Persist(PersistError::backend(err.to_string()))
                        })?;
                    if stored.request_hash != request_hash {
                        return Err(TxRetryError::Persist(
                            PersistConflict::CommandRequestMismatch {
                                command_id: ingress.command_id.clone(),
                            }
                            .into(),
                        ));
                    }
                    return Ok(stored.record);
                }

                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.set(&record_key, &stored_bytes);
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                self.mark_world_pending_inbox_in_tx(&trx, universe, world, 0)
                    .await
                    .map_err(map_fdb_binding_error)
                    .map_err(TxRetryError::Persist)?;
                Ok(initial_record.clone())
            });

            match op_result {
                Ok(record) => match block_on(trx.commit()) {
                    Ok(_) => return Ok(record),
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

    pub(super) fn update_command_record(
        &self,
        universe: UniverseId,
        world: WorldId,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        let record = self.normalize_command_record(universe, record)?;
        let key = self.command_record_key(universe, world, &record.command_id);
        let record_id = record.command_id.clone();
        let bytes = serde_cbor::to_vec(&StoredCommandRecord {
            request_hash: {
                let existing = self
                    .run(|trx, _| {
                        let key = key.clone();
                        async move {
                            let Some(value) = trx.get(&key, false).await? else {
                                return Ok(None);
                            };
                            let stored: StoredCommandRecord =
                                serde_cbor::from_slice(value.as_ref()).map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            Ok(Some(stored.request_hash))
                        }
                    })?
                    .ok_or_else(|| PersistError::not_found(format!("command {record_id}")))?;
                existing
            },
            record,
        })
        .map_err(|err| PersistError::backend(err.to_string()))?;
        self.run(|trx, _| {
            let key = key.clone();
            let bytes = bytes.clone();
            let record_id = record_id.clone();
            async move {
                if trx.get(&key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "command {record_id}"
                    ))));
                }
                trx.set(&key, &bytes);
                Ok(())
            }
        })
    }

    pub(super) fn update_command_record_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &crate::WorldLease,
        now_ns: u64,
        record: CommandRecord,
    ) -> Result<(), PersistError> {
        let record = self.normalize_command_record(universe, record)?;
        let key = self.command_record_key(universe, world, &record.command_id);
        let record_id = record.command_id.clone();
        let bytes = serde_cbor::to_vec(&StoredCommandRecord {
            request_hash: {
                let existing = self
                    .run(|trx, _| {
                        let key = key.clone();
                        async move {
                            let Some(value) = trx.get(&key, false).await? else {
                                return Ok(None);
                            };
                            let stored: StoredCommandRecord =
                                serde_cbor::from_slice(value.as_ref()).map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            Ok(Some(stored.request_hash))
                        }
                    })?
                    .ok_or_else(|| PersistError::not_found(format!("command {record_id}")))?;
                existing
            },
            record,
        })
        .map_err(|err| PersistError::backend(err.to_string()))?;
        self.run(|trx, _| {
            let key = key.clone();
            let bytes = bytes.clone();
            let lease = lease.clone();
            let record_id = record_id.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                if trx.get(&key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "command {record_id}"
                    ))));
                }
                trx.set(&key, &bytes);
                Ok(())
            }
        })
    }

    pub(super) fn set_world_placement_pin(
        &self,
        universe: UniverseId,
        world: WorldId,
        placement_pin: Option<String>,
    ) -> Result<(), PersistError> {
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let placement_pin = placement_pin.clone();
            async move {
                let mut meta = match trx.get(&meta_key, false).await? {
                    Some(bytes) => {
                        serde_cbor::from_slice::<WorldMeta>(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?
                    }
                    None => sample_world_meta(world),
                };
                if meta.admin.status.blocks_world_operations() {
                    return Err(custom_persist_error(
                        PersistConflict::WorldAdminBlocked {
                            world_id: world,
                            status: meta.admin.status,
                            action: "set_world_placement_pin".into(),
                        }
                        .into(),
                    ));
                }
                meta.placement_pin = placement_pin;
                let bytes = serde_cbor::to_vec(&meta)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&meta_key, &bytes);
                trx.set(&catalog_key, &bytes);
                Ok(())
            }
        })
    }

    pub(super) fn set_world_admin_lifecycle(
        &self,
        universe: UniverseId,
        world: WorldId,
        admin: WorldAdminLifecycle,
    ) -> Result<(), PersistError> {
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let admin = admin.clone();
            async move {
                let mut meta = match trx.get(&meta_key, false).await? {
                    Some(bytes) => {
                        serde_cbor::from_slice::<WorldMeta>(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?
                    }
                    None => sample_world_meta(world),
                };
                meta.admin = admin;
                let bytes = serde_cbor::to_vec(&meta)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if matches!(meta.admin.status, aos_node::WorldAdminStatus::Deleted) {
                    let handle_key = self.world_handle_key(universe, &meta.handle);
                    trx.clear(&handle_key);
                }
                trx.set(&meta_key, &bytes);
                trx.set(&catalog_key, &bytes);
                self.refresh_world_ready_state_in_trx(&trx, universe, world, 0)
                    .await?;
                Ok(())
            }
        })
    }

    pub(super) fn current_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<Option<WorldLease>, PersistError> {
        let lease_key = self.lease_current_key(universe, world);
        self.run(|trx, _| {
            let lease_key = lease_key.clone();
            async move {
                trx.get(&lease_key, false)
                    .await?
                    .map(|bytes| serde_cbor::from_slice(bytes.as_ref()))
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    pub(super) fn acquire_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        let lease_key = self.lease_current_key(universe, world);
        let lease_by_worker_key = self.lease_by_worker_key(worker_id, universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let default_meta = self.encode(&sample_world_meta(world))?;
        let worker_id = worker_id.to_string();
        self.run(|trx, _| {
            let lease_key = lease_key.clone();
            let lease_by_worker_key = lease_by_worker_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let default_meta = default_meta.clone();
            let worker_id = worker_id.clone();
            async move {
                let meta = match trx.get(&meta_key, false).await? {
                    Some(bytes) => {
                        serde_cbor::from_slice::<WorldMeta>(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?
                    }
                    None => {
                        trx.set(&meta_key, &default_meta);
                        trx.set(&catalog_key, &default_meta);
                        sample_world_meta(world)
                    }
                };
                if !meta.admin.status.allows_new_leases() {
                    return Err(custom_persist_error(
                        PersistConflict::WorldAdminBlocked {
                            world_id: world,
                            status: meta.admin.status,
                            action: "acquire_world_lease".into(),
                        }
                        .into(),
                    ));
                }
                let current = trx.get(&lease_key, false).await?;
                let current: Option<WorldLease> = current
                    .map(|bytes| serde_cbor::from_slice(bytes.as_ref()))
                    .transpose()
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if let Some(current) = current {
                    if current.expires_at_ns >= now_ns {
                        if current.holder_worker_id == worker_id {
                            let current_bytes = serde_cbor::to_vec(&current).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                            trx.set(&lease_by_worker_key, &current_bytes);
                            return Ok(current);
                        }
                        if self
                            .has_live_worker_heartbeat(&trx, &current.holder_worker_id, now_ns)
                            .await?
                        {
                            return Err(custom_persist_error(
                                PersistConflict::LeaseHeld {
                                    holder_worker_id: current.holder_worker_id,
                                    epoch: current.epoch,
                                    expires_at_ns: current.expires_at_ns,
                                }
                                .into(),
                            ));
                        }
                    }
                    let lease = WorldLease {
                        holder_worker_id: worker_id.clone(),
                        epoch: current.epoch + 1,
                        expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                    };
                    let value = serde_cbor::to_vec(&lease).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    trx.set(&lease_key, &value);
                    trx.clear(&self.lease_by_worker_key(
                        &current.holder_worker_id,
                        universe,
                        world,
                    ));
                    trx.set(&lease_by_worker_key, &value);
                    return Ok(lease);
                }
                let lease = WorldLease {
                    holder_worker_id: worker_id.clone(),
                    epoch: 1,
                    expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                };
                let value = serde_cbor::to_vec(&lease)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&lease_key, &value);
                trx.set(&lease_by_worker_key, &value);
                Ok(lease)
            }
        })
    }

    pub(super) fn renew_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        lease_ttl_ns: u64,
    ) -> Result<WorldLease, PersistError> {
        let lease_key = self.lease_current_key(universe, world);
        let lease_by_worker_key =
            self.lease_by_worker_key(&lease.holder_worker_id, universe, world);
        self.run(|trx, _| {
            let lease_key = lease_key.clone();
            let lease_by_worker_key = lease_by_worker_key.clone();
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                let renewed = WorldLease {
                    holder_worker_id: lease.holder_worker_id.clone(),
                    epoch: lease.epoch,
                    expires_at_ns: now_ns.saturating_add(lease_ttl_ns),
                };
                let value = serde_cbor::to_vec(&renewed)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&lease_key, &value);
                trx.set(&lease_by_worker_key, &value);
                Ok(renewed)
            }
        })
    }

    pub(super) fn release_world_lease(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
    ) -> Result<(), PersistError> {
        let lease_key = self.lease_current_key(universe, world);
        let lease_by_worker_key =
            self.lease_by_worker_key(&lease.holder_worker_id, universe, world);
        self.run(|trx, _| {
            let lease_key = lease_key.clone();
            let lease_by_worker_key = lease_by_worker_key.clone();
            let lease = lease.clone();
            async move {
                let current = trx.get(&lease_key, false).await?;
                let Some(current) = current else {
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
                let current: WorldLease = serde_cbor::from_slice(current.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if current.holder_worker_id != lease.holder_worker_id
                    || current.epoch != lease.epoch
                {
                    return Err(custom_persist_error(
                        PersistConflict::LeaseMismatch {
                            expected_worker_id: lease.holder_worker_id.clone(),
                            expected_epoch: lease.epoch,
                            actual_worker_id: Some(current.holder_worker_id),
                            actual_epoch: Some(current.epoch),
                        }
                        .into(),
                    ));
                }
                trx.clear(&lease_key);
                trx.clear(&lease_by_worker_key);
                Ok(())
            }
        })
    }

    pub(super) fn list_worker_worlds(
        &self,
        worker_id: &str,
        now_ns: u64,
        limit: u32,
        universe_filter: Option<&[UniverseId]>,
    ) -> Result<Vec<NodeWorldRuntimeInfo>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.lease_by_worker_space(worker_id);
        let (begin, end) = space.range();
        let universe_filter: Option<Vec<String>> =
            universe_filter.map(|ids| ids.iter().map(std::string::ToString::to_string).collect());
        let world_ids: Vec<(UniverseId, WorldId)> = self.run(|trx, _| {
            let space = space.clone();
            let begin = begin.clone();
            let end = end.clone();
            let universe_filter = universe_filter.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(limit.max(64) as usize);
                let mut worlds = Vec::new();
                loop {
                    let kvs = trx.get_range(&range, 1, false).await?;
                    if kvs.is_empty() {
                        break;
                    }
                    let mut last_key = None::<Vec<u8>>;
                    for kv in kvs.iter() {
                        last_key = Some(kv.key().to_vec());
                        let (universe_id_str, world_id_str) =
                            space.unpack::<(String, String)>(kv.key()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if universe_filter.as_ref().is_some_and(|filter| {
                            !filter.iter().any(|candidate| candidate == &universe_id_str)
                        }) {
                            continue;
                        }
                        let lease: WorldLease = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if lease.expires_at_ns < now_ns {
                            continue;
                        }
                        let universe = UniverseId::from_str(&universe_id_str).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        let world_id = WorldId::from_str(&world_id_str).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        worlds.push((universe, world_id));
                        if worlds.len() >= limit as usize {
                            return Ok(worlds);
                        }
                    }
                    let Some(last_key) = last_key else {
                        break;
                    };
                    range.begin = KeySelector::first_greater_than(last_key);
                }
                Ok(worlds)
            }
        })?;
        let mut worlds = Vec::with_capacity(world_ids.len());
        for (universe, world_id) in world_ids {
            worlds.push(NodeWorldRuntimeInfo {
                universe_id: universe,
                info: self.world_runtime_info(universe, world_id, now_ns)?,
            });
        }
        Ok(worlds)
    }

    pub(super) fn journal_append_batch_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
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
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
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

    pub(super) fn inbox_commit_cursor_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
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
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
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
                if let Some(current) = &actual_cursor
                    && new_cursor < *current
                {
                    return Err(custom_persist_error(PersistError::validation(
                        "inbox cursor cannot regress",
                    )));
                }
                let inbox_key = build_inbox_key(&inbox_space, &new_cursor);
                if trx.get(&inbox_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "inbox sequence {new_cursor} does not exist"
                    ))));
                }
                trx.set(&cursor_key, new_cursor.as_bytes());
                self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                    .await?;
                Ok(())
            }
        })
    }

    pub(super) fn drain_inbox_to_journal_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
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
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
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
                if let Some(current) = &actual_cursor
                    && new_cursor < *current
                {
                    return Err(custom_persist_error(PersistError::validation(
                        "inbox cursor cannot regress",
                    )));
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
                self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                    .await?;
                Ok(actual_head)
            }
        })
    }

    pub(super) fn materialize_query_projections_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        materialization: QueryProjectionMaterialization,
    ) -> Result<(), PersistError> {
        validate_query_projection_materialization(&materialization)?;
        let head_key = self.projection_head_key(universe, world);
        let cell_root = self.projection_cell_root(universe, world);
        let workspace_root = self.projection_workspace_root(universe, world);
        let head_bytes = self.encode(&materialization.head)?;
        let cell_records: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workflows
            .iter()
            .flat_map(|workflow| {
                workflow.cells.iter().map(|cell| {
                    let key = self.projection_cell_key(
                        universe,
                        world,
                        &workflow.workflow,
                        &cell.key_hash,
                    );
                    let value = self.encode(cell)?;
                    Ok((key, value))
                })
            })
            .collect::<Result<_, PersistError>>()?;
        let workspace_records: Vec<(Vec<u8>, Vec<u8>)> = materialization
            .workspaces
            .iter()
            .map(|workspace| {
                let key = self.projection_workspace_key(universe, world, &workspace.workspace);
                let value = self.encode(workspace)?;
                Ok((key, value))
            })
            .collect::<Result<_, PersistError>>()?;
        self.run(|trx, _| {
            let head_key = head_key.clone();
            let cell_root = cell_root.clone();
            let workspace_root = workspace_root.clone();
            let head_bytes = head_bytes.clone();
            let cell_records = cell_records.clone();
            let workspace_records = workspace_records.clone();
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                trx.set(&head_key, &head_bytes);
                let (begin, end) = cell_root.range();
                trx.clear_range(&begin, &end);
                for (key, value) in &cell_records {
                    trx.set(key, value);
                }
                let (workspace_begin, workspace_end) = workspace_root.range();
                trx.clear_range(&workspace_begin, &workspace_end);
                for (key, value) in &workspace_records {
                    trx.set(key, value);
                }
                Ok(())
            }
        })
    }

    pub(super) fn apply_query_projection_delta_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        delta: QueryProjectionDelta,
    ) -> Result<(), PersistError> {
        validate_query_projection_delta(&delta)?;
        let head_key = self.projection_head_key(universe, world);
        let head_bytes = self.encode(&delta.head)?;
        let cell_upserts: Vec<(Vec<u8>, Vec<u8>)> = delta
            .cell_upserts
            .iter()
            .map(|cell| {
                let key = self.projection_cell_key(universe, world, &cell.workflow, &cell.key_hash);
                let value = self.encode(cell)?;
                Ok((key, value))
            })
            .collect::<Result<_, PersistError>>()?;
        let cell_deletes: Vec<Vec<u8>> = delta
            .cell_deletes
            .iter()
            .map(|cell| self.projection_cell_key(universe, world, &cell.workflow, &cell.key_hash))
            .collect();
        let workspace_upserts: Vec<(Vec<u8>, Vec<u8>)> = delta
            .workspace_upserts
            .iter()
            .map(|workspace| {
                let key = self.projection_workspace_key(universe, world, &workspace.workspace);
                let value = self.encode(workspace)?;
                Ok((key, value))
            })
            .collect::<Result<_, PersistError>>()?;
        let workspace_deletes: Vec<Vec<u8>> = delta
            .workspace_deletes
            .iter()
            .map(|workspace| self.projection_workspace_key(universe, world, &workspace.workspace))
            .collect();
        self.run(|trx, _| {
            let head_key = head_key.clone();
            let head_bytes = head_bytes.clone();
            let cell_upserts = cell_upserts.clone();
            let cell_deletes = cell_deletes.clone();
            let workspace_upserts = workspace_upserts.clone();
            let workspace_deletes = workspace_deletes.clone();
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                trx.set(&head_key, &head_bytes);
                for key in &cell_deletes {
                    trx.clear(key);
                }
                for (key, value) in &cell_upserts {
                    trx.set(key, value);
                }
                for key in &workspace_deletes {
                    trx.clear(key);
                }
                for (key, value) in &workspace_upserts {
                    trx.set(key, value);
                }
                Ok(())
            }
        })
    }

    pub(super) fn snapshot_index_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_snapshot_record(&record)?;
        let key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(record.height, "snapshot height")?,));
        let value = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            let record = record.clone();
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                if let Some(existing) = trx.get(&key, false).await? {
                    let existing_record: SnapshotRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if existing_record == record {
                        return Ok(());
                    }
                    if can_upgrade_snapshot_record(&existing_record, &record) {
                        trx.set(&key, &value);
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

    pub(super) fn snapshot_commit_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SnapshotCommitRequest,
    ) -> Result<SnapshotCommitResult, PersistError> {
        self.verify_current_lease(universe, world, lease, now_ns)?;
        self.snapshot_commit(universe, world, request)
    }

    pub(super) fn snapshot_promote_baseline_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SnapshotRecord,
    ) -> Result<(), PersistError> {
        validate_baseline_promotion_record(&record)?;
        let snapshot_key = self
            .snapshot_by_height_space(universe, world)
            .pack(&(self.to_i64(record.height, "snapshot height")?,));
        let baseline_key = self.baseline_active_key(universe, world);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let record_bytes = self.encode(&record)?;
        self.run(|trx, _| {
            let snapshot_key = snapshot_key.clone();
            let baseline_key = baseline_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let record_bytes = record_bytes.clone();
            let record = record.clone();
            let lease = lease.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
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
                let mut meta = match trx.get(&meta_key, false).await? {
                    Some(bytes) => {
                        serde_cbor::from_slice::<WorldMeta>(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?
                    }
                    None => sample_world_meta(world),
                };
                meta.active_baseline_height = Some(record.height);
                meta.manifest_hash = record.manifest_hash.clone();
                let meta_bytes = serde_cbor::to_vec(&meta)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&baseline_key, &record_bytes);
                trx.set(&meta_key, &meta_bytes);
                trx.set(&catalog_key, &meta_bytes);
                self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                    .await?;
                Ok(())
            }
        })
    }

    pub(super) fn segment_index_put_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        record: SegmentIndexRecord,
    ) -> Result<(), PersistError> {
        self.verify_current_lease(universe, world, lease, now_ns)?;
        self.segment_index_put(universe, world, record)
    }

    pub(super) fn segment_export_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        request: SegmentExportRequest,
    ) -> Result<SegmentExportResult, PersistError> {
        self.verify_current_lease(universe, world, lease, now_ns)?;
        self.segment_export(universe, world, request)
    }

    pub(super) fn publish_effect_dispatches_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[EffectDispatchItem],
    ) -> Result<u32, PersistError> {
        if items.is_empty() {
            return Ok(0);
        }
        let normalized: Vec<_> = items
            .iter()
            .map(|item| {
                if item.world_id != world {
                    return Err(PersistError::validation(format!(
                        "effect dispatch world mismatch: expected {world}, got {}",
                        item.world_id
                    )));
                }
                self.normalize_effect_dispatch_item(universe, item.clone())
            })
            .collect::<Result<_, _>>()?;
        let pending_count_key = self.pending_effect_count_key(universe, world);
        self.run(|trx, _| {
            let lease = lease.clone();
            let normalized = normalized.clone();
            let pending_count_key = pending_count_key.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                let mut published = 0u32;
                for (idx, item) in normalized.iter().enumerate() {
                    let dedupe_key = self.effect_dedupe_key(universe, &item.intent_hash);
                    if trx.get(&dedupe_key, false).await?.is_some() {
                        continue;
                    }
                    let space = self.effects_pending_space(universe, item.shard);
                    let pending_key =
                        space.pack_with_versionstamp(&Versionstamp::incomplete(idx as u16));
                    let value = self.encode(item).map_err(custom_persist_error)?;
                    let status = self
                        .encode(&EffectDedupeRecord {
                            status: DispatchStatus::Pending,
                            completed_at_ns: None,
                            gc_after_ns: None,
                        })
                        .map_err(custom_persist_error)?;
                    trx.atomic_op(&pending_key, &value, MutationType::SetVersionstampedKey);
                    trx.set(&dedupe_key, &status);
                    published = published.saturating_add(1);
                }
                if published > 0 {
                    let current = match trx.get(&pending_count_key, false).await? {
                        Some(bytes) => decode_u64_static(bytes.as_ref())?,
                        None => 0,
                    };
                    trx.set(
                        &pending_count_key,
                        &current.saturating_add(published as u64).to_be_bytes(),
                    );
                }
                self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                    .await?;
                Ok(published)
            }
        })
    }

    pub(super) fn claim_pending_effects_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<(QueueSeq, EffectDispatchItem)>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = self.effects_pending_root(universe);
        let (begin, end) = root.range();
        let candidates: Vec<(Vec<u8>, EffectDispatchItem, QueueSeq)> = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, limit as usize, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let item: EffectDispatchItem = serde_cbor::from_slice(kv.value().as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if item.world_id != world {
                        continue;
                    }
                    let space = self.effects_pending_space(universe, item.shard);
                    let prefix_len = space.bytes().len();
                    let seq = QueueSeq::new(kv.key()[prefix_len..].to_vec());
                    candidates.push((kv.key().to_vec(), item, seq));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;

        let mut claimed = Vec::new();
        for (pending_key, item, seq) in candidates {
            let inflight_key =
                build_inbox_key(&self.effects_inflight_space(universe, item.shard), &seq);
            let dedupe_key = self.effect_dedupe_key(universe, &item.intent_hash);
            let worker_id = worker_id.to_string();
            let attempt = self.run(|trx, _| {
                let pending_key = pending_key.clone();
                let inflight_key = inflight_key.clone();
                let dedupe_key = dedupe_key.clone();
                let item = item.clone();
                let seq = seq.clone();
                let worker_id = worker_id.clone();
                async move {
                    let Some(bytes) = trx.get(&pending_key, false).await? else {
                        return Ok(None);
                    };
                    let pending_item: EffectDispatchItem = serde_cbor::from_slice(bytes.as_ref())
                        .map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    if pending_item.world_id != world {
                        return Ok(None);
                    }
                    let inflight = EffectInFlightItem {
                        dispatch: pending_item.clone(),
                        claim_until_ns: now_ns.saturating_add(claim_ttl_ns),
                        worker_id: Some(worker_id),
                    };
                    trx.clear(&pending_key);
                    trx.set(
                        &inflight_key,
                        &self.encode(&inflight).map_err(custom_persist_error)?,
                    );
                    trx.set(
                        &dedupe_key,
                        &self
                            .encode(&EffectDedupeRecord {
                                status: DispatchStatus::InFlight,
                                completed_at_ns: None,
                                gc_after_ns: None,
                            })
                            .map_err(custom_persist_error)?,
                    );
                    self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                        .await?;
                    Ok(Some((seq, item)))
                }
            })?;
            if let Some((seq, item)) = attempt {
                claimed.push((seq, item));
            }
        }
        Ok(claimed)
    }

    pub(super) fn ack_effect_dispatch_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        shard: u16,
        seq: QueueSeq,
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        let receipt_item = self.normalize_inbox_item(universe, InboxItem::Receipt(receipt))?;
        let receipt = match &receipt_item {
            InboxItem::Receipt(receipt) => receipt,
            _ => unreachable!("normalized effect receipt remains receipt"),
        };
        let inflight_key = build_inbox_key(&self.effects_inflight_space(universe, shard), &seq);
        let dedupe_key = self.effect_dedupe_key(universe, &receipt.intent_hash);
        let effect_gc_key = self.effect_dedupe_gc_key(
            universe,
            gc_bucket_for(
                now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns),
                self.config.dedupe_gc.bucket_width_ns,
            ),
            &receipt.intent_hash,
        );
        let pending_count_key = self.pending_effect_count_key(universe, world);
        let inbox_space = self.inbox_entry_space(universe, world);
        let notify_key = self.notify_counter_key(universe, world);
        let value = self.encode(&receipt_item)?;
        let worker_id = worker_id.to_string();
        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));
            let op_result: Result<(), TxRetryError> = block_on(async {
                let pending_count_key = pending_count_key.clone();
                let Some(inflight_bytes) = trx
                    .get(&inflight_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                else {
                    return Err(TxRetryError::Persist(PersistError::not_found(format!(
                        "effect inflight seq {seq}"
                    ))));
                };
                let inflight: EffectInFlightItem = serde_cbor::from_slice(inflight_bytes.as_ref())
                    .map_err(|err| TxRetryError::Persist(PersistError::backend(err.to_string())))?;
                if inflight.dispatch.world_id != world {
                    return Err(TxRetryError::Persist(PersistError::validation(format!(
                        "effect inflight world mismatch: expected {world}, got {}",
                        inflight.dispatch.world_id
                    ))));
                }
                if inflight.worker_id.as_deref() != Some(worker_id.as_str()) {
                    return Err(TxRetryError::Persist(PersistError::validation(format!(
                        "effect inflight seq {seq} not owned by worker {worker_id}"
                    ))));
                }
                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                trx.clear(&inflight_key);
                let pending = match trx.get(&pending_count_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.set(&pending_count_key, &pending.saturating_sub(1).to_be_bytes());
                trx.set(
                    &dedupe_key,
                    &self
                        .encode(&EffectDedupeRecord {
                            status: DispatchStatus::Complete,
                            completed_at_ns: Some(now_ns),
                            gc_after_ns: Some(
                                now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns),
                            ),
                        })
                        .map_err(TxRetryError::Persist)?,
                );
                trx.set(&effect_gc_key, &[]);
                self.mark_world_pending_inbox_in_tx(&trx, universe, world, 0)
                    .await
                    .map_err(map_fdb_binding_error)
                    .map_err(TxRetryError::Persist)?;
                Ok(())
            });

            match op_result {
                Ok(()) => match block_on(trx.commit()) {
                    Ok(_) => return Ok(()),
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

    pub(super) fn retain_effect_dispatches_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        valid_intents: &std::collections::HashSet<[u8; 32]>,
        now_ns: u64,
    ) -> Result<u32, PersistError> {
        let pending_root = self.effects_pending_root(universe);
        let inflight_root = self.effects_inflight_root(universe);
        let valid_intents = valid_intents.clone();
        let candidates: Vec<(Vec<u8>, Vec<u8>)> = self.run(|trx, _| {
            let (pending_begin, pending_end) = pending_root.range();
            let (inflight_begin, inflight_end) = inflight_root.range();
            let valid_intents = valid_intents.clone();
            async move {
                let mut candidates = Vec::new();

                let mut scan_pending = RangeOption::from((pending_begin, pending_end));
                scan_pending.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_pending.limit = Some(1024);
                    let kvs = trx.get_range(&scan_pending, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: EffectDispatchItem = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.world_id != world || item.intent_hash.len() != 32 {
                            continue;
                        }
                        let mut hash = [0u8; 32];
                        hash.copy_from_slice(&item.intent_hash);
                        if !valid_intents.contains(&hash) {
                            candidates.push((kv.key().to_vec(), item.intent_hash));
                        }
                    }
                    scan_pending.begin = KeySelector::first_greater_than(last_key);
                }

                let mut scan_inflight = RangeOption::from((inflight_begin, inflight_end));
                scan_inflight.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_inflight.limit = Some(1024);
                    let kvs = trx.get_range(&scan_inflight, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: EffectInFlightItem = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.dispatch.world_id != world || item.dispatch.intent_hash.len() != 32
                        {
                            continue;
                        }
                        let mut hash = [0u8; 32];
                        hash.copy_from_slice(&item.dispatch.intent_hash);
                        if !valid_intents.contains(&hash) {
                            candidates.push((kv.key().to_vec(), item.dispatch.intent_hash));
                        }
                    }
                    scan_inflight.begin = KeySelector::first_greater_than(last_key);
                }

                Ok(candidates)
            }
        })?;
        if candidates.is_empty() {
            return Ok(0);
        }

        let pending_count_key = self.pending_effect_count_key(universe, world);
        let dropped = self.run(|trx, _| {
            let candidates = candidates.clone();
            let pending_count_key = pending_count_key.clone();
            async move {
                let mut dropped = 0u32;
                for (key, intent_hash) in &candidates {
                    let Some(_) = trx.get(key, false).await? else {
                        continue;
                    };
                    trx.clear(key);
                    let dedupe_key = self.effect_dedupe_key(universe, intent_hash);
                    let effect_gc_key = self.effect_dedupe_gc_key(
                        universe,
                        gc_bucket_for(
                            now_ns.saturating_add(self.config.dedupe_gc.effect_retention_ns),
                            self.config.dedupe_gc.bucket_width_ns,
                        ),
                        intent_hash,
                    );
                    trx.set(
                        &dedupe_key,
                        &self
                            .encode(&EffectDedupeRecord {
                                status: DispatchStatus::Failed,
                                completed_at_ns: Some(now_ns),
                                gc_after_ns: Some(
                                    now_ns
                                        .saturating_add(self.config.dedupe_gc.effect_retention_ns),
                                ),
                            })
                            .map_err(custom_persist_error)?,
                    );
                    trx.set(&effect_gc_key, &[]);
                    dropped = dropped.saturating_add(1);
                }
                if dropped > 0 {
                    let pending = trx
                        .get(&pending_count_key, false)
                        .await?
                        .map(|bytes| decode_u64_static(bytes.as_ref()))
                        .transpose()?
                        .unwrap_or(0);
                    trx.set(
                        &pending_count_key,
                        &pending.saturating_sub(dropped as u64).to_be_bytes(),
                    );
                    self.mark_world_pending_inbox_in_tx(&trx, universe, world, 0)
                        .await?;
                }
                Ok(dropped)
            }
        })?;
        Ok(dropped)
    }

    pub(super) fn requeue_expired_effect_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        if limit == 0 {
            return Ok(0);
        }
        let root = self.effects_inflight_root(universe);
        let (begin, end) = root.range();
        let candidates: Vec<(Vec<u8>, EffectInFlightItem, QueueSeq)> = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, limit as usize, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let item: EffectInFlightItem = serde_cbor::from_slice(kv.value().as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if item.claim_until_ns > now_ns {
                        continue;
                    }
                    let space = self.effects_inflight_space(universe, item.dispatch.shard);
                    let prefix_len = space.bytes().len();
                    let seq = QueueSeq::new(kv.key()[prefix_len..].to_vec());
                    candidates.push((kv.key().to_vec(), item, seq));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;

        let mut requeued = 0u32;
        for (inflight_key, inflight, seq) in candidates {
            let pending_key = build_inbox_key(
                &self.effects_pending_space(universe, inflight.dispatch.shard),
                &seq,
            );
            let dedupe_key = self.effect_dedupe_key(universe, &inflight.dispatch.intent_hash);
            let moved = self.run(|trx, _| {
                let inflight_key = inflight_key.clone();
                let pending_key = pending_key.clone();
                let dedupe_key = dedupe_key.clone();
                let inflight = inflight.clone();
                async move {
                    let Some(bytes) = trx.get(&inflight_key, false).await? else {
                        return Ok(false);
                    };
                    let live: EffectInFlightItem =
                        serde_cbor::from_slice(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if live.claim_until_ns > now_ns {
                        return Ok(false);
                    }
                    trx.clear(&inflight_key);
                    trx.set(
                        &pending_key,
                        &self
                            .encode(&inflight.dispatch)
                            .map_err(custom_persist_error)?,
                    );
                    trx.set(
                        &dedupe_key,
                        &self
                            .encode(&EffectDedupeRecord {
                                status: DispatchStatus::Pending,
                                completed_at_ns: None,
                                gc_after_ns: None,
                            })
                            .map_err(custom_persist_error)?,
                    );
                    self.refresh_world_ready_state_in_trx(
                        &trx,
                        universe,
                        inflight.dispatch.world_id,
                        now_ns,
                    )
                    .await?;
                    Ok(true)
                }
            })?;
            if moved {
                requeued = requeued.saturating_add(1);
            }
        }
        Ok(requeued)
    }

    pub(super) fn publish_due_timers_guarded(
        &self,
        universe: UniverseId,
        world: WorldId,
        lease: &WorldLease,
        now_ns: u64,
        items: &[TimerDueItem],
    ) -> Result<u32, PersistError> {
        if items.is_empty() {
            return Ok(0);
        }
        let next_timer_due_key = self.next_timer_due_key(universe, world);
        self.run(|trx, _| {
            let lease = lease.clone();
            let items = items.to_vec();
            let next_timer_due_key = next_timer_due_key.clone();
            async move {
                let _ = self
                    .ensure_live_lease(&trx, universe, world, &lease, now_ns)
                    .await?;
                let mut published = 0u32;
                let mut next_due = trx
                    .get(&next_timer_due_key, false)
                    .await?
                    .map(|bytes| decode_u64_static(bytes.as_ref()))
                    .transpose()?;
                for item in &items {
                    if item.world_id != world {
                        return Err(custom_persist_error(PersistError::validation(format!(
                            "timer due world mismatch: expected {world}, got {}",
                            item.world_id
                        ))));
                    }
                    let dedupe_key = self.timer_dedupe_key(universe, &item.intent_hash);
                    if trx.get(&dedupe_key, false).await?.is_some() {
                        continue;
                    }
                    let due_key = self.timers_due_space(universe, item.shard).pack(&(
                        to_i64_static(item.time_bucket, "timer time bucket")?,
                        to_i64_static(item.deliver_at_ns, "timer deliver_at")?,
                        item.intent_hash.as_slice(),
                    ));
                    trx.set(&due_key, &self.encode(item).map_err(custom_persist_error)?);
                    trx.set(
                        &dedupe_key,
                        &self
                            .encode(&TimerDedupeRecord {
                                status: DeliveredStatus::Pending,
                                completed_at_ns: None,
                                gc_after_ns: None,
                            })
                            .map_err(custom_persist_error)?,
                    );
                    next_due = Some(match next_due {
                        Some(existing) => existing.min(item.deliver_at_ns),
                        None => item.deliver_at_ns,
                    });
                    published = published.saturating_add(1);
                }
                if let Some(next_due) = next_due {
                    trx.set(&next_timer_due_key, &next_due.to_be_bytes());
                }
                self.refresh_world_ready_state_in_trx(&trx, universe, world, now_ns)
                    .await?;
                Ok(published)
            }
        })
    }

    pub(super) fn claim_due_timers_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        now_ns: u64,
        claim_ttl_ns: u64,
        limit: u32,
    ) -> Result<Vec<TimerDueItem>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let root = self.timers_due_root(universe);
        let (begin, end) = root.range();
        let candidates: Vec<(Vec<u8>, TimerDueItem)> = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, limit as usize, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let item: TimerDueItem =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if item.world_id != world || item.deliver_at_ns > now_ns {
                        continue;
                    }
                    candidates.push((kv.key().to_vec(), item));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;

        let mut claimed = Vec::new();
        for (due_key, item) in candidates {
            let inflight_key = self.timer_inflight_key(universe, item.shard, &item.intent_hash);
            let worker_id = worker_id.to_string();
            let attempt = self.run(|trx, _| {
                let due_key = due_key.clone();
                let inflight_key = inflight_key.clone();
                let item = item.clone();
                let worker_id = worker_id.clone();
                async move {
                    let Some(bytes) = trx.get(&due_key, false).await? else {
                        return Ok(None);
                    };
                    let due: TimerDueItem =
                        serde_cbor::from_slice(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if due.world_id != world || due.deliver_at_ns > now_ns {
                        return Ok(None);
                    }
                    let inflight = FdbTimerInFlightItem {
                        due: due.clone(),
                        claim: TimerClaim {
                            intent_hash: due.intent_hash.clone(),
                            claim_until_ns: now_ns.saturating_add(claim_ttl_ns),
                            worker_id: Some(worker_id),
                        },
                    };
                    trx.clear(&due_key);
                    trx.set(
                        &inflight_key,
                        &self.encode(&inflight).map_err(custom_persist_error)?,
                    );
                    Ok(Some(item))
                }
            })?;
            if let Some(item) = attempt {
                claimed.push(item);
            }
        }
        self.refresh_world_next_timer_due_and_ready(universe, world, now_ns)?;
        Ok(claimed)
    }

    pub(super) fn ack_timer_delivery_with_receipt(
        &self,
        universe: UniverseId,
        world: WorldId,
        worker_id: &str,
        intent_hash: &[u8],
        now_ns: u64,
        receipt: ReceiptIngress,
    ) -> Result<(), PersistError> {
        let receipt_item = self.normalize_inbox_item(universe, InboxItem::Receipt(receipt))?;
        let InboxItem::Receipt(_) = &receipt_item else {
            unreachable!("normalized timer receipt remains receipt");
        };
        let timer_root = self.timer_inflight_root(universe);
        let (begin, end) = timer_root.range();
        let candidate = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(128);
                loop {
                    let kvs = trx.get_range(&range, 1, false).await?;
                    if kvs.is_empty() {
                        return Ok(None);
                    }
                    let mut last_key = None::<Vec<u8>>;
                    for kv in kvs.iter() {
                        last_key = Some(kv.key().to_vec());
                        let item: FdbTimerInFlightItem =
                            serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.due.intent_hash == intent_hash {
                            return Ok(Some((kv.key().to_vec(), item)));
                        }
                    }
                    let Some(last_key) = last_key else {
                        return Ok(None);
                    };
                    range.begin = KeySelector::first_greater_than(last_key);
                }
            }
        })?;
        let Some((inflight_key, inflight)) = candidate else {
            return Err(PersistError::not_found(format!(
                "timer inflight {}",
                hex::encode(intent_hash)
            )));
        };
        if inflight.due.world_id != world {
            return Err(PersistError::validation(format!(
                "timer inflight world mismatch: expected {world}, got {}",
                inflight.due.world_id
            )));
        }
        if inflight.claim.worker_id.as_deref() != Some(worker_id) {
            return Err(PersistError::validation(format!(
                "timer inflight {} not owned by worker {worker_id}",
                hex::encode(intent_hash)
            )));
        }
        let dedupe_key = self.timer_dedupe_key(universe, intent_hash);
        let timer_gc_key = self.timer_dedupe_gc_key(
            universe,
            gc_bucket_for(
                now_ns.saturating_add(self.config.dedupe_gc.timer_retention_ns),
                self.config.dedupe_gc.bucket_width_ns,
            ),
            intent_hash,
        );
        let inbox_space = self.inbox_entry_space(universe, world);
        let notify_key = self.notify_counter_key(universe, world);
        let value = self.encode(&receipt_item)?;
        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));
            let op_result: Result<(), TxRetryError> = block_on(async {
                if trx
                    .get(&inflight_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                    .is_none()
                {
                    return Err(TxRetryError::Persist(PersistError::not_found(format!(
                        "timer inflight {}",
                        hex::encode(intent_hash)
                    ))));
                }
                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                trx.clear(&inflight_key);
                trx.set(
                    &dedupe_key,
                    &self
                        .encode(&TimerDedupeRecord {
                            status: DeliveredStatus::Delivered,
                            completed_at_ns: Some(now_ns),
                            gc_after_ns: Some(
                                now_ns.saturating_add(self.config.dedupe_gc.timer_retention_ns),
                            ),
                        })
                        .map_err(TxRetryError::Persist)?,
                );
                trx.set(&timer_gc_key, &[]);
                self.mark_world_pending_inbox_in_tx(&trx, universe, world, 0)
                    .await
                    .map_err(map_fdb_binding_error)
                    .map_err(TxRetryError::Persist)?;
                Ok(())
            });
            match op_result {
                Ok(()) => match block_on(trx.commit()) {
                    Ok(_) => break,
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
        self.refresh_world_next_timer_due_and_ready(universe, world, 0)
    }

    pub(super) fn outstanding_intent_hashes_for_world(
        &self,
        universe: UniverseId,
        world: WorldId,
        now_ns: u64,
    ) -> Result<Vec<[u8; 32]>, PersistError> {
        let effects_pending_root = self.effects_pending_root(universe);
        let effects_inflight_root = self.effects_inflight_root(universe);
        let timers_due_root = self.timers_due_root(universe);
        let timers_inflight_root = self.timer_inflight_root(universe);
        let hashes = self.run(|trx, _| {
            let (effects_pending_begin, effects_pending_end) = effects_pending_root.range();
            let (effects_inflight_begin, effects_inflight_end) = effects_inflight_root.range();
            let (timers_due_begin, timers_due_end) = timers_due_root.range();
            let (timers_inflight_begin, timers_inflight_end) = timers_inflight_root.range();
            async move {
                let mut hashes = Vec::new();
                let mut scan_pending =
                    RangeOption::from((effects_pending_begin, effects_pending_end));
                scan_pending.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_pending.limit = Some(1024);
                    let kvs = trx.get_range(&scan_pending, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: EffectDispatchItem = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.world_id == world && item.intent_hash.len() == 32 {
                            let mut hash = [0u8; 32];
                            hash.copy_from_slice(&item.intent_hash);
                            hashes.push(hash);
                        }
                    }
                    scan_pending.begin = KeySelector::first_greater_than(last_key);
                }

                let mut scan_inflight =
                    RangeOption::from((effects_inflight_begin, effects_inflight_end));
                scan_inflight.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_inflight.limit = Some(1024);
                    let kvs = trx.get_range(&scan_inflight, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: EffectInFlightItem = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.dispatch.world_id == world && item.dispatch.intent_hash.len() == 32
                        {
                            let mut hash = [0u8; 32];
                            hash.copy_from_slice(&item.dispatch.intent_hash);
                            hashes.push(hash);
                        }
                    }
                    scan_inflight.begin = KeySelector::first_greater_than(last_key);
                }

                let mut scan_due = RangeOption::from((timers_due_begin, timers_due_end));
                scan_due.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_due.limit = Some(1024);
                    let kvs = trx.get_range(&scan_due, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: TimerDueItem = serde_cbor::from_slice(kv.value().as_ref())
                            .map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.world_id == world
                            && item.deliver_at_ns <= now_ns
                            && item.intent_hash.len() == 32
                        {
                            let mut hash = [0u8; 32];
                            hash.copy_from_slice(&item.intent_hash);
                            hashes.push(hash);
                        }
                    }
                    scan_due.begin = KeySelector::first_greater_than(last_key);
                }

                let mut scan_timers_inflight =
                    RangeOption::from((timers_inflight_begin, timers_inflight_end));
                scan_timers_inflight.mode = foundationdb::options::StreamingMode::WantAll;
                loop {
                    scan_timers_inflight.limit = Some(1024);
                    let kvs = trx.get_range(&scan_timers_inflight, 1, false).await?;
                    let Some(last_key) = kvs.last().map(|kv| kv.key().to_vec()) else {
                        break;
                    };
                    for kv in kvs.iter() {
                        let item: FdbTimerInFlightItem =
                            serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        if item.due.world_id == world && item.due.intent_hash.len() == 32 {
                            let mut hash = [0u8; 32];
                            hash.copy_from_slice(&item.due.intent_hash);
                            hashes.push(hash);
                        }
                    }
                    scan_timers_inflight.begin = KeySelector::first_greater_than(last_key);
                }

                hashes.sort_unstable();
                hashes.dedup();
                Ok(hashes)
            }
        })?;
        Ok(hashes)
    }

    pub(super) fn requeue_expired_timer_claims(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        if limit == 0 {
            return Ok(0);
        }
        let root = self.timer_inflight_root(universe);
        let (begin, end) = root.range();
        let candidates: Vec<(Vec<u8>, FdbTimerInFlightItem)> = self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, limit as usize, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let item: FdbTimerInFlightItem = serde_cbor::from_slice(kv.value().as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if item.claim.claim_until_ns > now_ns {
                        continue;
                    }
                    candidates.push((kv.key().to_vec(), item));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;

        let mut requeued = 0u32;
        for (inflight_key, item) in candidates {
            let due_key = self.timers_due_space(universe, item.due.shard).pack(&(
                self.to_i64(item.due.time_bucket, "timer time bucket")?,
                self.to_i64(item.due.deliver_at_ns, "timer deliver_at")?,
                item.due.intent_hash.as_slice(),
            ));
            let moved = self.run(|trx, _| {
                let inflight_key = inflight_key.clone();
                let due_key = due_key.clone();
                let item = item.clone();
                async move {
                    let Some(bytes) = trx.get(&inflight_key, false).await? else {
                        return Ok(false);
                    };
                    let live: FdbTimerInFlightItem = serde_cbor::from_slice(bytes.as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if live.claim.claim_until_ns > now_ns {
                        return Ok(false);
                    }
                    trx.clear(&inflight_key);
                    trx.set(
                        &due_key,
                        &self.encode(&item.due).map_err(custom_persist_error)?,
                    );
                    Ok(true)
                }
            })?;
            if moved {
                self.refresh_world_next_timer_due_and_ready(universe, item.due.world_id, now_ns)?;
                requeued = requeued.saturating_add(1);
            }
        }
        Ok(requeued)
    }

    pub(super) fn sweep_effect_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        if limit == 0 {
            return Ok(0);
        }
        let root = self.effect_dedupe_gc_root(universe);
        let (begin, end) = root.range();
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<(u64, Vec<u8>, Vec<u8>)> = self.run(|trx, _| {
            let root = root.clone();
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let (bucket, intent_hash) =
                        root.unpack::<(i64, Vec<u8>)>(kv.key()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    let bucket = u64::try_from(bucket).map_err(|_| {
                        custom_persist_error(PersistError::backend("negative gc bucket"))
                    })?;
                    if bucket > max_bucket {
                        break;
                    }
                    candidates.push((bucket, intent_hash, kv.key().to_vec()));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;
        let mut swept = 0u32;
        for (_bucket, intent_hash, gc_key) in candidates {
            let dedupe_key = self.effect_dedupe_key(universe, &intent_hash);
            let deleted = self.run(|trx, _| {
                let gc_key = gc_key.clone();
                let dedupe_key = dedupe_key.clone();
                async move {
                    let maybe = trx.get(&dedupe_key, false).await?;
                    let should_delete = match maybe {
                        Some(bytes) => {
                            let record: EffectDedupeRecord = serde_cbor::from_slice(bytes.as_ref())
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            record
                                .gc_after_ns
                                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns)
                        }
                        None => true,
                    };
                    if should_delete {
                        trx.clear(&dedupe_key);
                        trx.clear(&gc_key);
                    }
                    Ok(should_delete)
                }
            })?;
            if deleted {
                swept = swept.saturating_add(1);
            }
        }
        Ok(swept)
    }

    pub(super) fn sweep_timer_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        if limit == 0 {
            return Ok(0);
        }
        let root = self.timer_dedupe_gc_root(universe);
        let (begin, end) = root.range();
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<(u64, Vec<u8>, Vec<u8>)> = self.run(|trx, _| {
            let root = root.clone();
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let (bucket, intent_hash) =
                        root.unpack::<(i64, Vec<u8>)>(kv.key()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    let bucket = u64::try_from(bucket).map_err(|_| {
                        custom_persist_error(PersistError::backend("negative gc bucket"))
                    })?;
                    if bucket > max_bucket {
                        break;
                    }
                    candidates.push((bucket, intent_hash, kv.key().to_vec()));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;
        let mut swept = 0u32;
        for (_bucket, intent_hash, gc_key) in candidates {
            let dedupe_key = self.timer_dedupe_key(universe, &intent_hash);
            let deleted = self.run(|trx, _| {
                let gc_key = gc_key.clone();
                let dedupe_key = dedupe_key.clone();
                async move {
                    let maybe = trx.get(&dedupe_key, false).await?;
                    let should_delete = match maybe {
                        Some(bytes) => {
                            let record: TimerDedupeRecord = serde_cbor::from_slice(bytes.as_ref())
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            record
                                .gc_after_ns
                                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns)
                        }
                        None => true,
                    };
                    if should_delete {
                        trx.clear(&dedupe_key);
                        trx.clear(&gc_key);
                    }
                    Ok(should_delete)
                }
            })?;
            if deleted {
                swept = swept.saturating_add(1);
            }
        }
        Ok(swept)
    }

    pub(super) fn sweep_portal_dedupe_gc(
        &self,
        universe: UniverseId,
        now_ns: u64,
        limit: u32,
    ) -> Result<u32, PersistError> {
        if limit == 0 {
            return Ok(0);
        }
        let root = self.portal_dedupe_gc_root(universe);
        let (begin, end) = root.range();
        let max_bucket = gc_bucket_for(now_ns, self.config.dedupe_gc.bucket_width_ns);
        let candidates: Vec<(u64, WorldId, Vec<u8>, Vec<u8>)> = self.run(|trx, _| {
            let root = root.clone();
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let range = RangeOption::from((begin, end));
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut candidates = Vec::new();
                for kv in kvs.iter() {
                    let (bucket, world_id_str, message_id) = root
                        .unpack::<(i64, String, Vec<u8>)>(kv.key())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    let bucket = u64::try_from(bucket).map_err(|_| {
                        custom_persist_error(PersistError::backend("negative gc bucket"))
                    })?;
                    if bucket > max_bucket {
                        break;
                    }
                    let world = WorldId::from_str(&world_id_str).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                    candidates.push((bucket, world, message_id, kv.key().to_vec()));
                    if candidates.len() >= limit as usize {
                        break;
                    }
                }
                Ok(candidates)
            }
        })?;
        let mut swept = 0u32;
        for (_bucket, world, message_id, gc_key) in candidates {
            let dedupe_key = self.portal_dedupe_key(universe, world, &message_id);
            let deleted = self.run(|trx, _| {
                let gc_key = gc_key.clone();
                let dedupe_key = dedupe_key.clone();
                async move {
                    let maybe = trx.get(&dedupe_key, false).await?;
                    let should_delete = match maybe {
                        Some(bytes) => {
                            let record: PortalDedupeRecord = serde_cbor::from_slice(bytes.as_ref())
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })?;
                            record
                                .gc_after_ns
                                .is_some_and(|gc_after_ns| gc_after_ns <= now_ns)
                        }
                        None => true,
                    };
                    if should_delete {
                        trx.clear(&dedupe_key);
                        trx.clear(&gc_key);
                    }
                    Ok(should_delete)
                }
            })?;
            if deleted {
                swept = swept.saturating_add(1);
            }
        }
        Ok(swept)
    }

    pub(super) fn portal_send(
        &self,
        universe: UniverseId,
        dest_world: WorldId,
        now_ns: u64,
        message_id: &[u8],
        item: InboxItem,
    ) -> Result<PortalSendResult, PersistError> {
        let item = self.normalize_inbox_item(universe, item)?;
        let value = self.encode(&item)?;
        let meta_key = self.world_meta_key(universe, dest_world);
        let dedupe_key = self.portal_dedupe_key(universe, dest_world, message_id);
        let notify_key = self.notify_counter_key(universe, dest_world);
        let inbox_space = self.inbox_entry_space(universe, dest_world);
        let dedupe_value = self.encode(&PortalDedupeRecord {
            enqueued_seq: None,
            completed_at_ns: Some(now_ns),
            gc_after_ns: Some(now_ns.saturating_add(self.config.dedupe_gc.portal_retention_ns)),
        })?;
        let portal_gc_key = self.portal_dedupe_gc_key(
            universe,
            gc_bucket_for(
                now_ns.saturating_add(self.config.dedupe_gc.portal_retention_ns),
                self.config.dedupe_gc.bucket_width_ns,
            ),
            dest_world,
            message_id,
        );

        loop {
            let trx = self.db.create_trx().map_err(map_fdb_error)?;
            let versionstamp = trx.get_versionstamp();
            let inbox_key = inbox_space.pack_with_versionstamp(&Versionstamp::incomplete(0));
            let op_result: Result<Option<PortalDedupeRecord>, TxRetryError> = block_on(async {
                if trx
                    .get(&meta_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                    .is_none()
                {
                    return Err(TxRetryError::Persist(PersistError::not_found(format!(
                        "world {dest_world} in universe {universe}"
                    ))));
                }
                if let Some(existing) = trx
                    .get(&dedupe_key, false)
                    .await
                    .map_err(TxRetryError::Fdb)?
                {
                    let record: PortalDedupeRecord = serde_cbor::from_slice(existing.as_ref())
                        .map_err(|err| {
                            TxRetryError::Persist(PersistError::backend(err.to_string()))
                        })?;
                    return Ok(Some(record));
                }
                let notify = match trx.get(&notify_key, false).await {
                    Ok(Some(bytes)) => decode_u64_static(bytes.as_ref())
                        .map_err(map_fdb_binding_error)
                        .map_err(TxRetryError::Persist)?,
                    Ok(None) => 0,
                    Err(err) => return Err(TxRetryError::Fdb(err)),
                };
                trx.atomic_op(&inbox_key, &value, MutationType::SetVersionstampedKey);
                trx.set(&notify_key, &(notify.saturating_add(1)).to_be_bytes());
                trx.set(&dedupe_key, &dedupe_value);
                trx.set(&portal_gc_key, &[]);
                self.mark_world_pending_inbox_in_tx(&trx, universe, dest_world, 0)
                    .await
                    .map_err(map_fdb_binding_error)
                    .map_err(TxRetryError::Persist)?;
                Ok(None)
            });

            match op_result {
                Ok(Some(record)) => {
                    return Ok(PortalSendResult {
                        status: PortalSendStatus::AlreadyEnqueued,
                        enqueued_seq: record.enqueued_seq,
                    });
                }
                Ok(None) => match block_on(trx.commit()) {
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
                        return Ok(PortalSendResult {
                            status: PortalSendStatus::Enqueued,
                            enqueued_seq: Some(InboxSeq::new(packed[prefix_len..].to_vec())),
                        });
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
}
