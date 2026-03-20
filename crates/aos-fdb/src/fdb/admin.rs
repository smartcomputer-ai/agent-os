use super::*;

impl UniverseStore for FdbWorldPersistence {
    fn create_universe(
        &self,
        request: CreateUniverseRequest,
    ) -> Result<UniverseCreateResult, PersistError> {
        let universe_id = request
            .universe_id
            .unwrap_or_else(|| UniverseId::from(Uuid::new_v4()));
        let handle = match request.handle {
            Some(handle) => normalize_handle(&handle)?,
            None => default_universe_handle(universe_id),
        };
        let record = UniverseRecord {
            universe_id,
            created_at_ns: request.created_at_ns,
            meta: aos_node::UniverseMeta {
                handle: handle.clone(),
            },
            admin: UniverseAdminLifecycle::default(),
        };
        let meta_key = self.universe_meta_key(universe_id);
        let handle_key = self.universe_handle_key(&handle);
        let meta_bytes = self.encode(&record)?;
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let handle_key = handle_key.clone();
            let meta_bytes = meta_bytes.clone();
            let handle = handle.clone();
            async move {
                if trx.get(&meta_key, false).await?.is_some() {
                    return Err(custom_persist_error(
                        PersistConflict::UniverseExists { universe_id }.into(),
                    ));
                }
                if let Some(existing) = trx.get(&handle_key, false).await? {
                    let existing_universe = UniverseId::from_str(
                        std::str::from_utf8(existing.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?,
                    )
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                    let existing_meta_key = self.universe_meta_key(existing_universe);
                    let existing_deleted = trx
                        .get(&existing_meta_key, false)
                        .await?
                        .map(|bytes| {
                            serde_cbor::from_slice::<UniverseRecord>(bytes.as_ref())
                                .map(|record| {
                                    matches!(record.admin.status, UniverseAdminStatus::Deleted)
                                })
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })
                        })
                        .transpose()?
                        .unwrap_or(false);
                    if !existing_deleted {
                        return Err(custom_persist_error(
                            PersistConflict::UniverseHandleExists {
                                handle,
                                universe_id: existing_universe,
                            }
                            .into(),
                        ));
                    }
                }
                trx.set(&meta_key, &meta_bytes);
                trx.set(&handle_key, universe_id.to_string().as_bytes());
                Ok(())
            }
        })?;
        Ok(UniverseCreateResult { record })
    }

    fn delete_universe(
        &self,
        universe: UniverseId,
        deleted_at_ns: u64,
    ) -> Result<UniverseRecord, PersistError> {
        let meta_key = self.universe_meta_key(universe);
        let world_catalog = self.world_catalog_space(universe);
        let (world_begin, world_end) = world_catalog.range();
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let world_begin = world_begin.clone();
            let world_end = world_end.clone();
            let world_catalog = world_catalog.clone();
            async move {
                let Some(bytes) = trx.get(&meta_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe {universe}"
                    ))));
                };
                let mut record: UniverseRecord = serde_cbor::from_slice(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                    return Ok(record);
                }

                let world_kvs = trx
                    .get_range(&RangeOption::from((world_begin, world_end)), 1, false)
                    .await?;
                for kv in world_kvs.iter() {
                    let meta: WorldMeta =
                        serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    if !matches!(meta.admin.status, aos_node::WorldAdminStatus::Deleted) {
                        let (world_id,): (String,) =
                            world_catalog.unpack(kv.key()).map_err(|err| {
                                custom_persist_error(PersistError::backend(err.to_string()))
                            })?;
                        let world_id = WorldId::from_str(&world_id).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                        return Err(custom_persist_error(
                            PersistConflict::UniverseDeleteBlockedByWorld {
                                universe_id: universe,
                                world_id,
                                status: meta.admin.status,
                            }
                            .into(),
                        ));
                    }
                }

                record.admin = UniverseAdminLifecycle {
                    status: UniverseAdminStatus::Deleted,
                    updated_at_ns: deleted_at_ns,
                };
                let bytes = serde_cbor::to_vec(&record)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                let handle_key = self.universe_handle_key(&record.meta.handle);
                trx.set(&meta_key, &bytes);
                trx.clear(&handle_key);
                Ok(record)
            }
        })
    }

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseRecord, PersistError> {
        let meta_key = self.universe_meta_key(universe);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            async move {
                let Some(bytes) = trx.get(&meta_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe {universe}"
                    ))));
                };
                serde_cbor::from_slice::<UniverseRecord>(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))
            }
        })
    }

    fn get_universe_by_handle(&self, handle: &str) -> Result<UniverseRecord, PersistError> {
        let handle = normalize_handle(handle)?;
        let handle_key = self.universe_handle_key(&handle);
        self.run(|trx, _| {
            let handle_key = handle_key.clone();
            let handle = handle.clone();
            async move {
                let Some(bytes) = trx.get(&handle_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe handle '{handle}'"
                    ))));
                };
                let universe =
                    UniverseId::from_str(std::str::from_utf8(bytes.as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                let meta_key = self.universe_meta_key(universe);
                let Some(record_bytes) = trx.get(&meta_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe {universe}"
                    ))));
                };
                let record = serde_cbor::from_slice::<UniverseRecord>(record_bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe handle '{handle}'"
                    ))));
                }
                Ok(record)
            }
        })
    }

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.universe_catalog_space();
        let (begin, end) = space.range();
        self.run(|trx, _| {
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
                let mut universes = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    let record: UniverseRecord = serde_cbor::from_slice(kv.value().as_ref())
                        .map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?;
                    universes.push(record);
                }
                universes.sort_by_key(|record| record.universe_id);
                Ok(universes)
            }
        })
    }

    fn set_universe_handle(
        &self,
        universe: UniverseId,
        handle: String,
    ) -> Result<UniverseRecord, PersistError> {
        let handle = normalize_handle(&handle)?;
        let meta_key = self.universe_meta_key(universe);
        let new_handle_key = self.universe_handle_key(&handle);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let new_handle_key = new_handle_key.clone();
            let handle = handle.clone();
            async move {
                let Some(bytes) = trx.get(&meta_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe {universe}"
                    ))));
                };
                let mut record: UniverseRecord = serde_cbor::from_slice(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                    return Err(custom_persist_error(
                        PersistConflict::UniverseAdminBlocked {
                            universe_id: universe,
                            status: record.admin.status,
                            action: "set_universe_handle".into(),
                        }
                        .into(),
                    ));
                }
                if record.meta.handle == handle {
                    return Ok(record);
                }
                if let Some(existing) = trx.get(&new_handle_key, false).await? {
                    let existing_universe = UniverseId::from_str(
                        std::str::from_utf8(existing.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })?,
                    )
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                    let existing_meta_key = self.universe_meta_key(existing_universe);
                    let existing_deleted = trx
                        .get(&existing_meta_key, false)
                        .await?
                        .map(|bytes| {
                            serde_cbor::from_slice::<UniverseRecord>(bytes.as_ref())
                                .map(|record| {
                                    matches!(record.admin.status, UniverseAdminStatus::Deleted)
                                })
                                .map_err(|err| {
                                    custom_persist_error(PersistError::backend(err.to_string()))
                                })
                        })
                        .transpose()?
                        .unwrap_or(false);
                    if existing_universe != universe && !existing_deleted {
                        return Err(custom_persist_error(
                            PersistConflict::UniverseHandleExists {
                                handle,
                                universe_id: existing_universe,
                            }
                            .into(),
                        ));
                    }
                }
                let previous_handle_key = self.universe_handle_key(&record.meta.handle);
                record.meta.handle = handle.clone();
                let updated = serde_cbor::to_vec(&record)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.clear(&previous_handle_key);
                trx.set(&new_handle_key, universe.to_string().as_bytes());
                trx.set(&meta_key, &updated);
                Ok(record)
            }
        })
    }
}

