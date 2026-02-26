use super::*;

impl<S: Store + 'static> Kernel<S> {
    pub fn submit_proposal(
        &mut self,
        patch: ManifestPatch,
        description: Option<String>,
    ) -> Result<u64, KernelError> {
        let proposal_id = self.governance.alloc_proposal_id();

        let canonical_patch = canonicalize_patch(self.store.as_ref(), patch)?;

        for node in &canonical_patch.nodes {
            self.store.put_node(node)?;
        }
        self.store.put_node(&canonical_patch.manifest)?;

        let patch_bytes = to_canonical_cbor(&canonical_patch)
            .map_err(|err| KernelError::Manifest(format!("encode patch: {err}")))?;
        let patch_hash = self.store.put_blob(&patch_bytes)?;
        let record = GovernanceRecord::Proposed(ProposedRecord {
            proposal_id,
            description,
            patch_hash: patch_hash.to_hex(),
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(proposal_id)
    }

    pub fn run_shadow(
        &mut self,
        proposal_id: u64,
        harness: Option<ShadowHarness>,
    ) -> Result<ShadowSummary, KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        match proposal.state {
            ProposalState::Applied => return Err(KernelError::ProposalAlreadyApplied(proposal_id)),
            ProposalState::Submitted | ProposalState::Shadowed | ProposalState::Approved => {}
            ProposalState::Rejected => {
                return Err(KernelError::ProposalStateInvalid {
                    proposal_id,
                    state: proposal.state,
                    required: "not rejected",
                });
            }
        }
        let patch = self.load_manifest_patch(&proposal.patch_hash)?;
        let config = ShadowConfig {
            proposal_id,
            patch,
            patch_hash: proposal.patch_hash.clone(),
            harness,
        };
        let mut summary = ShadowExecutor::run(self.store.clone(), &config)?;
        summary.ledger_deltas = Self::compute_ledger_deltas(&self.manifest, &config.patch.manifest);
        let record = GovernanceRecord::ShadowReport(ShadowReportRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            manifest_hash: summary.manifest_hash.clone(),
            effects_predicted: summary.predicted_effects.clone(),
            pending_workflow_receipts: summary.pending_workflow_receipts.clone(),
            workflow_instances: summary.workflow_instances.clone(),
            module_effect_allowlists: summary.module_effect_allowlists.clone(),
            ledger_deltas: summary.ledger_deltas.clone(),
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(summary)
    }

    pub fn approve_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
    ) -> Result<(), KernelError> {
        self.decide_proposal(proposal_id, approver, ApprovalDecisionRecord::Approve)
    }

    pub fn reject_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
    ) -> Result<(), KernelError> {
        self.decide_proposal(proposal_id, approver, ApprovalDecisionRecord::Reject)
    }

    fn decide_proposal(
        &mut self,
        proposal_id: u64,
        approver: impl Into<String>,
        decision: ApprovalDecisionRecord,
    ) -> Result<(), KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        if matches!(proposal.state, ProposalState::Applied) {
            return Err(KernelError::ProposalAlreadyApplied(proposal_id));
        }
        if !matches!(
            proposal.state,
            ProposalState::Shadowed | ProposalState::Approved
        ) {
            return Err(KernelError::ProposalStateInvalid {
                proposal_id,
                state: proposal.state,
                required: "shadowed",
            });
        }
        let record = GovernanceRecord::Approved(ApprovedRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            approver: approver.into(),
            decision,
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(())
    }

    pub fn apply_proposal(&mut self, proposal_id: u64) -> Result<(), KernelError> {
        let proposal = self
            .governance
            .proposals()
            .get(&proposal_id)
            .ok_or(KernelError::ProposalNotFound(proposal_id))?
            .clone();
        if matches!(proposal.state, ProposalState::Applied) {
            return Err(KernelError::ProposalAlreadyApplied(proposal_id));
        }
        if !matches!(proposal.state, ProposalState::Approved) {
            return Err(KernelError::ProposalStateInvalid {
                proposal_id,
                state: proposal.state,
                required: "approved",
            });
        }
        let patch = self.load_manifest_patch(&proposal.patch_hash)?;
        self.swap_manifest(&patch)?;

        let manifest_bytes = to_canonical_cbor(&patch.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        let manifest_hash_new = Hash::of_bytes(&manifest_bytes).to_hex();

        let record = GovernanceRecord::Applied(AppliedRecord {
            proposal_id,
            patch_hash: proposal.patch_hash.clone(),
            manifest_hash_new,
        });
        self.append_record(JournalRecord::Governance(record.clone()))?;
        self.governance.apply_record(&record);
        Ok(())
    }

    /// Apply a manifest patch directly without governance (dev mode).
    ///
    /// The patch is canonicalized, stored in the CAS, and swapped into the live kernel.
    pub fn apply_patch_direct(&mut self, patch: ManifestPatch) -> Result<String, KernelError> {
        let canonical = canonicalize_patch(self.store.as_ref(), patch)?;

        for node in &canonical.nodes {
            self.store.put_node(node)?;
        }
        self.store.put_node(&canonical.manifest)?;
        self.store
            .put_node(&AirNode::Manifest(canonical.manifest.clone()))?;

        self.swap_manifest(&canonical)?;
        Ok(self.manifest_hash.to_hex())
    }

    fn load_manifest_patch(&self, hash_hex: &str) -> Result<ManifestPatch, KernelError> {
        let hash = Hash::from_hex_str(hash_hex)
            .map_err(|err| KernelError::Manifest(format!("invalid patch hash: {err}")))?;
        let bytes = self.store.get_blob(hash)?;
        let patch: ManifestPatch = serde_cbor::from_slice(&bytes)
            .map_err(|err| KernelError::Manifest(format!("decode patch: {err}")))?;
        Ok(patch)
    }

    fn swap_manifest(&mut self, patch: &ManifestPatch) -> Result<(), KernelError> {
        self.ensure_manifest_apply_quiescent()?;
        let loaded = patch.to_loaded_manifest(self.store.as_ref())?;
        self.apply_loaded_manifest(loaded, true)
    }

    fn ensure_manifest_apply_quiescent(&self) -> Result<(), KernelError> {
        let workflows_with_inflight = self
            .workflow_instances
            .values()
            .filter(|instance| !instance.inflight_intents.is_empty())
            .count();
        let inflight_workflow_intents = self
            .workflow_instances
            .values()
            .map(|instance| instance.inflight_intents.len())
            .sum::<usize>();
        let pending_reducer_receipts = self.pending_reducer_receipts.len();
        let queued_effects = self.effect_manager.queued().len();
        let scheduler_pending = !self.scheduler.is_empty();

        if workflows_with_inflight == 0
            && inflight_workflow_intents == 0
            && pending_reducer_receipts == 0
            && queued_effects == 0
            && !scheduler_pending
        {
            return Ok(());
        }

        Err(KernelError::ManifestApplyBlockedInFlight {
            plan_instances: workflows_with_inflight,
            waiting_events: inflight_workflow_intents,
            pending_plan_receipts: 0,
            pending_reducer_receipts,
            queued_effects,
            scheduler_pending,
        })
    }

    pub(super) fn apply_loaded_manifest(
        &mut self,
        loaded: LoadedManifest,
        record_manifest: bool,
    ) -> Result<(), KernelError> {
        let runtime = manifest_runtime::assemble_runtime(self.store.as_ref(), &loaded)?;

        self.manifest = loaded.manifest;
        let manifest_bytes = to_canonical_cbor(&self.manifest)
            .map_err(|err| KernelError::Manifest(format!("encode manifest: {err}")))?;
        self.manifest_hash = Hash::of_bytes(&manifest_bytes);
        self.secrets = loaded.secrets;
        self.module_defs = loaded.modules;
        self.plan_defs = HashMap::new();
        self.cap_defs = loaded.caps;
        self.effect_defs = loaded.effects;
        self.policy_defs = loaded.policies;
        self.schema_defs = loaded.schemas;
        self.plan_registry = runtime.plan_registry;
        self.router = runtime.router;
        self.plan_triggers = runtime.plan_triggers;

        self.plan_instances.clear();
        self.waiting_events.clear();
        self.pending_receipts.clear();
        self.pending_reducer_receipts.clear();
        self.workflow_instances.clear();
        self.recent_receipts.clear();
        self.recent_receipt_index.clear();
        self.plan_results.clear();

        self.schema_index = runtime.schema_index.clone();
        self.reducer_schemas = runtime.reducer_schemas;
        self.secret_resolver = ensure_secret_resolver(
            !self.secrets.is_empty(),
            self.secret_resolver.clone(),
            self.allow_placeholder_secrets,
        )?;
        let secret_catalog = if self.secrets.is_empty() {
            None
        } else {
            Some(crate::secret::SecretCatalog::new(&self.secrets))
        };
        let enforcer_invoker: Option<Arc<dyn CapEnforcerInvoker>> = Some(Arc::new(
            PureCapEnforcer::new(Arc::new(self.module_defs.clone()), self.pures.clone()),
        ));
        let param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>> = Some(Arc::new(
            GovernanceParamPreprocessor::new(self.store.clone(), self.manifest.clone()),
        ));
        self.effect_manager = EffectManager::new(
            runtime.capability_resolver,
            runtime.policy_gate,
            runtime.effect_catalog,
            runtime.schema_index.clone(),
            param_preprocessor,
            enforcer_invoker,
            secret_catalog,
            self.secret_resolver.clone(),
        );
        self.plan_cap_handles = runtime.plan_cap_handles;
        self.module_cap_bindings = runtime.module_cap_bindings;
        if record_manifest {
            self.record_manifest()?;
        }
        Ok(())
    }

    pub(super) fn compute_ledger_deltas(
        current: &Manifest,
        candidate: &Manifest,
    ) -> Vec<LedgerDelta> {
        let mut deltas = Vec::new();
        deltas.extend(
            crate::governance_utils::diff_named_refs(&current.caps, &candidate.caps)
                .into_iter()
                .map(|delta| LedgerDelta {
                    ledger: LedgerKind::Capability,
                    name: delta.name,
                    change: match delta.change {
                        crate::governance_utils::NamedRefDiffKind::Added => DeltaKind::Added,
                        crate::governance_utils::NamedRefDiffKind::Removed => DeltaKind::Removed,
                        crate::governance_utils::NamedRefDiffKind::Changed => DeltaKind::Changed,
                    },
                }),
        );
        deltas.extend(
            crate::governance_utils::diff_named_refs(&current.policies, &candidate.policies)
                .into_iter()
                .map(|delta| LedgerDelta {
                    ledger: LedgerKind::Policy,
                    name: delta.name,
                    change: match delta.change {
                        crate::governance_utils::NamedRefDiffKind::Added => DeltaKind::Added,
                        crate::governance_utils::NamedRefDiffKind::Removed => DeltaKind::Removed,
                        crate::governance_utils::NamedRefDiffKind::Changed => DeltaKind::Changed,
                    },
                }),
        );

        deltas.sort_by(|a, b| {
            let ledger_a = match a.ledger {
                LedgerKind::Capability => 0,
                LedgerKind::Policy => 1,
            };
            let ledger_b = match b.ledger {
                LedgerKind::Capability => 0,
                LedgerKind::Policy => 1,
            };
            (ledger_a, &a.name, format!("{:?}", a.change)).cmp(&(
                ledger_b,
                &b.name,
                format!("{:?}", b.change),
            ))
        });
        deltas
    }

    fn secret_resolver(&self) -> Option<SharedSecretResolver> {
        self.secret_resolver.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipts::ReducerEffectContext;
    use crate::world::test_support::{empty_manifest, hash, named_ref};
    use aos_effects::{EffectIntent, EffectKind};
    use aos_store::MemStore;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn ledger_deltas_capture_added_changed_and_removed() {
        let current = Manifest {
            caps: vec![
                named_ref("cap/a@1", &hash(1)),
                named_ref("cap/b@1", &hash(2)),
            ],
            policies: vec![named_ref("policy/old@1", &hash(3))],
            ..empty_manifest()
        };
        let candidate = Manifest {
            caps: vec![
                named_ref("cap/a@1", &hash(99)),
                named_ref("cap/c@1", &hash(4)),
            ],
            policies: vec![named_ref("policy/new@1", &hash(5))],
            ..empty_manifest()
        };

        let deltas = Kernel::<MemStore>::compute_ledger_deltas(&current, &candidate);

        assert_eq!(deltas.len(), 5);
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/a@1".to_string(),
            change: DeltaKind::Changed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/c@1".to_string(),
            change: DeltaKind::Added,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "cap/b@1".to_string(),
            change: DeltaKind::Removed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Policy,
            name: "policy/old@1".to_string(),
            change: DeltaKind::Removed,
        }));
        assert!(deltas.contains(&LedgerDelta {
            ledger: LedgerKind::Policy,
            name: "policy/new@1".to_string(),
            change: DeltaKind::Added,
        }));
    }

    fn empty_kernel() -> Kernel<MemStore> {
        let loaded = LoadedManifest {
            manifest: empty_manifest(),
            secrets: vec![],
            modules: HashMap::new(),
            effects: HashMap::new(),
            caps: HashMap::new(),
            policies: HashMap::new(),
            schemas: HashMap::new(),
            effect_catalog: aos_air_types::catalog::EffectCatalog::new(),
        };
        Kernel::from_loaded_manifest(
            Arc::new(MemStore::new()),
            loaded,
            Box::new(crate::journal::mem::MemJournal::new()),
        )
        .expect("kernel")
    }

    #[test]
    fn apply_patch_direct_blocks_when_runtime_not_quiescent() {
        let mut kernel = empty_kernel();
        let mut inflight = std::collections::BTreeMap::new();
        inflight.insert(
            [1u8; 32],
            WorkflowInflightIntentMeta {
                origin_module_id: "com.acme/Workflow@1".into(),
                origin_instance_key: None,
                effect_kind: "sys/http.request@1".into(),
                params_hash: None,
                emitted_at_seq: 0,
            },
        );
        kernel.workflow_instances.insert(
            "com.acme/Workflow@1::".into(),
            WorkflowInstanceState {
                state_bytes: vec![1],
                inflight_intents: inflight,
                status: WorkflowRuntimeStatus::Waiting,
                last_processed_event_seq: 0,
                module_version: None,
            },
        );
        kernel.pending_reducer_receipts.insert(
            [2u8; 32],
            ReducerEffectContext::new(
                "com.acme/Reducer@1".into(),
                None,
                "timer.set".into(),
                vec![],
                [2u8; 32],
                0,
                None,
            ),
        );
        kernel.effect_manager.restore_queue(vec![EffectIntent {
            kind: EffectKind::new("introspect.manifest"),
            cap_name: "sys/query@1".into(),
            params_cbor: vec![],
            idempotency_key: [0u8; 32],
            intent_hash: [3u8; 32],
        }]);

        let patch = ManifestPatch {
            manifest: empty_manifest(),
            nodes: vec![],
        };
        let err = kernel
            .apply_patch_direct(patch)
            .expect_err("manifest apply should block while in-flight state exists");
        match err {
            KernelError::ManifestApplyBlockedInFlight {
                plan_instances,
                waiting_events,
                pending_plan_receipts,
                pending_reducer_receipts,
                queued_effects,
                scheduler_pending,
            } => {
                assert_eq!(plan_instances, 1);
                assert_eq!(waiting_events, 1);
                assert_eq!(pending_plan_receipts, 0);
                assert_eq!(pending_reducer_receipts, 1);
                assert_eq!(queued_effects, 1);
                assert!(!scheduler_pending);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn apply_patch_direct_succeeds_when_runtime_quiescent() {
        let mut kernel = empty_kernel();
        let patch = ManifestPatch {
            manifest: empty_manifest(),
            nodes: vec![],
        };
        let result = kernel.apply_patch_direct(patch);
        assert!(
            result.is_ok(),
            "expected quiescent apply to succeed: {result:?}"
        );
    }
}
