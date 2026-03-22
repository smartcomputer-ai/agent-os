use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovDecision, GovPatchInput, GovProposeParams,
    GovShadowParams, HashRef,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{KernelError, Store};
use aos_runtime::{HostError, WorldHost};

use crate::CommandIngress;

use super::decode::parse_plane_hash_ref;
use super::traits::PlaneError;

fn prepare_manifest_patch<S: Store + 'static>(
    host: &WorldHost<S>,
    input: GovPatchInput,
    manifest_base: Option<HashRef>,
) -> Result<ManifestPatch, PlaneError> {
    match input {
        GovPatchInput::Hash(hash) => {
            if manifest_base.is_some() {
                return Err(PlaneError::Kernel(KernelError::Manifest(
                    "manifest_base is not supported with patch hash input".into(),
                )));
            }
            let bytes = host
                .store()
                .get_blob(parse_plane_hash_ref(hash.as_str())?)
                .map_err(PlaneError::Store)?;
            Ok(serde_cbor::from_slice(&bytes)?)
        }
        GovPatchInput::PatchCbor(bytes) => {
            if manifest_base.is_some() {
                return Err(PlaneError::Kernel(KernelError::Manifest(
                    "manifest_base is not supported with patch_cbor input".into(),
                )));
            }
            let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
            canonicalize_patch(host.store(), patch).map_err(PlaneError::Kernel)
        }
        GovPatchInput::PatchDocJson(bytes) => {
            let doc: PatchDocument = serde_json::from_slice(&bytes).map_err(|err| {
                PlaneError::Host(HostError::External(format!(
                    "decode patch_doc_json submission payload: {err}"
                )))
            })?;
            if let Some(expected) = manifest_base.as_ref()
                && expected.as_str() != doc.base_manifest_hash
            {
                return Err(PlaneError::Kernel(KernelError::Manifest(format!(
                    "manifest_base mismatch: expected {expected}, got {}",
                    doc.base_manifest_hash
                ))));
            }
            compile_patch_document(host.store(), doc).map_err(PlaneError::Kernel)
        }
        GovPatchInput::PatchBlobRef { blob_ref, format } => {
            let bytes = host
                .store()
                .get_blob(parse_plane_hash_ref(blob_ref.as_str())?)
                .map_err(PlaneError::Store)?;
            match format.as_str() {
                "manifest_patch_cbor" => {
                    prepare_manifest_patch(host, GovPatchInput::PatchCbor(bytes), manifest_base)
                }
                "patch_doc_json" => {
                    prepare_manifest_patch(host, GovPatchInput::PatchDocJson(bytes), manifest_base)
                }
                other => Err(PlaneError::Kernel(KernelError::Manifest(format!(
                    "unknown patch blob format '{other}'"
                )))),
            }
        }
    }
}

pub fn run_governance_plane_command<S: Store + 'static>(
    host: &mut WorldHost<S>,
    control: &CommandIngress,
    payload: &[u8],
) -> Result<(), PlaneError> {
    match control.command.as_str() {
        "gov-propose" => {
            let params: GovProposeParams = serde_cbor::from_slice(payload)?;
            let patch = prepare_manifest_patch(host, params.patch.clone(), params.manifest_base)?;
            host.kernel_mut()
                .submit_proposal(patch, params.description.clone())?;
            Ok(())
        }
        "gov-shadow" => {
            let params: GovShadowParams = serde_cbor::from_slice(payload)?;
            let _ = host.kernel_mut().run_shadow(params.proposal_id, None)?;
            Ok(())
        }
        "gov-approve" => {
            let params: GovApproveParams = serde_cbor::from_slice(payload)?;
            match params.decision {
                GovDecision::Approve => host
                    .kernel_mut()
                    .approve_proposal(params.proposal_id, params.approver.clone())?,
                GovDecision::Reject => host
                    .kernel_mut()
                    .reject_proposal(params.proposal_id, params.approver.clone())?,
            }
            Ok(())
        }
        "gov-apply" => {
            let params: GovApplyParams = serde_cbor::from_slice(payload)?;
            host.kernel_mut().apply_proposal(params.proposal_id)?;
            Ok(())
        }
        other => Err(PlaneError::Host(HostError::External(format!(
            "unsupported plane command submission '{other}'"
        )))),
    }
}