impl SecretStore for FdbWorldPersistence {
    fn put_secret_binding(
        &self,
        universe: UniverseId,
        record: SecretBindingRecord,
    ) -> Result<SecretBindingRecord, PersistError> {
        let universe_meta_key = self.universe_meta_key(universe);
        let binding_key = self.secret_binding_key(universe, &record.binding_id);
        let binding_bytes = self.encode(&record)?;
        let record_out = record.clone();
        self.run(|trx, _| {
            let universe_meta_key = universe_meta_key.clone();
            let binding_key = binding_key.clone();
            let binding_bytes = binding_bytes.clone();
            let record_out = record_out.clone();
            async move {
                if trx.get(&universe_meta_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "universe {universe}"
                    ))));
                }
                trx.set(&binding_key, &binding_bytes);
                Ok(record_out)
            }
        })
    }

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, PersistError> {
        let binding_key = self.secret_binding_key(universe, binding_id);
        self.run(|trx, _| {
            let binding_key = binding_key.clone();
            async move {
                trx.get(&binding_key, false)
                    .await?
                    .map(|bytes| {
                        serde_cbor::from_slice(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })
                    })
                    .transpose()
            }
        })
    }

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.secret_binding_root(universe);
        let (begin, end) = space.range();
        self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut out = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    out.push(serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?);
                }
                Ok(out)
            }
        })
    }

    fn disable_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        updated_at_ns: u64,
    ) -> Result<SecretBindingRecord, PersistError> {
        let binding_key = self.secret_binding_key(universe, binding_id);
        self.run(|trx, _| {
            let binding_key = binding_key.clone();
            async move {
                let Some(bytes) = trx.get(&binding_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "secret binding '{binding_id}'"
                    ))));
                };
                let mut record: SecretBindingRecord = serde_cbor::from_slice(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                record.status = aos_node::SecretBindingStatus::Disabled;
                record.updated_at_ns = updated_at_ns;
                let encoded = serde_cbor::to_vec(&record)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&binding_key, &encoded);
                Ok(record)
            }
        })
    }

    fn put_secret_version(
        &self,
        universe: UniverseId,
        request: PutSecretVersionRequest,
    ) -> Result<SecretVersionRecord, PersistError> {
        let binding_key = self.secret_binding_key(universe, &request.binding_id);
        self.run(|trx, _| {
            let binding_key = binding_key.clone();
            let request = request.clone();
            async move {
                let Some(binding_bytes) = trx.get(&binding_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "secret binding '{}'",
                        request.binding_id
                    ))));
                };
                let mut binding: SecretBindingRecord =
                    serde_cbor::from_slice(binding_bytes.as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?;
                let version = binding.latest_version.unwrap_or(0).saturating_add(1);
                let version_key = self
                    .secret_version_root(universe, &request.binding_id)
                    .pack(&(to_i64_static(version, "secret version")?,));
                let record = SecretVersionRecord {
                    binding_id: request.binding_id.clone(),
                    version,
                    digest: request.digest.clone(),
                    ciphertext: request.ciphertext.clone(),
                    dek_wrapped: request.dek_wrapped.clone(),
                    nonce: request.nonce.clone(),
                    enc_alg: request.enc_alg.clone(),
                    kek_id: request.kek_id.clone(),
                    created_at_ns: request.created_at_ns,
                    created_by: request.created_by.clone(),
                    status: aos_node::SecretVersionStatus::Active,
                };
                let record_bytes = serde_cbor::to_vec(&record)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&version_key, &record_bytes);
                binding.latest_version = Some(version);
                binding.updated_at_ns = request.created_at_ns;
                let binding_bytes = serde_cbor::to_vec(&binding)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.set(&binding_key, &binding_bytes);
                Ok(record)
            }
        })
    }

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, PersistError> {
        let version_key = self.secret_version_key(universe, binding_id, version)?;
        self.run(|trx, _| {
            let version_key = version_key.clone();
            async move {
                trx.get(&version_key, false)
                    .await?
                    .map(|bytes| {
                        serde_cbor::from_slice(bytes.as_ref()).map_err(|err| {
                            custom_persist_error(PersistError::backend(err.to_string()))
                        })
                    })
                    .transpose()
            }
        })
    }

    fn list_secret_versions(
        &self,
        universe: UniverseId,
        binding_id: &str,
        limit: u32,
    ) -> Result<Vec<SecretVersionRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let space = self.secret_version_root(universe, binding_id);
        let (begin, end) = space.range();
        self.run(|trx, _| {
            let begin = begin.clone();
            let end = end.clone();
            async move {
                let mut range = RangeOption::from((begin, end));
                range.limit = Some(limit as usize);
                let kvs = trx.get_range(&range, 1, false).await?;
                let mut out = Vec::with_capacity(kvs.len());
                for kv in kvs.iter() {
                    out.push(serde_cbor::from_slice(kv.value().as_ref()).map_err(|err| {
                        custom_persist_error(PersistError::backend(err.to_string()))
                    })?);
                }
                Ok(out)
            }
        })
    }

    fn append_secret_audit(
        &self,
        universe: UniverseId,
        record: SecretAuditRecord,
    ) -> Result<(), PersistError> {
        let key = self.secret_audit_key(universe, &record)?;
        let value = self.encode(&record)?;
        self.run(|trx, _| {
            let key = key.clone();
            let value = value.clone();
            async move {
                trx.set(&key, &value);
                Ok(())
            }
        })
    }
}

