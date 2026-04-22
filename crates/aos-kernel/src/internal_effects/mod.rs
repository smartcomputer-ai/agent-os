use aos_cbor::to_canonical_cbor;
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus, effect_ops};
use serde::{Deserialize, Serialize};

use crate::query::{Consistency, ReadMeta};
use crate::{Kernel, KernelError};

mod governance;
mod introspect;
mod workspace;

/// Executor entrypoints handled entirely inside the kernel (no host adapter).
pub(crate) static INTERNAL_EFFECT_ENTRYPOINTS: &[&str] = &[
    effect_ops::INTROSPECT_MANIFEST,
    effect_ops::INTROSPECT_WORKFLOW_STATE,
    effect_ops::INTROSPECT_JOURNAL_HEAD,
    effect_ops::INTROSPECT_LIST_CELLS,
    effect_ops::WORKSPACE_RESOLVE,
    effect_ops::WORKSPACE_EMPTY_ROOT,
    effect_ops::WORKSPACE_LIST,
    effect_ops::WORKSPACE_READ_REF,
    effect_ops::WORKSPACE_READ_BYTES,
    effect_ops::WORKSPACE_WRITE_BYTES,
    effect_ops::WORKSPACE_WRITE_REF,
    effect_ops::WORKSPACE_REMOVE,
    effect_ops::WORKSPACE_DIFF,
    effect_ops::WORKSPACE_ANNOTATIONS_GET,
    effect_ops::WORKSPACE_ANNOTATIONS_SET,
    "sys/governance.propose@1",
    "sys/governance.shadow@1",
    "sys/governance.approve@1",
    "sys/governance.apply@1",
];

#[derive(Debug, Serialize, Deserialize)]
struct MetaSer {
    journal_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    snapshot_hash: Option<Vec<u8>>,
    #[serde(with = "serde_bytes")]
    manifest_hash: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_baseline_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_baseline_receipt_horizon_height: Option<u64>,
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

fn to_meta(meta: &ReadMeta) -> MetaSer {
    MetaSer {
        journal_height: meta.journal_height,
        snapshot_hash: meta.snapshot_hash.as_ref().map(|h| h.as_bytes().to_vec()),
        manifest_hash: meta.manifest_hash.as_bytes().to_vec(),
        active_baseline_height: meta.active_baseline_height,
        active_baseline_receipt_horizon_height: meta.active_baseline_receipt_horizon_height,
    }
}

impl<S> Kernel<S>
where
    S: crate::Store + 'static,
{
    /// Handle an internal effect intent and return its receipt if the op is supported.
    pub fn handle_internal_intent(
        &mut self,
        intent: &EffectIntent,
    ) -> Result<Option<EffectReceipt>, KernelError> {
        let effect = intent.effect.as_str();
        if !INTERNAL_EFFECT_ENTRYPOINTS.contains(&effect) {
            return Ok(None);
        }

        let receipt_result = match effect {
            effect_ops::INTROSPECT_MANIFEST => self.handle_manifest(intent),
            effect_ops::INTROSPECT_WORKFLOW_STATE => self.handle_workflow_state(intent),
            effect_ops::INTROSPECT_JOURNAL_HEAD => self.handle_journal_head(intent),
            effect_ops::INTROSPECT_LIST_CELLS => self.handle_list_cells(intent),
            effect_ops::WORKSPACE_RESOLVE => self.handle_workspace_resolve(intent),
            effect_ops::WORKSPACE_EMPTY_ROOT => self.handle_workspace_empty_root(intent),
            effect_ops::WORKSPACE_LIST => self.handle_workspace_list(intent),
            effect_ops::WORKSPACE_READ_REF => self.handle_workspace_read_ref(intent),
            effect_ops::WORKSPACE_READ_BYTES => self.handle_workspace_read_bytes(intent),
            effect_ops::WORKSPACE_WRITE_BYTES => self.handle_workspace_write_bytes(intent),
            effect_ops::WORKSPACE_WRITE_REF => self.handle_workspace_write_ref(intent),
            effect_ops::WORKSPACE_REMOVE => self.handle_workspace_remove(intent),
            effect_ops::WORKSPACE_DIFF => self.handle_workspace_diff(intent),
            effect_ops::WORKSPACE_ANNOTATIONS_GET => self.handle_workspace_annotations_get(intent),
            effect_ops::WORKSPACE_ANNOTATIONS_SET => self.handle_workspace_annotations_set(intent),
            "sys/governance.propose@1" => self.handle_governance_propose(intent),
            "sys/governance.shadow@1" => self.handle_governance_shadow(intent),
            "sys/governance.approve@1" => self.handle_governance_approve(intent),
            "sys/governance.apply@1" => self.handle_governance_apply(intent),
            _ => unreachable!("guard ensures only internal kinds reach here"),
        };

        let receipt = match receipt_result {
            Ok(payload_cbor) => EffectReceipt {
                intent_hash: intent.intent_hash,
                status: ReceiptStatus::Ok,
                payload_cbor,
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
            Err(err) => {
                let payload_cbor = to_canonical_cbor(&err.to_string()).unwrap_or_default();
                EffectReceipt {
                    intent_hash: intent.intent_hash,
                    status: ReceiptStatus::Error,
                    payload_cbor,
                    cost_cents: Some(0),
                    signature: vec![0; 64],
                }
            }
        };

        Ok(Some(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KernelBuilder;
    use crate::MemStore;
    use crate::internal_effects::introspect::{
        ListCellsParams, ListCellsReceipt, ManifestParams, ManifestReceipt,
    };
    use aos_effects::IntentBuilder;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write_minimal_manifest(path: &std::path::Path) {
        let manifest = json!({
            "air_version": "2",
            "schemas": [],
            "modules": [],
            "workflows": [],
            "effects": [],
            "secrets": []
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
        let intent = IntentBuilder::new(effect_ops::INTROSPECT_MANIFEST, &params)
            .build()
            .unwrap();

        let receipt = kernel
            .handle_internal_intent(&intent)
            .expect("ok")
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Ok);
        let decoded: ManifestReceipt = receipt.payload().unwrap();
        assert!(!decoded.manifest.is_empty());
        // New worlds now include an initial baseline snapshot plus manifest record.
        assert_eq!(decoded.meta.journal_height, 2);
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
            effect: effect_ops::INTROSPECT_MANIFEST.into(),
            effect_hash: None,
            executor_module: None,
            executor_module_hash: None,
            executor_entrypoint: Some(effect_ops::INTROSPECT_MANIFEST.into()),
            params_cbor: b"\x01\x02\x03".to_vec(),
            idempotency_key: [0; 32],
            intent_hash: [9; 32],
        };
        let receipt = kernel
            .handle_internal_intent(&intent)
            .unwrap()
            .expect("handled");
        assert_eq!(receipt.status, ReceiptStatus::Error);
    }

    #[test]
    fn list_cells_empty_for_non_keyed() {
        let mut kernel = open_kernel();
        let params = ListCellsParams {
            workflow: "missing/Workflow@1".into(),
        };
        let intent = IntentBuilder::new(effect_ops::INTROSPECT_LIST_CELLS, &params)
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
