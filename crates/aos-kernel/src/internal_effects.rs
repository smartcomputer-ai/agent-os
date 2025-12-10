use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::{EffectIntent, EffectReceipt, EffectKind, ReceiptStatus};
use serde::{Deserialize, Serialize};

use crate::cell_index::CellMeta;
use crate::query::{Consistency, ReadMeta, StateReader};
use crate::{Kernel, KernelError};

const ADAPTER_ID: &str = "kernel.introspect";

/// Kinds handled entirely inside the kernel (no host adapter).
pub(crate) static INTERNAL_EFFECT_KINDS: &[&str] = &[
    "introspect.manifest",
    "introspect.reducer_state",
    "introspect.journal_head",
    "introspect.list_cells",
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
        &self,
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
            _ => unreachable!("guard ensures only internal kinds reach here"),
        };

        let receipt = match receipt_result {
            Ok(payload_cbor) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: ADAPTER_ID.to_string(),
                status: ReceiptStatus::Ok,
                payload_cbor,
                cost_cents: Some(0),
                signature: vec![0; 64],
            },
            Err(err) => EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: ADAPTER_ID.to_string(),
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
        let state_read = self.get_reducer_state(&params.reducer, params.key.as_deref(), consistency)?;
        let receipt = ReducerStateReceipt {
            state: state_read.value,
            meta: to_meta(&state_read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    fn handle_journal_head(&self, _intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let meta = self.get_journal_head();
        let receipt = JournalHeadReceipt { meta: to_meta(&meta) };
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
}

fn to_meta(meta: &ReadMeta) -> MetaSer {
    MetaSer {
        journal_height: meta.journal_height,
        snapshot_hash: meta
            .snapshot_hash
            .as_ref()
            .map(|h| h.as_bytes().to_vec()),
        manifest_hash: meta.manifest_hash.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_effects::IntentBuilder;
    use aos_store::MemStore;
    use crate::{KernelBuilder, KernelConfig};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use std::fs::File;
    use std::io::Write;

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
        let kernel = open_kernel();
        let params = ManifestParams {
            consistency: "head".into(),
        };
        let intent = IntentBuilder::new(
            EffectKind::introspect_manifest(),
            "sys/query@1",
            &params,
        )
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
        let kernel = open_kernel();
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
        assert_eq!(receipt.adapter_id, ADAPTER_ID);
    }

    #[test]
    fn list_cells_empty_for_non_keyed() {
        let kernel = open_kernel();
        let params = ListCellsParams {
            reducer: "missing/Reducer@1".into(),
        };
        let intent = IntentBuilder::new(
            EffectKind::introspect_list_cells(),
            "sys/query@1",
            &params,
        )
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
