use crate::contracts::SessionState;
use crate::helpers::workflow::SessionReduceError;
use crate::{helpers::llm::LlmMappingError, tools::ToolEffectKind};
use alloc::string::String;
use aos_effects::builtins::{BlobGetParams, BlobPutParams, LlmGenerateParams, TextOrSecretRef};
use aos_wasm_sdk::{PendingEffect, ReduceError, Value, WorkflowCtx};

use super::llm::{LlmStepContext, materialize_llm_generate_params_with_prompt_refs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffectCommand {
    LlmGenerate {
        params: LlmGenerateParams,
        pending: PendingEffect,
    },
    ToolEffect {
        kind: ToolEffectKind,
        params_json: String,
        pending: PendingEffect,
    },
    BlobPut {
        params: BlobPutParams,
        pending: PendingEffect,
    },
    BlobGet {
        params: BlobGetParams,
        pending: PendingEffect,
    },
}

impl SessionEffectCommand {
    pub fn pending(&self) -> &PendingEffect {
        match self {
            Self::LlmGenerate { pending, .. }
            | Self::ToolEffect { pending, .. }
            | Self::BlobPut { pending, .. }
            | Self::BlobGet { pending, .. } => pending,
        }
    }

    pub fn emit(self, ctx: &mut WorkflowCtx<SessionState, Value>) {
        match self {
            Self::LlmGenerate { params, pending } => {
                if let Some(cap_slot) = pending.cap_slot.as_deref() {
                    let mut effects = ctx.effects();
                    effects.sys().llm_generate(&params, cap_slot);
                } else {
                    ctx.effects()
                        .emit_raw_with_issuer_ref("llm.generate", &params, None, None);
                }
            }
            Self::ToolEffect {
                kind,
                params_json,
                pending,
            } => {
                let params: serde_json::Value =
                    serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Null);
                ctx.effects().emit_raw_with_issuer_ref(
                    kind.as_str(),
                    &params,
                    pending.cap_slot.as_deref(),
                    pending.issuer_ref.as_deref(),
                );
            }
            Self::BlobPut { params, pending } => {
                if let Some(cap_slot) = pending.cap_slot.as_deref() {
                    let mut effects = ctx.effects();
                    effects.sys().blob_put(&params, cap_slot);
                } else {
                    ctx.effects().emit_raw_with_issuer_ref(
                        "blob.put",
                        &params,
                        None,
                        pending.issuer_ref.as_deref(),
                    );
                }
            }
            Self::BlobGet { params, pending } => {
                if let Some(cap_slot) = pending.cap_slot.as_deref() {
                    let mut effects = ctx.effects();
                    effects.sys().blob_get(&params, cap_slot);
                } else {
                    ctx.effects().emit_raw_with_issuer_ref(
                        "blob.get",
                        &params,
                        None,
                        pending.issuer_ref.as_deref(),
                    );
                }
            }
        }
    }

    pub fn params_hash(&self) -> &str {
        self.pending().params_hash.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionReduceOutput {
    pub effects: alloc::vec::Vec<SessionEffectCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLlm {
    pub step: LlmStepContext,
    pub cap_slot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedLlm {
    pub pending: PendingEffect,
    pub params: LlmGenerateParams,
}

pub fn request_llm(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
    mut request: RequestLlm,
) -> Result<RequestedLlm, SessionReduceError> {
    let run_config = state
        .active_run_config
        .clone()
        .ok_or(SessionReduceError::RunNotActive)?;
    if request.step.api_key.is_none() {
        request.step.api_key = provider_secret_ref(run_config.provider.as_str());
    }
    let params = materialize_llm_generate_params_with_prompt_refs(&run_config, request.step)
        .map_err(map_llm_mapping_error)?;
    let pending = begin_pending_effect(state, "llm.generate", &params, request.cap_slot, None);
    out.effects.push(SessionEffectCommand::LlmGenerate {
        params: params.clone(),
        pending: pending.clone(),
    });
    Ok(RequestedLlm { pending, params })
}

pub fn begin_pending_effect<T: serde::Serialize>(
    state: &mut SessionState,
    effect_kind: &'static str,
    params: &T,
    cap_slot: Option<String>,
    issuer_ref: Option<String>,
) -> PendingEffect {
    match state.pending_effects.begin_with_issuer_ref(
        effect_kind,
        params,
        cap_slot.clone(),
        state.updated_at,
        issuer_ref.clone(),
    ) {
        Ok(pending) => pending,
        Err(_) => {
            let pending =
                PendingEffect::new(effect_kind, String::new(), cap_slot, state.updated_at)
                    .with_issuer_ref_opt(issuer_ref);
            state.pending_effects.insert(pending.clone());
            pending
        }
    }
}

fn map_llm_mapping_error(err: LlmMappingError) -> SessionReduceError {
    match err {
        LlmMappingError::MissingProvider => SessionReduceError::MissingProvider,
        LlmMappingError::MissingModel => SessionReduceError::MissingModel,
        LlmMappingError::EmptyMessageRefs => SessionReduceError::EmptyMessageRefs,
        LlmMappingError::InvalidHashRef => SessionReduceError::InvalidHashRef,
    }
}

fn provider_secret_ref(provider: &str) -> Option<TextOrSecretRef> {
    let normalized = provider.trim().to_ascii_lowercase();
    if normalized.contains("anthropic") {
        return Some(TextOrSecretRef::secret("llm/anthropic_api", 1));
    }
    if normalized.contains("openai") {
        return Some(TextOrSecretRef::secret("llm/openai_api", 1));
    }
    None
}

pub fn map_reduce_error(err: SessionReduceError) -> ReduceError {
    match err {
        SessionReduceError::InvalidLifecycleTransition => {
            ReduceError::new("invalid lifecycle transition")
        }
        SessionReduceError::HostCommandRejected => ReduceError::new("host command rejected"),
        SessionReduceError::ToolBatchAlreadyActive => ReduceError::new("tool batch already active"),
        SessionReduceError::MissingProvider => ReduceError::new("run config provider missing"),
        SessionReduceError::MissingModel => ReduceError::new("run config model missing"),
        SessionReduceError::UnknownProvider => ReduceError::new("run config provider unknown"),
        SessionReduceError::UnknownModel => ReduceError::new("run config model unknown"),
        SessionReduceError::RunAlreadyActive => ReduceError::new("run already active"),
        SessionReduceError::RunNotActive => ReduceError::new("run not active"),
        SessionReduceError::EmptyMessageRefs => {
            ReduceError::new("llm message_refs must not be empty")
        }
        SessionReduceError::TooManyPendingEffects => ReduceError::new("too many pending effects"),
        SessionReduceError::InvalidHashRef => ReduceError::new("invalid hash ref"),
        SessionReduceError::ToolProfileUnknown => ReduceError::new("tool profile unknown"),
        SessionReduceError::UnknownToolOverride => ReduceError::new("unknown tool override"),
        SessionReduceError::InvalidToolRegistry => ReduceError::new("invalid tool registry"),
        SessionReduceError::AmbiguousPendingToolEffect => {
            ReduceError::new("ambiguous pending tool effect")
        }
    }
}
