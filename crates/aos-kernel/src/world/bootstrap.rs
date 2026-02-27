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
            cap_defs,
            effect_defs,
            policy_defs,
            schema_defs,
            schema_index: runtime.schema_index.clone(),
            workflow_schemas: runtime.workflow_schemas.clone(),
            module_cap_bindings: runtime.module_cap_bindings,
            workflows: WorkflowRegistry::new(store.clone(), config.module_cache_dir.clone())?,
            pures,
            router: runtime.router,
            pending_workflow_receipts: HashMap::new(),
            recent_receipts: VecDeque::new(),
            recent_receipt_index: HashSet::new(),
            workflow_queue: VecDeque::new(),
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
            workflow_state: HashMap::new(),
            workflow_instances: HashMap::new(),
            workflow_index_roots: HashMap::new(),
            snapshot_index: HashMap::new(),
            journal,
            suppress_journal: false,
            replay_applying_domain_record: false,
            replay_generated_domain_event_hashes: HashMap::new(),
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
        let journal_empty = kernel.journal.next_seq() == 0;
        kernel.replay_existing_entries()?;
        if journal_empty {
            kernel.record_manifest()?;
        }
        kernel.ensure_active_baseline()?;
        Ok(kernel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::mem::MemJournal;
    use crate::world::test_support::empty_manifest;
    use aos_air_types::{SecretDecl, SecretEntry, catalog::EffectCatalog};
    use aos_store::MemStore;
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
            policy: None,
        }));
        let loaded = LoadedManifest {
            manifest,
            secrets: vec![SecretDecl {
                alias: "payments/stripe".into(),
                version: 1,
                binding_id: "stripe:prod".into(),
                expected_digest: None,
                policy: None,
            }],
            modules: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: EffectCatalog::new(),
        };

        let result = Kernel::from_loaded_manifest(store, loaded, Box::new(MemJournal::new()));

        assert!(matches!(result, Err(KernelError::SecretResolverMissing)));
    }
}