impl WorldAdminStore for FdbWorldPersistence {
    fn world_create_from_seed(
        &self,
        universe: UniverseId,
        request: CreateWorldSeedRequest,
    ) -> Result<WorldCreateResult, PersistError> {
        validate_create_world_seed_request(&request)?;
        let world_id = request
            .world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let handle = match request.handle {
            Some(handle) => normalize_handle(&handle)?,
            None => default_world_handle(world_id),
        };
        let record = self.create_world_from_seed_with_lineage(
            universe,
            world_id,
            &request.seed,
            handle,
            request.placement_pin,
            request.created_at_ns,
            Self::lineage_from_seed(request.created_at_ns, &request.seed),
        )?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, world_id, 0)
                .await?;
            Ok(())
        })?;
        Ok(WorldCreateResult { record })
    }

    fn world_prepare_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: WorldId,
        manifest_hash: Hash,
        handle: String,
        placement_pin: Option<String>,
        created_at_ns: u64,
        lineage: WorldLineage,
    ) -> Result<(), PersistError> {
        let universe_meta_key = self.universe_meta_key(universe);
        let universe_handle = default_universe_handle(universe);
        let universe_handle_key = self.universe_handle_key(&universe_handle);
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let handle = normalize_handle(&handle)?;
        let handle_key = self.world_handle_key(universe, &handle);
        let head_key = self.journal_head_key(universe, world);
        let ready_state_key = self.ready_state_key(universe, world);
        let manifest_meta_key = self.cas_meta_key(universe, manifest_hash);

        let mut meta = sample_world_meta(world);
        meta.handle = handle.clone();
        meta.created_at_ns = created_at_ns;
        meta.placement_pin = placement_pin;
        meta.lineage = Some(lineage);
        meta.manifest_hash = Some(manifest_hash.to_hex());

        let meta_bytes = self.encode(&meta)?;
        let universe_record_bytes = self.encode(&UniverseRecord {
            universe_id: universe,
            created_at_ns,
            meta: aos_node::UniverseMeta {
                handle: universe_handle.clone(),
            },
            admin: UniverseAdminLifecycle::default(),
        })?;
        let ready_state_bytes = self.encode(&ReadyState::default())?;
        self.run(|trx, _| {
            let universe_meta_key = universe_meta_key.clone();
            let universe_handle_key = universe_handle_key.clone();
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let handle_key = handle_key.clone();
            let head_key = head_key.clone();
            let ready_state_key = ready_state_key.clone();
            let manifest_meta_key = manifest_meta_key.clone();
            let meta_bytes = meta_bytes.clone();
            let universe_record_bytes = universe_record_bytes.clone();
            let ready_state_bytes = ready_state_bytes.clone();
            let handle = handle.clone();
            let universe_handle = universe_handle.clone();
            async move {
                if trx.get(&manifest_meta_key, false).await?.is_none() {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "manifest {} in universe {}",
                        manifest_hash.to_hex(),
                        universe
                    ))));
                }
                if trx.get(&meta_key, false).await?.is_some()
                    || trx.get(&head_key, false).await?.is_some()
                {
                    return Err(custom_persist_error(
                        PersistConflict::WorldExists { world_id: world }.into(),
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
                trx.set(&handle_key, world.to_string().as_bytes());
                trx.set(&head_key, &0u64.to_be_bytes());
                trx.set(&ready_state_key, &ready_state_bytes);
                Ok(())
            }
        })?;
        Ok(())
    }

    fn world_drop_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), PersistError> {
        let meta_key = self.world_meta_key(universe, world);
        let baseline_key = self.baseline_active_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let ready_high_key = self.ready_key(universe, 0, world);
        let ready_low_key = self.ready_key(universe, 1, world);
        let world_root = self.world_root(universe, world);
        let segment_root = self.segment_index_space(universe, world);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let baseline_key = baseline_key.clone();
            let catalog_key = catalog_key.clone();
            let ready_high_key = ready_high_key.clone();
            let ready_low_key = ready_low_key.clone();
            let world_root = world_root.clone();
            let segment_root = segment_root.clone();
            async move {
                let Some(meta_bytes) = trx.get(&meta_key, false).await? else {
                    return Ok(());
                };
                if trx.get(&baseline_key, false).await?.is_some() {
                    return Ok(());
                }
                let meta: WorldMeta = serde_cbor::from_slice(meta_bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                let handle_key = self.world_handle_key(universe, &meta.handle);
                let (world_begin, world_end) = world_root.range();
                trx.clear_range(&world_begin, &world_end);
                let (segment_begin, segment_end) = segment_root.range();
                trx.clear_range(&segment_begin, &segment_end);
                trx.clear(&catalog_key);
                trx.clear(&handle_key);
                trx.clear(&ready_high_key);
                trx.clear(&ready_low_key);
                Ok(())
            }
        })?;
        Ok(())
    }

    fn world_fork(
        &self,
        universe: UniverseId,
        request: ForkWorldRequest,
    ) -> Result<WorldForkResult, PersistError> {
        validate_fork_world_request(&request)?;
        let new_world_id = request
            .new_world_id
            .unwrap_or_else(|| WorldId::from(Uuid::new_v4()));
        let handle = match request.handle {
            Some(handle) => normalize_handle(&handle)?,
            None => default_world_handle(new_world_id),
        };
        let src_info =
            self.world_runtime_info(universe, request.src_world_id, request.forked_at_ns)?;
        let baseline =
            self.resolve_snapshot_selector(universe, request.src_world_id, &request.src_snapshot)?;
        let seed =
            self.seed_for_fork_policy(universe, &baseline, &request.pending_effect_policy)?;
        let record = self.create_world_from_seed_with_lineage(
            universe,
            new_world_id,
            &seed,
            handle,
            request
                .placement_pin
                .clone()
                .or(src_info.meta.placement_pin),
            request.forked_at_ns,
            WorldLineage::Fork {
                forked_at_ns: request.forked_at_ns,
                src_universe_id: universe,
                src_world_id: request.src_world_id,
                src_snapshot_ref: baseline.snapshot_ref,
                src_height: baseline.height,
            },
        )?;
        self.run(|trx, _| async move {
            self.refresh_world_ready_state_in_trx(&trx, universe, new_world_id, 0)
                .await?;
            Ok(())
        })?;
        Ok(WorldForkResult { record })
    }

    fn set_world_handle(
        &self,
        universe: UniverseId,
        world: WorldId,
        handle: String,
    ) -> Result<(), PersistError> {
        let handle = normalize_handle(&handle)?;
        let meta_key = self.world_meta_key(universe, world);
        let catalog_key = self.world_catalog_key(universe, world);
        let new_handle_key = self.world_handle_key(universe, &handle);
        self.run(|trx, _| {
            let meta_key = meta_key.clone();
            let catalog_key = catalog_key.clone();
            let new_handle_key = new_handle_key.clone();
            let handle = handle.clone();
            async move {
                let Some(bytes) = trx.get(&meta_key, false).await? else {
                    return Err(custom_persist_error(PersistError::not_found(format!(
                        "world {world} in universe {universe}"
                    ))));
                };
                let mut meta: WorldMeta = serde_cbor::from_slice(bytes.as_ref())
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                if meta.admin.status.blocks_world_operations() {
                    return Err(custom_persist_error(
                        PersistConflict::WorldAdminBlocked {
                            world_id: world,
                            status: meta.admin.status,
                            action: "set_world_handle".into(),
                        }
                        .into(),
                    ));
                }
                if meta.handle == handle {
                    return Ok(());
                }
                if let Some(existing) = trx.get(&new_handle_key, false).await? {
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
                    if existing_world != world && !existing_deleted {
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
                let previous_handle_key = self.world_handle_key(universe, &meta.handle);
                meta.handle = handle.clone();
                let encoded = serde_cbor::to_vec(&meta)
                    .map_err(|err| custom_persist_error(PersistError::backend(err.to_string())))?;
                trx.clear(&previous_handle_key);
                trx.set(&new_handle_key, world.to_string().as_bytes());
                trx.set(&meta_key, &encoded);
                trx.set(&catalog_key, &encoded);
                Ok(())
            }
        })
    }
}
