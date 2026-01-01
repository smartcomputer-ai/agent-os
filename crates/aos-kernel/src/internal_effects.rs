use aos_air_types::HashRef;
use aos_cbor::to_canonical_cbor;
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use serde::{Deserialize, Serialize};

use crate::cell_index::CellMeta;
use crate::governance_effects::{
    GovApplyParams, GovApplyReceipt, GovApprovalDecision, GovApproveParams, GovApproveReceipt,
    GovPatchInput, GovProposeParams, GovProposeReceipt, GovShadowParams, GovShadowReceipt,
    GovPredictedEffect, GovPendingReceipt, GovPlanResultPreview, GovLedgerDelta, GovLedgerKind,
    GovDeltaKind,
};
use crate::query::{Consistency, ReadMeta, StateReader};
use crate::{Kernel, KernelError};

const INTROSPECT_ADAPTER_ID: &str = "kernel.introspect";
const GOVERNANCE_ADAPTER_ID: &str = "kernel.governance";

/// Kinds handled entirely inside the kernel (no host adapter).
pub(crate) static INTERNAL_EFFECT_KINDS: &[&str] = &[
    "introspect.manifest",
    "introspect.reducer_state",
    "introspect.journal_head",
    "introspect.list_cells",
    "governance.propose",
    "governance.shadow",
    "governance.approve",
    "governance.apply",
];

