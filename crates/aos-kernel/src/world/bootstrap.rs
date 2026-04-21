use super::*;

impl<S: Store + 'static> Kernel<S> {
    fn build_from_loaded_manifest_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Journal,
        config: KernelConfig,
    ) -> Result<Self, KernelError> {
        let mut loaded = loaded;
        let secret_resolver = select_secret_resolver(!loaded.secrets.is_empty(), &config)?;
        let runtime = manifest_runtime::assemble_runtime(store.as_ref(), &loaded)?;
        let effect_defs = loaded.effects.clone();
        let schema_defs = loaded.schemas.clone();

        // Persist the loaded manifest + defs into the store so governance/patch doc
        // compilation can resolve the base manifest hash from CAS.
        manifest_runtime::persist_loaded_manifest(store.as_ref(), &mut loaded)?;

        let manifest_bytes = to_canonical_cbor(&loaded.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        let manifest_hash = Hash::of_bytes(&manifest_bytes);

        let pures = Arc::new(Mutex::new(PureRegistry::new(
            store.clone(),
            config.module_cache_dir.clone(),
        )?));

        let param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>> = Some(Arc::new(
            GovernanceParamPreprocessor::new(store.clone(), loaded.manifest.clone()),
        ));
        let mut kernel = Self {
            store: store.clone(),
            manifest: loaded.manifest.clone(),
            manifest_hash,
            module_defs: loaded.modules,
            effect_defs,
            schema_defs,
            schema_index: runtime.schema_index.clone(),
            workflow_schemas: runtime.workflow_schemas.clone(),
            workflows: WorkflowRegistry::new(store.clone(), config.module_cache_dir.clone())?,
            pures,
            router: runtime.router,
            pending_workflow_receipts: HashMap::new(),
            recent_receipts: VecDeque::new(),
            recent_receipt_index: HashSet::new(),
            workflow_queue: VecDeque::new(),
            effect_manager: EffectManager::new(
                runtime.effect_catalog.clone(),
                runtime.schema_index.clone(),
                param_preprocessor,
                if loaded.secrets.is_empty() {
                    None
                } else {
                    Some(crate::secret::SecretCatalog::new(&loaded.secrets))
                },
                secret_resolver.clone(),
            ),
            clock: KernelClock::new(),
            workflow_state: HashMap::new(),
            workflow_instances: HashMap::new(),
            workflow_index_roots: HashMap::new(),
            snapshot_index: HashMap::new(),
            journal,
            compat_drain_cursor: 0,
            suppress_journal: false,
            replay_applying_domain_record: false,
            replay_generated_domain_event_hashes: HashMap::new(),
            governance: GovernanceManager::new(),
            secret_resolver: secret_resolver.clone(),
            allow_placeholder_secrets: config.allow_placeholder_secrets,
            cell_cache_size: config.cell_cache_size.max(1),
            universe_id: config.universe_id,
            secrets: loaded.secrets,
            active_baseline: None,
            last_snapshot_height: None,
            last_snapshot_hash: None,
            pinned_roots: Vec::new(),
            workspace_roots: Vec::new(),
            pending_cell_projection_deltas: BTreeMap::new(),
            replay_metrics: None,
            cell_delta_access_tick: 0,
            cell_spill_put_blob_count: 0,
            cell_snapshot_put_blob_count: 0,
        };
        if config.eager_module_load {
            for (name, module_def) in kernel.module_defs.iter() {
                match module_def.module_kind {
                    aos_air_types::ModuleKind::Workflow => {
                        kernel.workflows.ensure_loaded(name, module_def)?;
                    }
                    aos_air_types::ModuleKind::Pure => {
                        let mut pures = kernel.pures.lock().map_err(|_| {
                            KernelError::Manifest("pure registry lock poisoned".into())
                        })?;
                        pures.ensure_loaded(name, module_def)?;
                    }
                }
            }
        }
        Ok(kernel)
    }

    pub fn from_loaded_manifest(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Journal,
    ) -> Result<Self, KernelError> {
        Self::from_loaded_manifest_with_config(store, loaded, journal, KernelConfig::default())
    }

    pub fn from_loaded_manifest_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Journal,
        config: KernelConfig,
    ) -> Result<Self, KernelError> {
        let mut kernel =
            Self::build_from_loaded_manifest_with_config(store, loaded, journal, config)?;
        let journal_empty = kernel.journal.next_seq() == 0;
        kernel.replay_existing_entries()?;
        if journal_empty {
            kernel.record_manifest()?;
        }
        kernel.ensure_active_baseline()?;
        Ok(kernel)
    }

    pub fn from_loaded_manifest_without_replay_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Journal,
        config: KernelConfig,
    ) -> Result<Self, KernelError> {
        Self::build_from_loaded_manifest_with_config(store, loaded, journal, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;
    use crate::journal::Journal;
    use crate::world::test_support::empty_manifest;
    use aos_air_types::{SecretDecl, SecretEntry, catalog::EffectCatalog};
    use std::collections::HashMap;

    #[test]
    fn kernel_requires_secret_resolver_for_secretful_manifest() {
        let store = Arc::new(MemStore::new());
        let mut manifest = empty_manifest();
        manifest.secrets.push(SecretEntry::Decl(SecretDecl {
            alias: "payments/stripe".into(),
            version: 1,
            binding_id: "stripe:prod".into(),
            expected_digest: None,
        }));
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
            }],
            modules: HashMap::new(),
            effects: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::new(),
        };

        let result = Kernel::from_loaded_manifest(store, loaded, Journal::new());

        assert!(matches!(result, Err(KernelError::SecretResolverMissing)));
    }
}
