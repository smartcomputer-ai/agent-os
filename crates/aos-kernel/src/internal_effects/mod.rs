use aos_cbor::to_canonical_cbor;
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use serde::{Deserialize, Serialize};

use crate::query::{Consistency, ReadMeta};
use crate::{Kernel, KernelError};

mod governance;
mod introspect;
mod workspace;

const INTROSPECT_ADAPTER_ID: &str = "kernel.introspect";
const GOVERNANCE_ADAPTER_ID: &str = "kernel.governance";

/// Kinds handled entirely inside the kernel (no host adapter).
pub(crate) static INTERNAL_EFFECT_KINDS: &[&str] = &[
    "introspect.manifest",
    "introspect.workflow_state",
    "introspect.journal_head",
    "introspect.list_cells",
    "workspace.resolve",
    "workspace.empty_root",
    "workspace.list",
    "workspace.read_ref",
    "workspace.read_bytes",
    "workspace.write_bytes",
    "workspace.remove",
    "workspace.diff",
    "workspace.annotations_get",
    "workspace.annotations_set",
    "governance.propose",
    "governance.shadow",
    "governance.approve",
    "governance.apply",
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
            EffectKind::INTROSPECT_WORKFLOW_STATE => self.handle_workflow_state(intent),
            EffectKind::INTROSPECT_JOURNAL_HEAD => self.handle_journal_head(intent),
            EffectKind::INTROSPECT_LIST_CELLS => self.handle_list_cells(intent),
            "workspace.resolve" => self.handle_workspace_resolve(intent),
            "workspace.empty_root" => self.handle_workspace_empty_root(intent),
            "workspace.list" => self.handle_workspace_list(intent),
            "workspace.read_ref" => self.handle_workspace_read_ref(intent),
            "workspace.read_bytes" => self.handle_workspace_read_bytes(intent),
            "workspace.write_bytes" => self.handle_workspace_write_bytes(intent),
            "workspace.remove" => self.handle_workspace_remove(intent),
            "workspace.diff" => self.handle_workspace_diff(intent),
            "workspace.annotations_get" => self.handle_workspace_annotations_get(intent),
            "workspace.annotations_set" => self.handle_workspace_annotations_set(intent),
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
            Err(err) => {
                let payload_cbor = to_canonical_cbor(&err.to_string()).unwrap_or_default();
                EffectReceipt {
                    intent_hash: intent.intent_hash,
                    adapter_id: adapter_id.to_string(),
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
    use crate::internal_effects::introspect::{
        ListCellsParams, ListCellsReceipt, ManifestParams, ManifestReceipt,
    };
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
            workflow: "missing/Workflow@1".into(),
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