#[derive(Debug, Serialize, Deserialize)]
struct ManifestParams {
    consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestReceipt {
    #[serde(with = "serde_bytes")]
    manifest: Vec<u8>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReducerStateParams {
    reducer: String,
    #[serde(default)]
    key: Option<Vec<u8>>, // bytes
    consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReducerStateReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<Vec<u8>>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListCellsParams {
    reducer: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ListCellsReceipt {
    cells: Vec<CellEntry>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct CellEntry {
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
    state_hash: [u8; 32],
    size: u64,
    last_active_ns: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeadReceipt {
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct MetaSer {
    journal_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    snapshot_hash: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    manifest_hash: Vec<u8>,
}

/// Map textual consistency param to enum.
fn parse_consistency(value: &str) -> Result<Consistency, KernelError> {
    let v = value.trim().to_lowercase();
    if v == "head" {
        return Ok(Consistency::Head);
    }
    if let Some(rest) = v.strip_prefix("exact:") {
        let h = rest
            .parse::<u64>()
            .map_err(|e| KernelError::Query(format!("invalid exact height '{rest}': {e}")))?;
        return Ok(Consistency::Exact(h));
    }
    if let Some(rest) = v.strip_prefix("at_least:") {
        let h = rest
            .parse::<u64>()
            .map_err(|e| KernelError::Query(format!("invalid at_least height '{rest}': {e}")))?;
        return Ok(Consistency::AtLeast(h));
    }
    Err(KernelError::Query(format!("unknown consistency '{value}'")))
}

impl<S> Kernel<S>
where
    S: aos_store::Store + 'static,
{
    /// Handle an internal effect intent and return its receipt if the kind is supported.
    pub fn handle_internal_intent(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Option<EffectReceipt>, KernelError> {
        if !INTERNAL_EFFECT_KINDS.contains(&intent.kind.as_str()) {
            return Ok(None);
        }

        let receipt_result = match intent.kind.as_str() {
            EffectKind::INTROSPECT_MANIFEST => self.handle_manifest(intent),
            EffectKind::INTROSPECT_REDUCER_STATE => self.handle_reducer_state(intent),
            EffectKind::INTROSPECT_JOURNAL_HEAD => self.handle_journal_head(intent),
            EffectKind::INTROSPECT_LIST_CELLS => self.handle_list_cells(intent),
            "governance.propose" => self.handle_governance_propose(intent),
            "governance.shadow" => self.handle_governance_shadow(intent),
            "governance.approve" => self.handle_governance_approve(intent),
            "governance.apply" => self.handle_governance_apply(intent),
            _ => unreachable!("guard ensures only internal kinds reach here"),
        };

        let adapter_id = if intent.kind.as_str().starts_with("governance.") {
            GOVERNANCE_ADAPTER_ID
        } else {
            INTROSPECT_ADAPTER_ID
        };
        let receipt = match receipt_result {
            Ok(payload_cbor) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: adapter_id.to_string(),
                status: ReceiptStatus::Ok,
                payload_cbor,
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
            Err(err) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: adapter_id.to_string(),
                status: ReceiptStatus::Error,
                payload_cbor: Vec::new(),
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
        };

        Ok(Some(receipt))
    }

    fn handle_manifest(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ManifestParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let consistency = parse_consistency(&params.consistency)?;
        let read = self.get_manifest(consistency)?;
        let manifest_bytes = to_canonical_cbor(&read.value)
            .map_err(|e| KernelError::Manifest(format!("encode manifest: {e}")))?;
        let receipt = ManifestReceipt {
            manifest: manifest_bytes,
            meta: to_meta(&read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_reducer_state(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ReducerStateParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let consistency = parse_consistency(&params.consistency)?;
        let state_read =
            self.get_reducer_state(&params.reducer, params.key.as_deref(), consistency)?;
        let receipt = ReducerStateReceipt {
            state: state_read.value,
            meta: to_meta(&state_read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_journal_head(&self, _intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let meta = self.get_journal_head();
        let receipt = JournalHeadReceipt {
            meta: to_meta(&meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_list_cells(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ListCellsParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let cells_meta = self.list_cells(&params.reducer)?;
        let cells: Vec<CellEntry> = cells_meta
            .into_iter()
            .map(|meta| CellEntry {
                key: meta.key_bytes,
                state_hash: meta.state_hash,
                size: meta.size,
                last_active_ns: meta.last_active_ns,
            })
            .collect();
        let receipt = ListCellsReceipt {
            cells,
            meta: to_meta(&self.read_meta()),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_governance_propose(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

    fn handle_governance_shadow(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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
                        Some(value) => Some(
                            serde_json::to_string(&value).map_err(|err| {
                                KernelError::Manifest(format!("encode params_json: {err}"))
                            })?,
                        ),
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
            pending_receipts: summary
                .pending_receipts
                .into_iter()
                .map(|pending| {
                    Ok(GovPendingReceipt {
                        plan_id: pending.plan_id,
                        plan: pending.plan,
                        intent_hash: hash_ref_from_hex(&pending.intent_hash)?,
                    })
                })
                .collect::<Result<Vec<_>, KernelError>>()?,
            plan_results: summary
                .plan_results
                .into_iter()
                .map(|result| GovPlanResultPreview {
                    plan: result.plan,
                    plan_id: result.plan_id,
                    output_schema: result.output_schema,
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

    fn handle_governance_approve(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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
            GovApprovalDecision::Approve => self.approve_proposal(params.proposal_id, params.approver.clone())?,
            GovApprovalDecision::Reject => self.reject_proposal(params.proposal_id, params.approver.clone())?,
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

    fn handle_governance_apply(&mut self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

fn hash_ref_from_hex(hex: &str) -> Result<HashRef, KernelError> {
    let value = format!("sha256:{hex}");
    HashRef::new(value).map_err(|e| KernelError::Manifest(format!("invalid hash: {e}")))
}

fn to_meta(meta: &ReadMeta) -> MetaSer {
    MetaSer {
        journal_height: meta.journal_height,
        snapshot_hash: meta.snapshot_hash.as_ref().map(|h| h.as_bytes().to_vec()),
        manifest_hash: meta.manifest_hash.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KernelBuilder, KernelConfig};
    use aos_effects::IntentBuilder;
    use aos_store::MemStore;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write_minimal_manifest(path: &std::path::Path) {
        let manifest = json!({
            "air_version": "1",
            "schemas": [],
            "modules": [],
            "plans": [],
            "effects": [],
            "caps": [],
            "policies": [],
            "triggers": []
        });
        let bytes = serde_cbor::to_vec(&manifest).expect("cbor encode");
        let mut file = File::create(path).expect("create manifest");
        file.write_all(&bytes).expect("write manifest");
    }

    fn open_kernel() -> Kernel<MemStore> {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.cbor");
        write_minimal_manifest(&manifest_path);
        let store = Arc::new(MemStore::new());
        KernelBuilder::new(store)
            .from_manifest_path(&manifest_path)
            .expect("kernel")
    }

    #[test]
    fn manifest_intent_produces_receipt() {
        let mut kernel = open_kernel();
        let params = ManifestParams {
            consistency: "head".into(),
        };
        let intent = IntentBuilder::new(EffectKind::introspect_manifest(), "sys/query@1", &params)
            .build()
            .unwrap();

        let receipt = kernel
            .handle_internal_intent(&intent)
            .expect("ok")
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        let decoded: ManifestReceipt = receipt.payload().unwrap();
        assert!(decoded.manifest.len() > 0);
        assert_eq!(decoded.meta.journal_height, 0);
    }

    #[test]
    fn parse_consistency_variants() {
        assert!(matches!(parse_consistency("head"), Ok(Consistency::Head)));
        assert_eq!(parse_consistency("exact:5").unwrap(), Consistency::Exact(5));
        assert_eq!(
            parse_consistency("at_least:10").unwrap(),
            Consistency::AtLeast(10)
        );
        assert!(parse_consistency("bogus").is_err());
    }

    #[test]
    fn invalid_params_returns_error_receipt() {
        let mut kernel = open_kernel();
        // bogus CBOR payload
        let intent = EffectIntent {
            kind: EffectKind::introspect_manifest(),
            cap_name: "sys/query@1".into(),
            params_cbor: b"\x01\x02\x03".to_vec(),
            idempotency_key: [0; 32],
            intent_hash: [9; 32],
        };
        let receipt = kernel
            .handle_internal_intent(&intent)
            .unwrap()
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Error);
        assert_eq!(receipt.adapter_id, INTROSPECT_ADAPTER_ID);
    }

    #[test]
    fn list_cells_empty_for_non_keyed() {
        let mut kernel = open_kernel();
        let params = ListCellsParams {
            reducer: "missing/Reducer@1".into(),
        };
        let intent =
            IntentBuilder::new(EffectKind::introspect_list_cells(), "sys/query@1", &params)
                .build()
                .unwrap();

        let receipt = kernel
            .handle_internal_intent(&intent)
            .unwrap()
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        let decoded: ListCellsReceipt = receipt.payload().unwrap();
        assert!(decoded.cells.is_empty());
    }
}
