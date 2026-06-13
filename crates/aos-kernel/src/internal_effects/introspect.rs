use aos_cbor::{Hash, to_canonical_cbor};
use aos_effect_types::{
    HashRef, IntrospectCellInfo, IntrospectJournalHeadReceipt, IntrospectListCellsParams,
    IntrospectListCellsReceipt, IntrospectManifestParams, IntrospectManifestReceipt,
    IntrospectWorkflowStateParams, IntrospectWorkflowStateReceipt,
};
use aos_effects::EffectIntent;

use crate::cell_index::CellMeta;
use crate::query::StateReader;
use crate::{Kernel, KernelError};

pub(super) type ManifestParams = IntrospectManifestParams;
pub(super) type ManifestReceipt = IntrospectManifestReceipt;
type WorkflowStateParams = IntrospectWorkflowStateParams;
type WorkflowStateReceipt = IntrospectWorkflowStateReceipt;
pub(super) type ListCellsParams = IntrospectListCellsParams;
pub(super) type ListCellsReceipt = IntrospectListCellsReceipt;
type JournalHeadReceipt = IntrospectJournalHeadReceipt;

use super::{parse_consistency, to_meta};

impl<S> Kernel<S>
where
    S: crate::Store + 'static,
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
            meta: to_meta(&read.meta)?,
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
            meta: to_meta(&state_read.meta)?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_journal_head(
        &self,
        _intent: &EffectIntent,
    ) -> Result<Vec<u8>, KernelError> {
        let meta = self.get_journal_head();
        let receipt = JournalHeadReceipt {
            meta: to_meta(&meta)?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }

    pub(super) fn handle_list_cells(&self, intent: &EffectIntent) -> Result<Vec<u8>, KernelError> {
        let params: ListCellsParams = intent
            .params()
            .map_err(|e| KernelError::Query(format!("decode params: {e}")))?;
        let cells_meta = self.list_cells(&params.workflow)?;
        let cells: Vec<IntrospectCellInfo> = cells_meta
            .into_iter()
            .map(|meta: CellMeta| {
                Ok(IntrospectCellInfo {
                    key: meta.key_bytes,
                    state_hash: hash_ref_from_bytes(meta.state_hash)?,
                    size: meta.size,
                    last_active_ns: meta.last_active_ns,
                })
            })
            .collect::<Result<Vec<_>, KernelError>>()?;
        let receipt = ListCellsReceipt {
            cells,
            meta: to_meta(&self.read_meta())?,
        };
        Ok(to_canonical_cbor(&receipt).map_err(|e| KernelError::Manifest(e.to_string()))?)
    }
}

fn hash_ref_from_bytes(bytes: [u8; 32]) -> Result<HashRef, KernelError> {
    let hash = Hash::from_bytes(&bytes)
        .map_err(|err| KernelError::Manifest(format!("invalid hash bytes: {err}")))?;
    HashRef::new(hash.to_hex()).map_err(|err| KernelError::Manifest(err.to_string()))
}
