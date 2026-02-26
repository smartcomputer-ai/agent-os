use super::{
    allocate_run_id, can_apply_host_command, enqueue_host_text, pop_follow_up_if_ready,
    transition_lifecycle,
};
use crate::contracts::{
    ActiveToolBatch, HostCommandKind, PendingIntent, RunConfig, SessionConfig, SessionIngressKind,
    SessionLifecycle, SessionState, SessionWorkflowEvent, ToolCallStatus, WorkspaceApplyMode,
    WorkspaceBinding, WorkspaceSnapshot,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use super::llm::{
    LlmStepContext, LlmToolChoice, SysLlmGenerateParams,
    materialize_llm_generate_params_with_workspace,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffectCommand {
    LlmGenerate {
        params: SysLlmGenerateParams,
        cap_slot: Option<String>,
        params_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionReduceOutput {
    pub effects: Vec<SessionEffectCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionRuntimeLimits {
    pub max_pending_intents: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReduceError {
    InvalidLifecycleTransition,
    HostCommandRejected,
    ToolBatchAlreadyActive,
    ToolBatchNotActive,
    ToolBatchIdMismatch,
    ToolCallUnknown,
    ToolBatchNotSettled,
    MissingProvider,
    MissingModel,
    UnknownProvider,
    UnknownModel,
    RunAlreadyActive,
    InvalidWorkspacePromptPackJson,
    InvalidWorkspaceToolCatalogJson,
    MissingWorkspacePromptPackBytes,
    MissingWorkspaceToolCatalogBytes,
    TooManyPendingIntents,
}

impl SessionReduceError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLifecycleTransition => "invalid lifecycle transition",
            Self::HostCommandRejected => "host command rejected",
            Self::ToolBatchAlreadyActive => "tool batch already active",
            Self::ToolBatchNotActive => "tool batch not active",
            Self::ToolBatchIdMismatch => "tool batch id mismatch",
            Self::ToolCallUnknown => "tool call id not expected in active batch",
            Self::ToolBatchNotSettled => "tool batch not settled",
            Self::MissingProvider => "run config provider missing",
            Self::MissingModel => "run config model missing",
            Self::UnknownProvider => "run config provider unknown",
            Self::UnknownModel => "run config model unknown",
            Self::RunAlreadyActive => "run already active",
            Self::InvalidWorkspacePromptPackJson => "workspace prompt pack JSON invalid",
            Self::InvalidWorkspaceToolCatalogJson => "workspace tool catalog JSON invalid",
            Self::MissingWorkspacePromptPackBytes => {
                "workspace prompt pack bytes missing for validation"
            }
            Self::MissingWorkspaceToolCatalogBytes => {
                "workspace tool catalog bytes missing for validation"
            }
            Self::TooManyPendingIntents => "too many pending intents",
        }
    }
}

impl core::fmt::Display for SessionReduceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::error::Error for SessionReduceError {}

pub fn apply_session_workflow_event(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
) -> Result<SessionReduceOutput, SessionReduceError> {
    apply_session_workflow_event_with_catalog_and_limits(
        state,
        event,
        &[],
        &[],
        SessionRuntimeLimits::default(),
    )
}

pub fn apply_session_workflow_event_with_catalog(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<SessionReduceOutput, SessionReduceError> {
    apply_session_workflow_event_with_catalog_and_limits(
        state,
        event,
        allowed_providers,
        allowed_models,
        SessionRuntimeLimits::default(),
    )
}

pub fn apply_session_workflow_event_with_catalog_and_limits(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
    limits: SessionRuntimeLimits,
) -> Result<SessionReduceOutput, SessionReduceError> {
    stamp_timestamps(state, event);

    let mut out = SessionReduceOutput::default();
    match event {
        SessionWorkflowEvent::Ingress(ingress) => {
            if state.session_id.0.is_empty() {
                state.session_id = ingress.session_id.clone();
            }
            match &ingress.ingress {
                SessionIngressKind::RunRequested {
                    input_ref,
                    run_overrides,
                } => {
                    validate_run_request_catalog(
                        state,
                        run_overrides.as_ref(),
                        allowed_providers,
                        allowed_models,
                    )?;
                    on_run_requested(state, input_ref, run_overrides.as_ref(), &mut out)?;
                }
                SessionIngressKind::HostCommandReceived(command) => {
                    on_host_command(state, command)?
                }
                SessionIngressKind::WorkspaceSyncRequested {
                    workspace_binding,
                    prompt_pack,
                    tool_catalog,
                } => on_workspace_sync_requested(
                    state,
                    workspace_binding,
                    prompt_pack.as_ref(),
                    tool_catalog.as_ref(),
                ),
                SessionIngressKind::WorkspaceSyncUnchanged { workspace, version } => {
                    on_workspace_sync_unchanged(state, workspace, *version)
                }
                SessionIngressKind::WorkspaceSnapshotReady(ready) => on_workspace_snapshot_ready(
                    state,
                    &ready.snapshot,
                    ready.prompt_pack_bytes.as_deref(),
                    ready.tool_catalog_bytes.as_deref(),
                )?,
                SessionIngressKind::WorkspaceSyncFailed { .. } => {}
                SessionIngressKind::WorkspaceApplyRequested { mode } => {
                    on_workspace_apply_requested(state, *mode)
                }
                SessionIngressKind::ToolBatchStarted {
                    tool_batch_id,
                    intent_id,
                    params_hash,
                    expected_call_ids,
                } => on_tool_batch_started(
                    state,
                    tool_batch_id,
                    intent_id,
                    params_hash.as_ref(),
                    expected_call_ids,
                )?,
                SessionIngressKind::ToolCallSettled {
                    tool_batch_id,
                    call_id,
                    status,
                } => on_tool_call_settled(state, tool_batch_id, call_id, status)?,
                SessionIngressKind::ToolBatchSettled {
                    tool_batch_id,
                    results_ref,
                } => on_tool_batch_settled(state, tool_batch_id, results_ref.clone())?,
                SessionIngressKind::ActiveToolBatchReplaced(batch) => {
                    state.active_tool_batch = Some(batch.clone())
                }
                SessionIngressKind::RunCompleted => {
                    transition_lifecycle(state, SessionLifecycle::Completed)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                }
                SessionIngressKind::RunFailed { .. } => {
                    transition_lifecycle(state, SessionLifecycle::Failed)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                }
                SessionIngressKind::RunCancelled { .. } => {
                    transition_lifecycle(state, SessionLifecycle::Cancelled)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                }
                SessionIngressKind::Noop => {}
            }
        }
        SessionWorkflowEvent::Receipt(receipt) => on_receipt_envelope(state, receipt)?,
        SessionWorkflowEvent::ReceiptRejected(rejected) => on_receipt_rejected(state, rejected)?,
        SessionWorkflowEvent::StreamFrame(_frame) => {}
        SessionWorkflowEvent::Noop => {}
    }

    enforce_runtime_limits(state, limits)?;
    Ok(out)
}

pub fn apply_session_event(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
) -> Result<SessionReduceOutput, SessionReduceError> {
    apply_session_workflow_event(state, event)
}

pub fn apply_session_event_with_catalog(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<SessionReduceOutput, SessionReduceError> {
    apply_session_workflow_event_with_catalog(state, event, allowed_providers, allowed_models)
}

pub fn apply_session_event_with_catalog_and_limits(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
    limits: SessionRuntimeLimits,
) -> Result<SessionReduceOutput, SessionReduceError> {
    apply_session_workflow_event_with_catalog_and_limits(
        state,
        event,
        allowed_providers,
        allowed_models,
        limits,
    )
}

pub fn validate_run_request_catalog(
    state: &SessionState,
    run_overrides: Option<&SessionConfig>,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionReduceError> {
    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_catalog(&requested, allowed_providers, allowed_models)
}

pub fn validate_run_catalog(
    config: &RunConfig,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionReduceError> {
    validate_run_config(config)?;

    if !allowed_providers.is_empty()
        && !allowed_providers
            .iter()
            .any(|value| config.provider.trim() == value.trim())
    {
        return Err(SessionReduceError::UnknownProvider);
    }

    if !allowed_models.is_empty()
        && !allowed_models
            .iter()
            .any(|value| config.model.trim() == value.trim())
    {
        return Err(SessionReduceError::UnknownModel);
    }

    Ok(())
}

fn on_run_requested(
    state: &mut SessionState,
    input_ref: &str,
    run_overrides: Option<&SessionConfig>,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if state.active_run_id.is_some() {
        return Err(SessionReduceError::RunAlreadyActive);
    }

    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_config(&requested)?;
    if state.pending_workspace_apply_mode == Some(WorkspaceApplyMode::NextRun) {
        apply_pending_workspace_snapshot(state);
    }

    transition_lifecycle(state, SessionLifecycle::Running)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;

    let run_id = allocate_run_id(state);
    state.active_run_id = Some(run_id.clone());
    state.active_run_config = Some(requested.clone());
    state.active_tool_batch = None;

    let step_ctx = LlmStepContext {
        correlation_id: Some(alloc::format!("run-{}-initial", run_id.run_seq)),
        message_refs: vec![input_ref.into()],
        temperature: None,
        top_p: None,
        tool_refs: None,
        tool_choice: Some(LlmToolChoice::Auto),
        stop_sequences: None,
        metadata: None,
        provider_options_ref: None,
        response_format_ref: None,
        api_key: None,
    };

    let params = materialize_llm_generate_params_with_workspace(
        &requested,
        state.active_workspace_snapshot.as_ref(),
        step_ctx,
    )
    .map_err(|_| SessionReduceError::MissingProvider)?;

    let params_hash = hash_llm_params(&params);
    state.pending_intents.insert(
        params_hash.clone(),
        PendingIntent {
            effect_kind: "llm.generate".into(),
            params_hash: params_hash.clone(),
            intent_id: None,
            cap_slot: Some("llm".into()),
            emitted_at_ns: state.updated_at,
        },
    );
    state.in_flight_effects = state.pending_intents.len() as u64;

    out.effects.push(SessionEffectCommand::LlmGenerate {
        params,
        cap_slot: Some("llm".into()),
        params_hash,
    });

    Ok(())
}

fn on_host_command(
    state: &mut SessionState,
    command: &crate::contracts::HostCommand,
) -> Result<(), SessionReduceError> {
    if !can_apply_host_command(state, &command.command) {
        return Err(SessionReduceError::HostCommandRejected);
    }

    enqueue_host_text(state, &command.command);

    match &command.command {
        HostCommandKind::Pause => {
            transition_lifecycle(state, SessionLifecycle::Paused)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Resume => {
            transition_lifecycle(state, SessionLifecycle::Running)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Cancel { .. } => {
            transition_lifecycle(state, SessionLifecycle::Cancelling)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            transition_lifecycle(state, SessionLifecycle::Cancelled)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
        }
        HostCommandKind::Steer { .. }
        | HostCommandKind::FollowUp { .. }
        | HostCommandKind::Noop => {
            let _ = pop_follow_up_if_ready(state);
        }
    }

    Ok(())
}

fn on_workspace_sync_requested(
    state: &mut SessionState,
    workspace_binding: &WorkspaceBinding,
    prompt_pack: Option<&String>,
    tool_catalog: Option<&String>,
) {
    state.session_config.workspace_binding = Some(workspace_binding.clone());
    state.session_config.default_prompt_pack = prompt_pack.cloned();
    state.session_config.default_tool_catalog = tool_catalog.cloned();
}

fn on_workspace_sync_unchanged(state: &mut SessionState, workspace: &str, version: Option<u64>) {
    if let Some(active) = state.active_workspace_snapshot.as_mut() {
        if active.workspace == workspace {
            active.version = version;
        }
    }
    if let Some(pending) = state.pending_workspace_snapshot.as_mut() {
        if pending.workspace == workspace {
            pending.version = version;
        }
    }
}

fn on_workspace_snapshot_ready(
    state: &mut SessionState,
    snapshot: &WorkspaceSnapshot,
    prompt_pack_bytes: Option<&[u8]>,
    tool_catalog_bytes: Option<&[u8]>,
) -> Result<(), SessionReduceError> {
    validate_workspace_snapshot_json(snapshot, prompt_pack_bytes, tool_catalog_bytes)?;
    state.pending_workspace_snapshot = Some(snapshot.clone());
    if state.pending_workspace_apply_mode == Some(WorkspaceApplyMode::ImmediateIfIdle)
        && state.active_run_id.is_none()
    {
        apply_pending_workspace_snapshot(state);
    }
    Ok(())
}

fn validate_workspace_snapshot_json(
    snapshot: &WorkspaceSnapshot,
    prompt_pack_bytes: Option<&[u8]>,
    tool_catalog_bytes: Option<&[u8]>,
) -> Result<(), SessionReduceError> {
    if snapshot.prompt_pack_ref.is_some() {
        let bytes = prompt_pack_bytes.ok_or(SessionReduceError::MissingWorkspacePromptPackBytes)?;
        validate_prompt_pack_json(bytes)?;
    }
    if snapshot.tool_catalog_ref.is_some() {
        let bytes =
            tool_catalog_bytes.ok_or(SessionReduceError::MissingWorkspaceToolCatalogBytes)?;
        validate_tool_catalog_json(bytes)?;
    }
    Ok(())
}

fn validate_prompt_pack_json(bytes: &[u8]) -> Result<(), SessionReduceError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| SessionReduceError::InvalidWorkspacePromptPackJson)?;

    fn looks_like_message(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Array(items) => items.iter().all(looks_like_message),
            serde_json::Value::Object(obj) => {
                obj.contains_key("role")
                    || obj.contains_key("content")
                    || obj.contains_key("type")
                    || obj.contains_key("tool_calls")
                    || obj.contains_key("output")
                    || obj.contains_key("tool_call_id")
                    || obj.contains_key("call_id")
            }
            _ => false,
        }
    }

    if looks_like_message(&value) {
        Ok(())
    } else {
        Err(SessionReduceError::InvalidWorkspacePromptPackJson)
    }
}

fn validate_tool_catalog_json(bytes: &[u8]) -> Result<(), SessionReduceError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| SessionReduceError::InvalidWorkspaceToolCatalogJson)?;

    fn looks_like_tool_def(value: &serde_json::Value) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };
        if obj
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            return true;
        }
        obj.get("function")
            .and_then(serde_json::Value::as_object)
            .and_then(|function| function.get("name"))
            .and_then(serde_json::Value::as_str)
            .is_some()
    }

    match value {
        serde_json::Value::Array(items) => {
            if items.iter().all(looks_like_tool_def) {
                Ok(())
            } else {
                Err(SessionReduceError::InvalidWorkspaceToolCatalogJson)
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(items) = obj.get("tools").and_then(serde_json::Value::as_array) {
                if items.iter().all(looks_like_tool_def) {
                    return Ok(());
                }
                return Err(SessionReduceError::InvalidWorkspaceToolCatalogJson);
            }
            if obj.contains_key("tool_choice") {
                return Ok(());
            }
            if looks_like_tool_def(&serde_json::Value::Object(obj)) {
                return Ok(());
            }
            Err(SessionReduceError::InvalidWorkspaceToolCatalogJson)
        }
        _ => Err(SessionReduceError::InvalidWorkspaceToolCatalogJson),
    }
}

fn on_workspace_apply_requested(state: &mut SessionState, mode: WorkspaceApplyMode) {
    match mode {
        WorkspaceApplyMode::ImmediateIfIdle => {
            state.pending_workspace_apply_mode = Some(WorkspaceApplyMode::ImmediateIfIdle);
            if state.active_run_id.is_none() {
                apply_pending_workspace_snapshot(state);
            }
        }
        WorkspaceApplyMode::NextRun => {
            state.pending_workspace_apply_mode = Some(WorkspaceApplyMode::NextRun);
        }
    }
}

fn apply_pending_workspace_snapshot(state: &mut SessionState) -> bool {
    let Some(snapshot) = state.pending_workspace_snapshot.take() else {
        return false;
    };
    state.active_workspace_snapshot = Some(snapshot);
    state.pending_workspace_apply_mode = None;
    true
}

fn on_tool_batch_started(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    intent_id: &str,
    params_hash: Option<&String>,
    expected_call_ids: &[String],
) -> Result<(), SessionReduceError> {
    if state
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| !batch.is_settled())
    {
        return Err(SessionReduceError::ToolBatchAlreadyActive);
    }

    let expected_set: BTreeSet<String> = expected_call_ids.iter().cloned().collect();
    let mut call_status = BTreeMap::new();
    for call_id in &expected_set {
        call_status.insert(call_id.clone(), ToolCallStatus::Pending);
    }

    state.active_tool_batch = Some(ActiveToolBatch {
        tool_batch_id: tool_batch_id.clone(),
        intent_id: intent_id.into(),
        params_hash: params_hash.cloned(),
        expected_call_ids: expected_set,
        call_status,
        results_ref: None,
    });
    Ok(())
}

fn on_tool_call_settled(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &str,
    status: &ToolCallStatus,
) -> Result<(), SessionReduceError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(SessionReduceError::ToolBatchNotActive)?;
    if batch.tool_batch_id != *tool_batch_id {
        return Err(SessionReduceError::ToolBatchIdMismatch);
    }
    if !batch.expected_call_ids.contains(call_id) {
        return Err(SessionReduceError::ToolCallUnknown);
    }

    batch.call_status.insert(call_id.into(), status.clone());
    Ok(())
}

