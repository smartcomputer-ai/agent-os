use super::*;

impl UniverseStore for MemoryWorldPersistence {
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
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        if guard.universes.contains_key(&universe_id) {
            return Err(PersistConflict::UniverseExists { universe_id }.into());
        }
        Self::ensure_universe_handle_available(&guard, universe_id, &handle)?;
        let record = UniverseRecord {
            universe_id,
            created_at_ns: request.created_at_ns,
            meta: UniverseMeta {
                handle: handle.clone(),
            },
            admin: UniverseAdminLifecycle::default(),
        };
        guard.universes.insert(universe_id, record.clone());
        guard.universe_handles.insert(handle, universe_id);
        Ok(UniverseCreateResult { record })
    }

    fn delete_universe(
        &self,
        universe: UniverseId,
        deleted_at_ns: u64,
    ) -> Result<UniverseRecord, PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let Some(record) = guard.universes.get(&universe).cloned() else {
            return Err(PersistError::not_found(format!("universe {universe}")));
        };
        if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
            return Ok(record);
        }
        for ((world_universe, world_id), world_state) in &guard.worlds {
            if *world_universe == universe
                && !matches!(world_state.meta.admin.status, WorldAdminStatus::Deleted)
            {
                return Err(PersistConflict::UniverseDeleteBlockedByWorld {
                    universe_id: universe,
                    world_id: *world_id,
                    status: world_state.meta.admin.status,
                }
                .into());
            }
        }
        let (handle, updated) = {
            let record = guard
                .universes
                .get_mut(&universe)
                .expect("universe record still present");
            record.admin = UniverseAdminLifecycle {
                status: UniverseAdminStatus::Deleted,
                updated_at_ns: deleted_at_ns,
            };
            (record.meta.handle.clone(), record.clone())
        };
        guard.universe_handles.remove(&handle);
        Ok(updated)
    }

    fn get_universe(&self, universe: UniverseId) -> Result<UniverseRecord, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        guard
            .universes
            .get(&universe)
            .cloned()
            .ok_or_else(|| PersistError::not_found(format!("universe {universe}")))
    }

    fn get_universe_by_handle(&self, handle: &str) -> Result<UniverseRecord, PersistError> {
        let handle = normalize_handle(handle)?;
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let universe = guard
            .universe_handles
            .get(&handle)
            .ok_or_else(|| PersistError::not_found(format!("universe handle '{handle}'")))?;
        guard
            .universes
            .get(universe)
            .filter(|record| !matches!(record.admin.status, UniverseAdminStatus::Deleted))
            .cloned()
            .ok_or_else(|| PersistError::not_found(format!("universe {universe}")))
    }

    fn list_universes(
        &self,
        after: Option<UniverseId>,
        limit: u32,
    ) -> Result<Vec<UniverseRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let records = match after {
            Some(cursor) => guard
                .universes
                .range((
                    std::ops::Bound::Excluded(cursor),
                    std::ops::Bound::Unbounded,
                ))
                .map(|(_, record)| record)
                .take(limit as usize)
                .cloned()
                .collect(),
            None => guard
                .universes
                .values()
                .take(limit as usize)
                .cloned()
                .collect(),
        };
        Ok(records)
    }

    fn set_universe_handle(
        &self,
        universe: UniverseId,
        handle: String,
    ) -> Result<UniverseRecord, PersistError> {
        let handle = normalize_handle(&handle)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_universe_handle_available(&guard, universe, &handle)?;
        let (previous, updated) = {
            let record = guard
                .universes
                .get_mut(&universe)
                .ok_or_else(|| PersistError::not_found(format!("universe {universe}")))?;
            if matches!(record.admin.status, UniverseAdminStatus::Deleted) {
                return Err(PersistConflict::UniverseAdminBlocked {
                    universe_id: universe,
                    status: record.admin.status,
                    action: "set_universe_handle".into(),
                }
                .into());
            }
            if record.meta.handle == handle {
                return Ok(record.clone());
            }
            let previous = record.meta.handle.clone();
            record.meta.handle = handle.clone();
            (previous, record.clone())
        };
        guard.universe_handles.remove(&previous);
        guard.universe_handles.insert(handle, universe);
        Ok(updated)
    }
}

impl SecretStore for MemoryWorldPersistence {
    fn put_secret_binding(
        &self,
        universe: UniverseId,
        mut record: SecretBindingRecord,
    ) -> Result<SecretBindingRecord, PersistError> {
        Self::validate_secret_binding(&record)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_secret_universe_record(&mut guard, universe)?;
        let existing = guard
            .secret_bindings
            .entry(universe)
            .or_default()
            .get(&record.binding_id)
            .cloned();
        if let Some(existing) = existing {
            record.created_at_ns = existing.created_at_ns;
            if record.latest_version.is_none() {
                record.latest_version = existing.latest_version;
            }
        }
        if record.updated_at_ns == 0 {
            record.updated_at_ns = record.created_at_ns;
        }
        guard
            .secret_bindings
            .entry(universe)
            .or_default()
            .insert(record.binding_id.clone(), record.clone());
        Ok(record)
    }

