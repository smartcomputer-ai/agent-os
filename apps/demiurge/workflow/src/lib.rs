#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use aos_agent_sdk::{
    EffectReceiptRejected, EffectStreamFrameEnvelope, SessionEffectCommand, SessionId, SessionIngress,
    SessionIngressKind, SessionReduceError, SessionRuntimeLimits, SessionState, SysLlmGenerateParams,
    SysLlmRuntimeArgs,
    SessionWorkflowEvent, ToolBatchId, ToolCallStatus, apply_session_workflow_event_with_catalog_and_limits,
};
use aos_wasm_sdk::{EffectReceiptEnvelope, ReduceError, Workflow, WorkflowCtx, Value, aos_workflow};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

aos_workflow!(Demiurge);

const KNOWN_PROVIDERS: &[&str] = &["openai-responses", "anthropic", "openai-compatible", "mock"];
const KNOWN_MODELS: &[&str] = &[
    "gpt-5.2",
    "gpt-5-mini",
    "gpt-5.2-codex",
    "claude-sonnet-4-5",
    "gpt-mock",
];
const RUNTIME_LIMITS: SessionRuntimeLimits = SessionRuntimeLimits {
    max_pending_intents: Some(64),
};

const OPENAI_SECRET_ALIAS: &str = "llm/openai_api";
const ANTHROPIC_SECRET_ALIAS: &str = "llm/anthropic_api";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SecretRef {
    alias: String,
    version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
enum TextOrSecretRef {
    #[serde(rename = "literal")]
    Literal(String),
    #[serde(rename = "secret")]
    Secret(SecretRef),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LlmGenerateParamsWithSecret {
    correlation_id: Option<String>,
    provider: String,
    model: String,
    message_refs: Vec<String>,
    runtime: SysLlmRuntimeArgs,
    api_key: Option<TextOrSecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DemiurgeState {
    pub session: SessionState,
    pub pending_tool_call: Option<PendingToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingToolCall {
    pub tool_batch_id: ToolBatchId,
    pub call_id: String,
    pub finalize_batch: bool,
    pub stage: PendingToolStage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum PendingToolStage {
    AwaitIntrospectManifest,
    AwaitWorkspaceResolve { path: String },
    AwaitWorkspaceReadBytes,
}

impl PendingToolStage {
    fn effect_kind(&self) -> &'static str {
        match self {
            Self::AwaitIntrospectManifest => "introspect.manifest",
            Self::AwaitWorkspaceResolve { .. } => "workspace.resolve",
            Self::AwaitWorkspaceReadBytes => "workspace.read_bytes",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "$tag", content = "$value")]
pub enum DemiurgeWorkflowEvent {
    Ingress(SessionIngress),
    Receipt(EffectReceiptEnvelope),
    ReceiptRejected(EffectReceiptRejected),
    StreamFrame(EffectStreamFrameEnvelope),
    ToolCallRequested(ToolCallRequested),
    #[default]
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallRequested {
    pub session_id: SessionId,
    pub observed_at_ns: u64,
    pub tool_batch_id: ToolBatchId,
    pub call_id: String,
    pub finalize_batch: bool,
    pub params: ToolCallParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "$tag", content = "$value")]
pub enum ToolCallParams {
    IntrospectManifest { consistency: String },
    WorkspaceReadBytes {
        workspace: String,
        version: Option<u64>,
        path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IntrospectManifestParams {
    consistency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkspaceResolveParams {
    workspace: String,
    version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkspaceReadBytesParams {
    root_hash: String,
    path: String,
    range: Option<WorkspaceReadRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkspaceReadRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WorkspaceResolveReceipt {
    exists: bool,
    root_hash: Option<String>,
}

#[derive(Default)]
struct Demiurge;

impl Workflow for Demiurge {
    type State = DemiurgeState;
    type Event = DemiurgeWorkflowEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut WorkflowCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        match event {
            DemiurgeWorkflowEvent::Ingress(ingress) => {
                apply_sdk_event(ctx, SessionWorkflowEvent::Ingress(ingress))
            }
            DemiurgeWorkflowEvent::StreamFrame(frame) => {
                apply_sdk_event(ctx, SessionWorkflowEvent::StreamFrame(frame))
            }
            DemiurgeWorkflowEvent::Receipt(receipt) => {
                if handle_custom_tool_receipt(ctx, &receipt)? {
                    Ok(())
                } else {
                    apply_sdk_event(ctx, SessionWorkflowEvent::Receipt(receipt))
                }
            }
            DemiurgeWorkflowEvent::ReceiptRejected(rejected) => {
                if handle_custom_tool_rejection(ctx, &rejected)? {
                    Ok(())
                } else {
                    apply_sdk_event(ctx, SessionWorkflowEvent::ReceiptRejected(rejected))
                }
            }
            DemiurgeWorkflowEvent::ToolCallRequested(requested) => handle_tool_call_requested(ctx, requested),
            DemiurgeWorkflowEvent::Noop => apply_sdk_event(ctx, SessionWorkflowEvent::Noop),
        }
    }
}

fn apply_sdk_event(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    event: SessionWorkflowEvent,
) -> Result<(), ReduceError> {
    let out = apply_session_workflow_event_with_catalog_and_limits(
        &mut ctx.state.session,
        &event,
        KNOWN_PROVIDERS,
        KNOWN_MODELS,
        RUNTIME_LIMITS,
    )
    .map_err(map_reduce_error)?;
    emit_session_effects(ctx, out.effects);
    Ok(())
}

fn emit_session_effects(ctx: &mut WorkflowCtx<DemiurgeState, Value>, effects: Vec<SessionEffectCommand>) {
    for effect in effects {
        match effect {
            SessionEffectCommand::LlmGenerate {
                params, cap_slot, ..
            } => {
                let params = llm_params_with_secret(params);
                ctx.effects()
                    .emit_raw("llm.generate", &params, cap_slot.as_deref())
            }
        }
    }
}

fn llm_params_with_secret(params: SysLlmGenerateParams) -> LlmGenerateParamsWithSecret {
    let api_key = params
        .api_key
        .map(TextOrSecretRef::Literal)
        .or_else(|| {
            llm_provider_secret_alias(&params.provider).map(|alias| {
                TextOrSecretRef::Secret(SecretRef {
                    alias: alias.into(),
                    version: 1,
                })
            })
        });

    LlmGenerateParamsWithSecret {
        correlation_id: params.correlation_id,
        provider: params.provider,
        model: params.model,
        message_refs: params.message_refs,
        runtime: params.runtime,
        api_key,
    }
}

fn llm_provider_secret_alias(provider: &str) -> Option<&'static str> {
    match provider {
        "openai-responses" | "openai-chat" | "openai-compatible" | "openai" => {
            Some(OPENAI_SECRET_ALIAS)
        }
        "anthropic" => Some(ANTHROPIC_SECRET_ALIAS),
        _ => None,
    }
}

fn handle_tool_call_requested(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    requested: ToolCallRequested,
) -> Result<(), ReduceError> {
    if ctx.state.pending_tool_call.is_some() {
        settle_tool_call(
            ctx,
            &requested.session_id,
            requested.observed_at_ns,
            &requested.tool_batch_id,
            &requested.call_id,
            ToolCallStatus::Failed {
                code: "tool_busy".into(),
                detail: "previous tool call is still in flight".into(),
            },
            requested.finalize_batch,
        )?;
        return Ok(());
    }

    match requested.params {
        ToolCallParams::IntrospectManifest { consistency } => {
            ctx.effects().emit_raw(
                "introspect.manifest",
                &IntrospectManifestParams { consistency },
                Some("query"),
            );
            ctx.state.pending_tool_call = Some(PendingToolCall {
                tool_batch_id: requested.tool_batch_id,
                call_id: requested.call_id,
                finalize_batch: requested.finalize_batch,
                stage: PendingToolStage::AwaitIntrospectManifest,
            });
        }
        ToolCallParams::WorkspaceReadBytes {
            workspace,
            version,
            path,
        } => {
            ctx.effects().emit_raw(
                "workspace.resolve",
                &WorkspaceResolveParams { workspace, version },
                Some("workspace"),
            );
            ctx.state.pending_tool_call = Some(PendingToolCall {
                tool_batch_id: requested.tool_batch_id,
                call_id: requested.call_id,
                finalize_batch: requested.finalize_batch,
                stage: PendingToolStage::AwaitWorkspaceResolve { path },
            });
        }
    }
    Ok(())
}

fn handle_custom_tool_receipt(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    receipt: &EffectReceiptEnvelope,
) -> Result<bool, ReduceError> {
    let Some(pending) = ctx.state.pending_tool_call.clone() else {
        return Ok(false);
    };
    if pending.stage.effect_kind() != receipt.effect_kind {
        return Ok(false);
    }

    let observed_at_ns = receipt.emitted_at_seq;
    if receipt.status != "ok" {
        ctx.state.pending_tool_call = None;
        let session_id = ctx.state.session.session_id.clone();
        settle_tool_call(
            ctx,
            &session_id,
            observed_at_ns,
            &pending.tool_batch_id,
            &pending.call_id,
            ToolCallStatus::Failed {
                code: "tool_effect_failed".into(),
                detail: format!(
                    "{} receipt status={} adapter={}",
                    receipt.effect_kind, receipt.status, receipt.adapter_id
                ),
            },
            pending.finalize_batch,
        )?;
        return Ok(true);
    }

    match pending.stage {
        PendingToolStage::AwaitIntrospectManifest => {
            ctx.state.pending_tool_call = None;
            let session_id = ctx.state.session.session_id.clone();
            settle_tool_call(
                ctx,
                &session_id,
                observed_at_ns,
                &pending.tool_batch_id,
                &pending.call_id,
                ToolCallStatus::Succeeded,
                pending.finalize_batch,
            )?;
        }
        PendingToolStage::AwaitWorkspaceResolve { path } => {
            let resolved: WorkspaceResolveReceipt = serde_cbor::from_slice(&receipt.receipt_payload)
                .map_err(|_| ReduceError::new("workspace.resolve receipt decode failed"))?;

            if !resolved.exists {
                ctx.state.pending_tool_call = None;
                let session_id = ctx.state.session.session_id.clone();
                settle_tool_call(
                    ctx,
                    &session_id,
                    observed_at_ns,
                    &pending.tool_batch_id,
                    &pending.call_id,
                    ToolCallStatus::Failed {
                        code: "workspace_not_found".into(),
                        detail: "workspace.resolve returned exists=false".into(),
                    },
                    pending.finalize_batch,
                )?;
                return Ok(true);
            }

            let Some(root_hash) = resolved.root_hash else {
                ctx.state.pending_tool_call = None;
                let session_id = ctx.state.session.session_id.clone();
                settle_tool_call(
                    ctx,
                    &session_id,
                    observed_at_ns,
                    &pending.tool_batch_id,
                    &pending.call_id,
                    ToolCallStatus::Failed {
                        code: "workspace_root_missing".into(),
                        detail: "workspace.resolve returned no root_hash".into(),
                    },
                    pending.finalize_batch,
                )?;
                return Ok(true);
            };

            ctx.effects().emit_raw(
                "workspace.read_bytes",
                &WorkspaceReadBytesParams {
                    root_hash,
                    path,
                    range: None,
                },
                Some("workspace"),
            );
            ctx.state.pending_tool_call = Some(PendingToolCall {
                tool_batch_id: pending.tool_batch_id,
                call_id: pending.call_id,
                finalize_batch: pending.finalize_batch,
                stage: PendingToolStage::AwaitWorkspaceReadBytes,
            });
        }
        PendingToolStage::AwaitWorkspaceReadBytes => {
            ctx.state.pending_tool_call = None;
            let session_id = ctx.state.session.session_id.clone();
            settle_tool_call(
                ctx,
                &session_id,
                observed_at_ns,
                &pending.tool_batch_id,
                &pending.call_id,
                ToolCallStatus::Succeeded,
                pending.finalize_batch,
            )?;
        }
    }

    Ok(true)
}

fn handle_custom_tool_rejection(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    rejected: &EffectReceiptRejected,
) -> Result<bool, ReduceError> {
    let Some(pending) = ctx.state.pending_tool_call.clone() else {
        return Ok(false);
    };
    if pending.stage.effect_kind() != rejected.effect_kind {
        return Ok(false);
    }

    ctx.state.pending_tool_call = None;
    let session_id = ctx.state.session.session_id.clone();
    settle_tool_call(
        ctx,
        &session_id,
        rejected.emitted_at_seq,
        &pending.tool_batch_id,
        &pending.call_id,
        ToolCallStatus::Failed {
            code: rejected.error_code.clone(),
            detail: rejected.error_message.clone(),
        },
        pending.finalize_batch,
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn settle_tool_call(
    ctx: &mut WorkflowCtx<DemiurgeState, Value>,
    session_id: &SessionId,
    observed_at_ns: u64,
    tool_batch_id: &ToolBatchId,
    call_id: &str,
    status: ToolCallStatus,
    finalize_batch: bool,
) -> Result<(), ReduceError> {
    let settled = SessionIngressKind::ToolCallSettled {
        tool_batch_id: tool_batch_id.clone(),
        call_id: call_id.into(),
        status,
    };
    apply_sdk_event(
        ctx,
        SessionWorkflowEvent::Ingress(SessionIngress {
            session_id: session_id.clone(),
            observed_at_ns,
            ingress: settled,
        }),
    )?;

    if finalize_batch {
        apply_sdk_event(
            ctx,
            SessionWorkflowEvent::Ingress(SessionIngress {
                session_id: session_id.clone(),
                observed_at_ns: observed_at_ns.saturating_add(1),
                ingress: SessionIngressKind::ToolBatchSettled {
                    tool_batch_id: tool_batch_id.clone(),
                    results_ref: None,
                },
            }),
        )?;
    }

    Ok(())
}

fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::ToolBatchNotActive => ReduceError::new("tool batch not active"),
        SessionReduceError::ToolBatchIdMismatch => ReduceError::new("tool batch id mismatch"),
        SessionReduceError::ToolCallUnknown => ReduceError::new("tool call id not expected"),
        SessionReduceError::ToolBatchNotSettled => ReduceError::new("tool batch not settled"),
        SessionReduceError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionReduceError::MissingModel => ReduceError::new("run config model missing"),
        SessionReduceError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionReduceError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionReduceError::InvalidWorkspacePromptPackJson => {
            ReduceError::new("workspace prompt pack JSON invalid")
        }
        SessionReduceError::InvalidWorkspaceToolCatalogJson => {
            ReduceError::new("workspace tool catalog JSON invalid")
        }
        SessionReduceError::MissingWorkspacePromptPackBytes => {
            ReduceError::new("workspace prompt pack bytes missing for validation")
        }
        SessionReduceError::MissingWorkspaceToolCatalogBytes => {
            ReduceError::new("workspace tool catalog bytes missing for validation")
        }
        SessionReduceError::TooManyPendingIntents => ReduceError::new("too many pending intents"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn make_params(provider: &str, api_key: Option<&str>) -> SysLlmGenerateParams {
        SysLlmGenerateParams {
            correlation_id: Some("run-1".into()),
            provider: provider.into(),
            model: "gpt-5.2".into(),
            message_refs: vec!["sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()],
            runtime: SysLlmRuntimeArgs::default(),
            api_key: api_key.map(str::to_string),
        }
    }

    #[test]
    fn openai_provider_maps_to_secret_ref() {
        let mapped = llm_params_with_secret(make_params("openai-responses", None));
        assert_eq!(
            mapped.api_key,
            Some(TextOrSecretRef::Secret(SecretRef {
                alias: OPENAI_SECRET_ALIAS.into(),
                version: 1,
            }))
        );
    }

    #[test]
    fn anthropic_provider_maps_to_secret_ref() {
        let mapped = llm_params_with_secret(make_params("anthropic", None));
        assert_eq!(
            mapped.api_key,
            Some(TextOrSecretRef::Secret(SecretRef {
                alias: ANTHROPIC_SECRET_ALIAS.into(),
                version: 1,
            }))
        );
    }

    #[test]
    fn explicit_api_key_literal_is_preserved() {
        let mapped = llm_params_with_secret(make_params("openai-responses", Some("literal-key")));
        assert_eq!(
            mapped.api_key,
            Some(TextOrSecretRef::Literal("literal-key".into()))
        );
    }

    #[test]
    fn provider_without_mapping_emits_no_secret_ref() {
        let mapped = llm_params_with_secret(make_params("mock", None));
        assert_eq!(mapped.api_key, None);
    }
}
