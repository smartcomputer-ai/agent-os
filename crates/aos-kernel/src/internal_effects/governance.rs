use aos_air_types::HashRef;
use aos_cbor::to_canonical_cbor;
use aos_effects::EffectIntent;

use crate::governance_effects::{
    GovApplyParams, GovApplyReceipt, GovApprovalDecision, GovApproveParams, GovApproveReceipt,
    GovDeltaKind, GovLedgerDelta, GovLedgerKind, GovModuleEffectAllowlist, GovPatchInput,
    GovPendingWorkflowReceipt, GovPredictedEffect, GovProposeParams, GovProposeReceipt,
    GovShadowParams, GovShadowReceipt, GovWorkflowInstancePreview,
};
use crate::{Kernel, KernelError};

fn hash_ref_from_hex(hex: &str) -> Result<HashRef, KernelError> {
    let value = format!("sha256:{hex}");
    HashRef::new(value).map_err(|e| KernelError::Manifest(format!("invalid hash: {e}")))
}

impl<S> Kernel<S>
where
    S: aos_store::Store + 'static,
{
    pub(super) fn handle_governance_propose(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: GovProposeParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.propose params: {e}")))?;
        let patch_hash = match &params.patch {
            GovPatchInput::Hash(hash) => hash.clone(),
            _ => {
                return Err(KernelError::Manifest(
                    "gov.propose params must use patch hash input after normalization".into(),
                ));
            }
        };
        let patch =
            crate::governance_effects::load_patch_by_hash(self.store().as_ref(), &patch_hash)?;
        let proposal_id = self.submit_proposal(patch, params.description.clone())?;
        let receipt = GovProposeReceipt {
            proposal_id,
            patch_hash,
            manifest_base: params.manifest_base.clone(),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_governance_shadow(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: GovShadowParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.shadow params: {e}")))?;
        let summary = self.run_shadow(params.proposal_id, None)?;
        let receipt = GovShadowReceipt {
            proposal_id: params.proposal_id,
            manifest_hash: HashRef::new(summary.manifest_hash)
                .map_err(|e| KernelError::Manifest(format!("invalid manifest hash: {e}")))?,
            predicted_effects: summary
                .predicted_effects
                .into_iter()
                .map(|effect| {
                    let intent_hash = hash_ref_from_hex(&effect.intent_hash)?;
                    let params_json = match effect.params_json {
                        Some(value) => Some(serde_json::to_string(&value).map_err(|err| {
                            KernelError::Manifest(format!("encode params_json: {err}"))
                        })?),
                        None => None,
                    };
                    Ok(GovPredictedEffect {
                        kind: effect.kind,
                        cap: effect.cap,
                        intent_hash,
                        params_json,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()?,
            pending_workflow_receipts: summary
                .pending_workflow_receipts
                .into_iter()
                .map(|pending| {
                    Ok(GovPendingWorkflowReceipt {
                        instance_id: pending.instance_id,
                        origin_module_id: pending.origin_module_id,
                        origin_instance_key_b64: pending.origin_instance_key_b64,
                        intent_hash: hash_ref_from_hex(&pending.intent_hash)?,
                        effect_kind: pending.effect_kind,
                        emitted_at_seq: pending.emitted_at_seq,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()?,
            workflow_instances: summary
                .workflow_instances
                .into_iter()
                .map(|instance| GovWorkflowInstancePreview {
                    instance_id: instance.instance_id,
                    status: instance.status,
                    last_processed_event_seq: instance.last_processed_event_seq,
                    module_version: instance.module_version,
                    inflight_intents: instance.inflight_intents as u64,
                })
                .collect(),
            module_effect_allowlists: summary
                .module_effect_allowlists
                .into_iter()
                .map(|allowlist| GovModuleEffectAllowlist {
                    module: allowlist.module,
                    effects_emitted: allowlist.effects_emitted,
                })
                .collect(),
            ledger_deltas: summary
                .ledger_deltas
                .into_iter()
                .map(|delta| GovLedgerDelta {
                    ledger: match delta.ledger {
                        crate::shadow::LedgerKind::Capability => GovLedgerKind::Capability,
                        crate::shadow::LedgerKind::Policy => GovLedgerKind::Policy,
                    },
                    name: delta.name,
                    change: match delta.change {
                        crate::shadow::DeltaKind::Added => GovDeltaKind::Added,
                        crate::shadow::DeltaKind::Removed => GovDeltaKind::Removed,
                        crate::shadow::DeltaKind::Changed => GovDeltaKind::Changed,
                    },
                })
                .collect(),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_governance_approve(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: GovApproveParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.approve params: {e}")))?;
        let proposal = self
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
        let patch_hash = HashRef::new(proposal.patch_hash.clone())
            .map_err(|e| KernelError::Manifest(format!("invalid patch hash: {e}")))?;
        match params.decision {
            GovApprovalDecision::Approve => {
                self.approve_proposal(params.proposal_id, params.approver.clone())?
            }
            GovApprovalDecision::Reject => {
                self.reject_proposal(params.proposal_id, params.approver.clone())?
            }
        }
        let receipt = GovApproveReceipt {
            proposal_id: params.proposal_id,
            decision: params.decision,
            patch_hash,
            approver: params.approver,
            reason: params.reason,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_governance_apply(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: GovApplyParams = intent
            .params()
            .map_err(|e| KernelError::Manifest(format!("decode gov.apply params: {e}")))?;
        let proposal = self
            .governance()
            .proposals()
            .get(&params.proposal_id)
            .ok_or(KernelError::ProposalNotFound(params.proposal_id))?;
        let patch_hash = HashRef::new(proposal.patch_hash.clone())
            .map_err(|e| KernelError::Manifest(format!("invalid patch hash: {e}")))?;
        self.apply_proposal(params.proposal_id)?;
        let manifest_hash_new = HashRef::new(self.manifest_hash().to_hex())
            .map_err(|e| KernelError::Manifest(format!("invalid manifest hash: {e}")))?;
        let receipt = GovApplyReceipt {
            proposal_id: params.proposal_id,
            manifest_hash_new,
            patch_hash,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }
}
