use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovDecision, GovPatchInput, GovProposeParams, GovShadowParams,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{KernelError, Store};
use aos_node::{CommandErrorBody, CommandIngress, CommandRecord, CommandStatus, WorldId};
use aos_runtime::{HostError, WorldHost};

use super::types::WorkerError;
use super::util::{parse_hash_ref, unix_time_ns};

pub(super) fn run_plane_command<S: Store + 'static>(
    host: &mut WorldHost<S>,
    control: &CommandIngress,
    payload: &[u8],
) -> Result<(), WorkerError> {
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
        other => Err(WorkerError::Host(HostError::External(format!(
            "unsupported log-first command submission '{other}'"
        )))),
    }
}

fn prepare_manifest_patch<S: Store + 'static>(
    host: &WorldHost<S>,
    input: GovPatchInput,
    manifest_base: Option<aos_effect_types::HashRef>,
) -> Result<ManifestPatch, WorkerError> {
    match input {
        GovPatchInput::Hash(hash) => {
            if manifest_base.is_some() {
                return Err(WorkerError::Kernel(KernelError::Manifest(
                    "manifest_base is not supported with patch hash input".into(),
                )));
            }
            let bytes = host
                .store()
                .get_blob(parse_hash_ref(hash.as_str())?)
                .map_err(WorkerError::Store)?;
            Ok(serde_cbor::from_slice(&bytes)?)
        }
        GovPatchInput::PatchCbor(bytes) => {
            if manifest_base.is_some() {
                return Err(WorkerError::Kernel(KernelError::Manifest(
                    "manifest_base is not supported with patch_cbor input".into(),
                )));
            }
            let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
            canonicalize_patch(host.store(), patch).map_err(WorkerError::Kernel)
        }
        GovPatchInput::PatchDocJson(bytes) => {
            let doc: PatchDocument = serde_json::from_slice(&bytes).map_err(WorkerError::Json)?;
            if let Some(expected) = manifest_base.as_ref()
                && expected.as_str() != doc.base_manifest_hash
            {
                return Err(WorkerError::Kernel(KernelError::Manifest(format!(
                    "manifest_base mismatch: expected {expected}, got {}",
                    doc.base_manifest_hash
                ))));
            }
            compile_patch_document(host.store(), doc).map_err(WorkerError::Kernel)
        }
        GovPatchInput::PatchBlobRef { blob_ref, format } => {
            let bytes = host
                .store()
                .get_blob(parse_hash_ref(blob_ref.as_str())?)
                .map_err(WorkerError::Store)?;
            match format.as_str() {
                "manifest_patch_cbor" => {
                    prepare_manifest_patch(host, GovPatchInput::PatchCbor(bytes), manifest_base)
                }
                "patch_doc_json" => {
                    prepare_manifest_patch(host, GovPatchInput::PatchDocJson(bytes), manifest_base)
                }
                other => Err(WorkerError::Kernel(KernelError::Manifest(format!(
                    "unknown patch blob format '{other}'"
                )))),
            }
        }
    }
}

pub(crate) fn command_submit_response(
    world_id: WorldId,
    record: CommandRecord,
) -> aos_node::api::CommandSubmitResponse {
    aos_node::api::CommandSubmitResponse {
        poll_url: format!("/v1/worlds/{world_id}/commands/{}", record.command_id),
        command_id: record.command_id,
        status: record.status,
    }
}

pub(super) fn synthesize_queued_command_record(command: &CommandIngress) -> CommandRecord {
    CommandRecord {
        command_id: command.command_id.clone(),
        command: command.command.clone(),
        status: CommandStatus::Queued,
        submitted_at_ns: command.submitted_at_ns,
        started_at_ns: None,
        finished_at_ns: None,
        journal_height: None,
        manifest_hash: None,
        result_payload: None,
        error: None,
    }
}

pub(super) fn command_running_record(record: &CommandRecord) -> CommandRecord {
    CommandRecord {
        status: CommandStatus::Running,
        started_at_ns: Some(record.started_at_ns.unwrap_or_else(unix_time_ns)),
        finished_at_ns: None,
        journal_height: None,
        manifest_hash: None,
        result_payload: None,
        error: None,
        ..record.clone()
    }
}

pub(super) fn command_succeeded_record(
    record: CommandRecord,
    journal_height: u64,
    manifest_hash: String,
) -> CommandRecord {
    CommandRecord {
        status: CommandStatus::Succeeded,
        started_at_ns: Some(record.started_at_ns.unwrap_or(record.submitted_at_ns)),
        finished_at_ns: Some(unix_time_ns()),
        journal_height: Some(journal_height),
        manifest_hash: Some(manifest_hash),
        result_payload: None,
        error: None,
        ..record
    }
}

pub(super) fn command_failed_record(
    record: CommandRecord,
    err: &WorkerError,
    journal_height: u64,
    manifest_hash: String,
) -> CommandRecord {
    CommandRecord {
        status: CommandStatus::Failed,
        started_at_ns: Some(record.started_at_ns.unwrap_or(record.submitted_at_ns)),
        finished_at_ns: Some(unix_time_ns()),
        journal_height: Some(journal_height),
        manifest_hash: Some(manifest_hash),
        result_payload: None,
        error: Some(CommandErrorBody {
            code: "command_failed".into(),
            message: err.to_string(),
        }),
        ..record
    }
}
