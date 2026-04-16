use aos_effect_types::{
    GovApplyParams, GovApproveParams, GovDecision, GovPatchInput, GovProposeParams, GovShadowParams,
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::governance_utils::canonicalize_patch;
use aos_kernel::journal::ApprovalDecisionRecord;
use aos_kernel::patch_doc::{PatchDocument, compile_patch_document};
use aos_kernel::{KernelError, Store, WorldControl};
use aos_node::{CommandErrorBody, CommandIngress, CommandRecord, CommandStatus, WorldId};

use super::types::WorkerError;
use super::util::{parse_hash_ref, unix_time_ns};

pub(crate) fn world_control_from_command_payload<S: Store + 'static>(
    store: &S,
    command: &str,
    payload: &[u8],
) -> Result<WorldControl, WorkerError> {
    match command {
        "gov-propose" => {
            let params: GovProposeParams = serde_cbor::from_slice(payload)?;
            let patch = prepare_manifest_patch(store, params.patch, params.manifest_base)?;
            Ok(WorldControl::SubmitProposal {
                patch,
                description: params.description,
            })
        }
        "gov-shadow" => {
            let params: GovShadowParams = serde_cbor::from_slice(payload)?;
            Ok(WorldControl::RunShadow {
                proposal_id: params.proposal_id,
            })
        }
        "gov-approve" => {
            let params: GovApproveParams = serde_cbor::from_slice(payload)?;
            Ok(WorldControl::DecideProposal {
                proposal_id: params.proposal_id,
                approver: params.approver,
                decision: match params.decision {
                    GovDecision::Approve => ApprovalDecisionRecord::Approve,
                    GovDecision::Reject => ApprovalDecisionRecord::Reject,
                },
            })
        }
        "gov-apply" => {
            let params: GovApplyParams = serde_cbor::from_slice(payload)?;
            Ok(WorldControl::ApplyProposal {
                proposal_id: params.proposal_id,
            })
        }
        other => Err(WorkerError::Persist(aos_node::PersistError::validation(
            format!("unsupported world control command '{other}'"),
        ))),
    }
}

fn prepare_manifest_patch<S: Store + 'static>(
    store: &S,
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
            let bytes = store.get_blob(parse_hash_ref(hash.as_str())?)?;
            Ok(serde_cbor::from_slice(&bytes)?)
        }
        GovPatchInput::PatchCbor(bytes) => {
            if manifest_base.is_some() {
                return Err(WorkerError::Kernel(KernelError::Manifest(
                    "manifest_base is not supported with patch_cbor input".into(),
                )));
            }
            let patch: ManifestPatch = serde_cbor::from_slice(&bytes)?;
            canonicalize_patch(store, patch).map_err(WorkerError::Kernel)
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
            compile_patch_document(store, doc).map_err(WorkerError::Kernel)
        }
        GovPatchInput::PatchBlobRef { blob_ref, format } => {
            let bytes = store.get_blob(parse_hash_ref(blob_ref.as_str())?)?;
            match format.as_str() {
                "manifest_patch_cbor" => {
                    prepare_manifest_patch(store, GovPatchInput::PatchCbor(bytes), manifest_base)
                }
                "patch_doc_json" => {
                    prepare_manifest_patch(store, GovPatchInput::PatchDocJson(bytes), manifest_base)
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

pub(crate) fn synthesize_queued_command_record(command: &CommandIngress) -> CommandRecord {
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
