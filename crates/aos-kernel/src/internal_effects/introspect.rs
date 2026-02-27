use aos_cbor::to_canonical_cbor;
use aos_effects::EffectIntent;
use serde::{Deserialize, Serialize};

use crate::cell_index::CellMeta;
use crate::query::StateReader;
use crate::{Kernel, KernelError};

use super::{MetaSer, parse_consistency, to_meta};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ManifestParams {
    pub(super) consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ManifestReceipt {
    #[serde(with = "serde_bytes")]
    pub(super) manifest: Vec<u8>,
    pub(super) meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkflowStateParams {
    workflow: String,
    #[serde(default)]
    key: Option<Vec<u8>>, // bytes
    consistency: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkflowStateReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<Vec<u8>>,
    meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ListCellsParams {
    pub(super) workflow: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ListCellsReceipt {
    pub(super) cells: Vec<CellEntry>,
    pub(super) meta: MetaSer,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CellEntry {
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

impl<S> Kernel<S>
where
    S: aos_store::Store + 'static,
{
    pub(super) fn handle_manifest(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
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

    pub(super) fn handle_workflow_state(
        &self,
        intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let params: WorkflowStateParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let consistency = parse_consistency(&params.consistency)?;
        let state_read =
            self.get_workflow_state(&params.workflow, params.key.as_deref(), consistency)?;
        let receipt = WorkflowStateReceipt {
            state: state_read.value,
            meta: to_meta(&state_read.meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_journal_head(
        &self,
        _intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let meta = self.get_journal_head();
        let receipt = JournalHeadReceipt {
            meta: to_meta(&meta),
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_list_cells(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ListCellsParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let cells_meta = self.list_cells(&params.workflow)?;
        let cells: Vec<CellEntry> = cells_meta
            .into_iter()
            .map(|meta: CellMeta| CellEntry {
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