    fn get_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
    ) -> Result<Option<SecretBindingRecord>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Ok(guard
            .secret_bindings
            .get(&universe)
            .and_then(|bindings| bindings.get(binding_id))
            .cloned())
    }

    fn list_secret_bindings(
        &self,
        universe: UniverseId,
        limit: u32,
    ) -> Result<Vec<SecretBindingRecord>, PersistError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Ok(guard
            .secret_bindings
            .get(&universe)
            .into_iter()
            .flat_map(|bindings| bindings.values())
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn disable_secret_binding(
        &self,
        universe: UniverseId,
        binding_id: &str,
        updated_at_ns: u64,
    ) -> Result<SecretBindingRecord, PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let record = guard
            .secret_bindings
            .entry(universe)
            .or_default()
            .get_mut(binding_id)
            .ok_or_else(|| PersistError::not_found(format!("secret binding '{binding_id}'")))?;
        record.status = SecretBindingStatus::Disabled;
        record.updated_at_ns = updated_at_ns;
        Ok(record.clone())
    }

    fn put_secret_version(
        &self,
        universe: UniverseId,
        request: PutSecretVersionRequest,
    ) -> Result<SecretVersionRecord, PersistError> {
        if request.binding_id.trim().is_empty() {
            return Err(PersistError::validation("binding_id must be non-empty"));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_secret_universe_record(&mut guard, universe)?;
        let previous_version = {
            let binding = guard
                .secret_bindings
                .entry(universe)
                .or_default()
                .get_mut(&request.binding_id)
                .ok_or_else(|| {
                    PersistError::not_found(format!("secret binding '{}'", request.binding_id))
                })?;
            if !matches!(binding.status, SecretBindingStatus::Active) {
                return Err(PersistError::validation(format!(
                    "secret binding '{}' is disabled",
                    request.binding_id
                )));
            }
            if !matches!(
                binding.source_kind,
                SecretBindingSourceKind::NodeSecretStore
            ) {
                return Err(PersistError::validation(format!(
                    "secret binding '{}' is not node_secret_store",
                    request.binding_id
                )));
            }
            binding.latest_version
        };
        if let Some(previous_version) = previous_version {
            if let Some(previous) = guard
                .secret_versions
                .entry(universe)
                .or_default()
                .get_mut(&(request.binding_id.clone(), previous_version))
            {
                previous.status = SecretVersionStatus::Superseded;
            }
        }
        let version = previous_version.unwrap_or(0).saturating_add(1);
        let record = SecretVersionRecord {
            binding_id: request.binding_id.clone(),
            version,
            digest: request.digest,
            ciphertext: request.ciphertext,
            dek_wrapped: request.dek_wrapped,
            nonce: request.nonce,
            enc_alg: request.enc_alg,
            kek_id: request.kek_id,
            created_at_ns: request.created_at_ns,
            created_by: request.created_by,
            status: SecretVersionStatus::Active,
        };
        guard
            .secret_versions
            .entry(universe)
            .or_default()
            .insert((record.binding_id.clone(), version), record.clone());
        let binding = guard
            .secret_bindings
            .entry(universe)
            .or_default()
            .get_mut(&request.binding_id)
            .expect("binding exists while updating latest_version");
        binding.latest_version = Some(version);
        binding.updated_at_ns = record.created_at_ns;
        Ok(record)
    }

    fn get_secret_version(
        &self,
        universe: UniverseId,
        binding_id: &str,
        version: u64,
    ) -> Result<Option<SecretVersionRecord>, PersistError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Ok(guard
            .secret_versions
            .get(&universe)
            .and_then(|versions| versions.get(&(binding_id.to_string(), version)))
            .cloned())
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
        let guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let mut records: Vec<_> = guard
            .secret_versions
            .get(&universe)
            .into_iter()
            .flat_map(|versions| versions.values())
            .filter(|record| record.binding_id == binding_id)
            .cloned()
            .collect();
        records.sort_by_key(|record| record.version);
        records.truncate(limit as usize);
        Ok(records)
    }

    fn append_secret_audit(
        &self,
        universe: UniverseId,
        record: SecretAuditRecord,
    ) -> Result<(), PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        guard.secret_audit.entry(universe).or_default().insert(
            (
                record.ts_ns,
                record.binding_id.clone(),
                record.version.unwrap_or(0),
            ),
            record,
        );
        Ok(())
    }
}