fn on_tool_batch_settled(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    results_ref: Option<String>,
) -> Result<(), SessionReduceError> {
    let batch = state
        .active_tool_batch
        .as_mut()
        .ok_or(SessionReduceError::ToolBatchNotActive)?;
    if batch.tool_batch_id != *tool_batch_id {
        return Err(SessionReduceError::ToolBatchIdMismatch);
    }
    if !batch.is_settled() {
        return Err(SessionReduceError::ToolBatchNotSettled);
    }
    batch.results_ref = results_ref;
    Ok(())
}

fn on_receipt_envelope(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
) -> Result<(), SessionReduceError> {
    if let Some(params_hash) = &envelope.params_hash {
        if let Some(mut intent) = state.pending_intents.remove(params_hash) {
            intent.intent_id = Some(envelope.intent_id.clone());
        }
    } else if let Some((key, _intent)) = state
        .pending_intents
        .iter()
        .find(|(_, pending)| pending.intent_id.as_deref() == Some(envelope.intent_id.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
    {
        state.pending_intents.remove(&key);
    }

    state.in_flight_effects = state.pending_intents.len() as u64;

    if envelope.status == "ok" {
        if matches!(state.lifecycle, SessionLifecycle::Running) {
            transition_lifecycle(state, SessionLifecycle::WaitingInput)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
    } else {
        transition_lifecycle(state, SessionLifecycle::Failed)
            .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        clear_active_run(state);
    }

    Ok(())
}

fn on_receipt_rejected(
    state: &mut SessionState,
    rejected: &crate::contracts::EffectReceiptRejected,
) -> Result<(), SessionReduceError> {
    if let Some(params_hash) = &rejected.params_hash {
        state.pending_intents.remove(params_hash);
    } else if let Some((key, _intent)) = state
        .pending_intents
        .iter()
        .find(|(_, pending)| pending.intent_id.as_deref() == Some(rejected.intent_id.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
    {
        state.pending_intents.remove(&key);
    }

    state.in_flight_effects = state.pending_intents.len() as u64;

    transition_lifecycle(state, SessionLifecycle::Failed)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    Ok(())
}

fn select_run_config(session: &SessionConfig, override_cfg: Option<&SessionConfig>) -> RunConfig {
    let source = override_cfg.unwrap_or(session);
    RunConfig {
        provider: source.provider.clone(),
        model: source.model.clone(),
        reasoning_effort: source.reasoning_effort,
        max_tokens: source.max_tokens,
        workspace_binding: source.workspace_binding.clone(),
        prompt_pack: source.default_prompt_pack.clone(),
        prompt_refs: source.default_prompt_refs.clone(),
        tool_catalog: source.default_tool_catalog.clone(),
        tool_refs: source.default_tool_refs.clone(),
    }
}

fn validate_run_config(config: &RunConfig) -> Result<(), SessionReduceError> {
    if config.provider.trim().is_empty() {
        return Err(SessionReduceError::MissingProvider);
    }
    if config.model.trim().is_empty() {
        return Err(SessionReduceError::MissingModel);
    }
    Ok(())
}

fn clear_active_run(state: &mut SessionState) {
    state.active_run_id = None;
    state.active_run_config = None;
    state.active_tool_batch = None;
    state.pending_intents.clear();
    state.in_flight_effects = 0;
}

fn enforce_runtime_limits(
    state: &SessionState,
    limits: SessionRuntimeLimits,
) -> Result<(), SessionReduceError> {
    if let Some(max) = limits.max_pending_intents {
        if state.pending_intents.len() as u64 > max {
            return Err(SessionReduceError::TooManyPendingIntents);
        }
    }
    Ok(())
}

fn stamp_timestamps(state: &mut SessionState, event: &SessionWorkflowEvent) {
    let ts = match event {
        SessionWorkflowEvent::Ingress(ingress) => ingress.observed_at_ns,
        SessionWorkflowEvent::Receipt(receipt) => receipt.emitted_at_seq,
        SessionWorkflowEvent::ReceiptRejected(rejected) => rejected.emitted_at_seq,
        SessionWorkflowEvent::StreamFrame(frame) => frame.emitted_at_seq,
        SessionWorkflowEvent::Noop => state.updated_at,
    };

    if state.created_at == 0 {
        state.created_at = ts;
    }
    state.updated_at = ts;
}

fn hash_llm_params(params: &SysLlmGenerateParams) -> String {
    let bytes = serde_cbor::to_vec(params).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        let hi = byte >> 4;
        let lo = byte & 0x0f;
        out.push(nibble_to_hex(hi));
        out.push(nibble_to_hex(lo));
    }
    out
}

fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{SessionId, SessionIngress};

    fn run_request_event() -> SessionWorkflowEvent {
        SessionWorkflowEvent::Ingress(SessionIngress {
            session_id: SessionId("s-1".into()),
            observed_at_ns: 1,
            ingress: SessionIngressKind::RunRequested {
                input_ref:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                run_overrides: Some(SessionConfig {
                    provider: "openai".into(),
                    model: "gpt-5.2".into(),
                    reasoning_effort: None,
                    max_tokens: Some(512),
                    workspace_binding: None,
                    default_prompt_pack: None,
                    default_prompt_refs: None,
                    default_tool_catalog: None,
                    default_tool_refs: None,
                }),
            },
        })
    }

    #[test]
    fn run_request_emits_llm_effect_and_tracks_pending_intent() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event()).expect("reduce");
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert_eq!(out.effects.len(), 1);
        assert_eq!(state.pending_intents.len(), 1);
        assert_eq!(state.in_flight_effects, 1);
    }

    #[test]
    fn receipt_ok_moves_running_to_waiting_input() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        let out = apply_session_workflow_event(&mut state, &run_request_event()).expect("reduce");
        let params_hash = match &out.effects[0] {
            SessionEffectCommand::LlmGenerate { params_hash, .. } => params_hash.clone(),
        };

        let receipt = aos_wasm_sdk::EffectReceiptEnvelope {
            origin_module_id: "demo/Session@1".into(),
            origin_instance_key: None,
            intent_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .into(),
            effect_kind: "llm.generate".into(),
            params_hash: Some(params_hash),
            receipt_payload: vec![],
            status: "ok".into(),
            emitted_at_seq: 2,
            adapter_id: "llm".into(),
            cost_cents: None,
            signature: vec![],
        };

        let event = SessionWorkflowEvent::Receipt(receipt);
        apply_session_workflow_event(&mut state, &event).expect("receipt");
        assert_eq!(state.lifecycle, SessionLifecycle::WaitingInput);
        assert_eq!(state.pending_intents.len(), 0);
    }
}
