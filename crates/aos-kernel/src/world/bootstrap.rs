use super::*;

impl<S: Store + 'static> Kernel<S> {
    pub fn from_loaded_manifest(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Box<dyn Journal>,
    ) -> Result<Self, KernelError> {
        Self::from_loaded_manifest_with_config(store, loaded, journal, KernelConfig::default())
    }

    pub fn from_loaded_manifest_with_config(
        store: Arc<S>,
        loaded: LoadedManifest,
        journal: Box<dyn Journal>,
        config: KernelConfig,
    ) -> Result<Self, KernelError> {
        let mut loaded = loaded;
        let secret_resolver = select_secret_resolver(!loaded.secrets.is_empty(), &config)?;
        let runtime = manifest_runtime::assemble_runtime(store.as_ref(), &loaded)?;
        let plan_defs = loaded.plans.clone();
        let cap_defs = loaded.caps.clone();
        let effect_defs = loaded.effects.clone();
        let policy_defs = loaded.policies.clone();
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
        let enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>> = Some(Arc::new(
            PureCapEnforcer::new(Arc::new(loaded.modules.clone()), pures.clone()),
        ));

        let param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>> = Some(Arc::new(
            GovernanceParamPreprocessor::new(store.clone(), loaded.manifest.clone()),
        ));
        let mut kernel = Self {
            store: store.clone(),
            manifest: loaded.manifest.clone(),
            manifest_hash,
            module_defs: loaded.modules,
            plan_defs,
            cap_defs,
            effect_defs,
            policy_defs,
            schema_defs,
            schema_index: runtime.schema_index.clone(),
            reducer_schemas: runtime.reducer_schemas.clone(),
            plan_cap_handles: runtime.plan_cap_handles,
            module_cap_bindings: runtime.module_cap_bindings,
            reducers: ReducerRegistry::new(store.clone(), config.module_cache_dir.clone())?,
            pures,
            router: runtime.router,
            plan_registry: runtime.plan_registry,
            plan_instances: HashMap::new(),
            plan_triggers: runtime.plan_triggers,
            waiting_events: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_reducer_receipts: HashMap::new(),
            recent_receipts: VecDeque::new(),
            recent_receipt_index: HashSet::new(),
            plan_results: VecDeque::new(),
            scheduler: Scheduler::default(),
            effect_manager: EffectManager::new(
                runtime.capability_resolver,
                runtime.policy_gate,
                runtime.effect_catalog.clone(),
                runtime.schema_index.clone(),
                param_preprocessor,
                enforcer_invoker,
                if loaded.secrets.is_empty() {
                    None
                } else {
                    Some(crate::secret::SecretCatalog::new(&loaded.secrets))
                },
                secret_resolver.clone(),
            ),
            clock: KernelClock::new(),
            reducer_state: HashMap::new(),
            reducer_index_roots: HashMap::new(),
            snapshot_index: HashMap::new(),
            journal,
            suppress_journal: false,
            governance: GovernanceManager::new(),
            secret_resolver: secret_resolver.clone(),
            allow_placeholder_secrets: config.allow_placeholder_secrets,
            secrets: loaded.secrets,
            active_baseline: None,
            last_snapshot_height: None,
            last_snapshot_hash: None,
            pinned_roots: Vec::new(),
            workspace_roots: Vec::new(),
        };
        if config.eager_module_load {
            for (name, module_def) in kernel.module_defs.iter() {
                match module_def.module_kind {
                    aos_air_types::ModuleKind::Reducer => {
                        kernel.reducers.ensure_loaded(name, module_def)?;
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
        let journal_empty = kernel.journal.next_seq() == 0;
        kernel.replay_existing_entries()?;
        if journal_empty {
            kernel.record_manifest()?;
        }
        kernel.ensure_active_baseline()?;
        Ok(kernel)
    }
}