impl WorldAdminStore for MemoryWorldPersistence {
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
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_universe_record(&mut guard, universe, request.created_at_ns)?;
        let record = Self::create_world_state_from_seed(
            &mut guard,
            &self.cas,
            self.config,
            universe,
            world_id,
            &request.seed,
            handle,
            request.placement_pin,
            request.created_at_ns,
            Self::lineage_from_seed(request.created_at_ns, &request.seed),
        )?;
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
        if !self.cas.has(universe, manifest_hash)? {
            return Err(PersistError::not_found(format!(
                "manifest {} in universe {}",
                manifest_hash.to_hex(),
                universe
            )));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_universe_record(&mut guard, universe, created_at_ns)?;
        if guard.worlds.contains_key(&(universe, world)) {
            return Err(PersistConflict::WorldExists { world_id: world }.into());
        }
        let handle = normalize_handle(&handle)?;
        Self::ensure_world_handle_available(&guard, universe, world, &handle)?;
        let mut world_state = WorldState::default();
        world_state.meta.handle = handle.clone();
        world_state.meta.created_at_ns = created_at_ns;
        world_state.meta.placement_pin = placement_pin;
        world_state.meta.lineage = Some(lineage);
        world_state.meta.manifest_hash = Some(manifest_hash.to_hex());
        world_state.ready_state = Self::recompute_ready_state(&world_state, self.config);
        guard.worlds.insert((universe, world), world_state);
        guard
            .world_handles
            .entry(universe)
            .or_default()
            .insert(handle, world);
        Self::sync_ready_state(&mut guard, universe, world, 0, self.config);
        Ok(())
    }

    fn world_drop_manifest_bootstrap(
        &self,
        universe: UniverseId,
        world: WorldId,
    ) -> Result<(), PersistError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        let can_drop = match guard.worlds.get(&(universe, world)) {
            Some(world_state) => {
                world_state.active_baseline.is_none() && world_state.snapshots.is_empty()
            }
            None => return Ok(()),
        };
        if !can_drop {
            return Ok(());
        }
        let handle = guard
            .worlds
            .get(&(universe, world))
            .map(|world_state| world_state.meta.handle.clone());
        if let Some(handle) = handle {
            if let Some(handles) = guard.world_handles.get_mut(&universe) {
                handles.remove(&handle);
            }
        }
        guard.worlds.remove(&(universe, world));
        guard
            .ready_hints
            .retain(|(_, _, hint_universe, hint_world), _| {
                *hint_universe != universe || *hint_world != world
            });
        guard
            .lease_by_worker
            .retain(|(_, lease_universe, lease_world), _| {
                *lease_universe != universe || *lease_world != world
            });
        guard.portal_dedupe.remove(&(universe, world));
        if let Some(gc) = guard.portal_dedupe_gc.get_mut(&universe) {
            gc.retain(|(_, gc_world, _), _| *gc_world != world);
        }
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
        let (baseline, inherited_pin) = {
            let guard = self
                .state
                .lock()
                .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
            let src_world = guard
                .worlds
                .get(&(universe, request.src_world_id))
                .ok_or_else(|| {
                    PersistError::not_found(format!(
                        "world {} in universe {}",
                        request.src_world_id, universe
                    ))
                })?;
            let baseline =
                Self::resolve_snapshot_selector_from_state(src_world, &request.src_snapshot)?;
            (baseline, src_world.meta.placement_pin.clone())
        };
        let seed =
            self.seed_for_fork_policy(universe, &baseline, &request.pending_effect_policy)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_universe_record(&mut guard, universe, request.forked_at_ns)?;
        let record = Self::create_world_state_from_seed(
            &mut guard,
            &self.cas,
            self.config,
            universe,
            new_world_id,
            &seed,
            handle,
            request.placement_pin.clone().or(inherited_pin),
            request.forked_at_ns,
            WorldLineage::Fork {
                forked_at_ns: request.forked_at_ns,
                src_universe_id: universe,
                src_world_id: request.src_world_id,
                src_snapshot_ref: baseline.snapshot_ref,
                src_height: baseline.height,
            },
        )?;
        Ok(WorldForkResult { record })
    }

    fn set_world_handle(
        &self,
        universe: UniverseId,
        world: WorldId,
        handle: String,
    ) -> Result<(), PersistError> {
        let handle = normalize_handle(&handle)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| PersistError::backend("memory persistence mutex poisoned"))?;
        Self::ensure_world_handle_available(&guard, universe, world, &handle)?;
        let previous = {
            let world_state = guard.worlds.get_mut(&(universe, world)).ok_or_else(|| {
                PersistError::not_found(format!("world {world} in universe {universe}"))
            })?;
            if world_state.meta.admin.status.blocks_world_operations() {
                return Err(PersistConflict::WorldAdminBlocked {
                    world_id: world,
                    status: world_state.meta.admin.status,
                    action: "set_world_handle".into(),
                }
                .into());
            }
            if world_state.meta.handle == handle {
                return Ok(());
            }
            let previous = world_state.meta.handle.clone();
            world_state.meta.handle = handle.clone();
            previous
        };
        let handles = guard.world_handles.entry(universe).or_default();
        handles.remove(&previous);
        handles.insert(handle, world);
        Ok(())
    }
}
