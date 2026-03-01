use super::{
    allocate_run_id, allocate_tool_batch_id, can_apply_host_command, enqueue_host_text,
    pop_follow_up_if_ready, transition_lifecycle,
};
use crate::contracts::{
    ActiveToolBatch, EffectiveTool, EffectiveToolSet, HostCommandKind, PendingIntent,
    PlannedToolCall, RunConfig, SessionConfig, SessionIngressKind, SessionLifecycle, SessionState,
    SessionWorkflowEvent, ToolAvailabilityRule, ToolBatchPlan, ToolCallObserved, ToolCallStatus,
    ToolExecutionPlan, ToolOverrideScope, ToolSpec, WorkspaceApplyMode, WorkspaceBinding,
    WorkspaceSnapshot, default_tool_profile_for_provider,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use super::llm::{
    LlmMappingError, LlmStepContext, LlmToolChoice, SysLlmGenerateParams,
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
    RunNotActive,
    InvalidWorkspacePromptPackJson,
    MissingWorkspacePromptPackBytes,
    TooManyPendingIntents,
    ToolProfileUnknown,
    UnknownToolOverride,
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
            Self::RunNotActive => "run not active",
            Self::InvalidWorkspacePromptPackJson => "workspace prompt pack JSON invalid",
            Self::MissingWorkspacePromptPackBytes => {
                "workspace prompt pack bytes missing for validation"
            }
            Self::TooManyPendingIntents => "too many pending intents",
            Self::ToolProfileUnknown => "tool profile unknown",
            Self::UnknownToolOverride => "unknown tool override",
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
                } => on_workspace_sync_requested(state, workspace_binding, prompt_pack.as_ref()),
                SessionIngressKind::WorkspaceSyncUnchanged { workspace, version } => {
                    on_workspace_sync_unchanged(state, workspace, *version)
                }
                SessionIngressKind::WorkspaceSnapshotReady(ready) => on_workspace_snapshot_ready(
                    state,
                    &ready.snapshot,
                    ready.prompt_pack_bytes.as_deref(),
                )?,
                SessionIngressKind::WorkspaceSyncFailed { .. } => {}
                SessionIngressKind::WorkspaceApplyRequested { mode } => {
                    on_workspace_apply_requested(state, *mode)
                }
                SessionIngressKind::ToolRegistrySet {
                    registry,
                    profiles,
                    default_profile,
                } => on_tool_registry_set(
                    state,
                    registry,
                    profiles.as_ref(),
                    default_profile.as_ref(),
                )?,
                SessionIngressKind::ToolProfileSelected { profile_id } => {
                    on_tool_profile_selected(state, profile_id)?
                }
                SessionIngressKind::ToolOverridesSet {
                    scope,
                    enable,
                    disable,
                    force,
                } => on_tool_overrides_set(
                    state,
                    *scope,
                    enable.as_deref(),
                    disable.as_deref(),
                    force.as_deref(),
                )?,
                SessionIngressKind::HostSessionUpdated {
                    host_session_id,
                    host_session_status,
                } => {
                    on_host_session_updated(state, host_session_id.as_ref(), *host_session_status)?
                }
                SessionIngressKind::ToolCallsObserved {
                    intent_id,
                    params_hash,
                    calls,
                } => on_tool_calls_observed(state, intent_id, params_hash.as_ref(), calls)?,
                SessionIngressKind::ToolCallSettled {
                    tool_batch_id,
                    call_id,
                    status,
                } => on_tool_call_settled(state, tool_batch_id, call_id, status)?,
                SessionIngressKind::ToolBatchSettled {
                    tool_batch_id,
                    results_ref,
                } => on_tool_batch_settled(state, tool_batch_id, results_ref.clone())?,
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

    refresh_effective_tools(state, Some(&requested))?;

    let step_ctx = LlmStepContext {
        correlation_id: Some(alloc::format!("run-{}-initial", run_id.run_seq)),
        message_refs: vec![input_ref.into()],
        temperature: None,
        top_p: None,
        tool_refs: state.effective_tools.tool_refs(),
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
    .map_err(map_llm_mapping_error)?;

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

fn map_llm_mapping_error(err: LlmMappingError) -> SessionReduceError {
    match err {
        LlmMappingError::MissingProvider => SessionReduceError::MissingProvider,
        LlmMappingError::MissingModel => SessionReduceError::MissingModel,
        LlmMappingError::EmptyMessageRefs => SessionReduceError::InvalidWorkspacePromptPackJson,
    }
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
) {
    state.session_config.workspace_binding = Some(workspace_binding.clone());
    state.session_config.default_prompt_pack = prompt_pack.cloned();
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
) -> Result<(), SessionReduceError> {
    validate_workspace_snapshot_json(snapshot, prompt_pack_bytes)?;
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
) -> Result<(), SessionReduceError> {
    if snapshot.prompt_pack_ref.is_some() {
        let bytes = prompt_pack_bytes.ok_or(SessionReduceError::MissingWorkspacePromptPackBytes)?;
        validate_prompt_pack_json(bytes)?;
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

fn on_tool_registry_set(
    state: &mut SessionState,
    registry: &BTreeMap<String, ToolSpec>,
    profiles: Option<&BTreeMap<String, Vec<String>>>,
    default_profile: Option<&String>,
) -> Result<(), SessionReduceError> {
    state.tool_registry = registry.clone();
    if let Some(profiles) = profiles {
        state.tool_profiles = profiles.clone();
    }
    if let Some(default_profile) = default_profile {
        state.tool_profile = default_profile.clone();
    }

    let active = state.active_run_config.clone();
    refresh_effective_tools(state, active.as_ref())
}

fn on_tool_profile_selected(
    state: &mut SessionState,
    profile_id: &str,
) -> Result<(), SessionReduceError> {
    if !state.tool_profiles.contains_key(profile_id) {
        return Err(SessionReduceError::ToolProfileUnknown);
    }
    state.tool_profile = profile_id.into();
    let active = state.active_run_config.clone();
    refresh_effective_tools(state, active.as_ref())
}

fn on_tool_overrides_set(
    state: &mut SessionState,
    scope: ToolOverrideScope,
    enable: Option<&[String]>,
    disable: Option<&[String]>,
    force: Option<&[String]>,
) -> Result<(), SessionReduceError> {
    validate_known_tool_names(state, enable)?;
    validate_known_tool_names(state, disable)?;
    validate_known_tool_names(state, force)?;

    match scope {
        ToolOverrideScope::Session => {
            state.session_config.default_tool_enable = enable.map(|items| items.to_vec());
            state.session_config.default_tool_disable = disable.map(|items| items.to_vec());
            state.session_config.default_tool_force = force.map(|items| items.to_vec());
        }
        ToolOverrideScope::Run => {
            let active = state
                .active_run_config
                .as_mut()
                .ok_or(SessionReduceError::RunNotActive)?;
            active.tool_enable = enable.map(|items| items.to_vec());
            active.tool_disable = disable.map(|items| items.to_vec());
            active.tool_force = force.map(|items| items.to_vec());
        }
    }

    let active = state.active_run_config.clone();
    refresh_effective_tools(state, active.as_ref())
}

fn on_host_session_updated(
    state: &mut SessionState,
    host_session_id: Option<&String>,
    host_session_status: Option<crate::contracts::HostSessionStatus>,
) -> Result<(), SessionReduceError> {
    state.tool_runtime_context.host_session_id = host_session_id.cloned();
    state.tool_runtime_context.host_session_status = host_session_status;

    let active = state.active_run_config.clone();
    refresh_effective_tools(state, active.as_ref())
}

fn on_tool_calls_observed(
    state: &mut SessionState,
    intent_id: &str,
    params_hash: Option<&String>,
    calls: &[ToolCallObserved],
) -> Result<(), SessionReduceError> {
    if state
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| !batch.is_settled())
    {
        return Err(SessionReduceError::ToolBatchAlreadyActive);
    }

    let run_id = state
        .active_run_id
        .clone()
        .ok_or(SessionReduceError::RunNotActive)?;
    let tool_batch_id = allocate_tool_batch_id(state, &run_id);

    let (plan, call_status) = plan_tool_batch(state, calls);
    state.last_tool_plan_hash = Some(hash_tool_plan(&plan));

    state.active_tool_batch = Some(ActiveToolBatch {
        tool_batch_id,
        intent_id: intent_id.into(),
        params_hash: params_hash.cloned(),
        plan,
        call_status,
        results_ref: None,
    });

    Ok(())
}

fn plan_tool_batch(
    state: &SessionState,
    calls: &[ToolCallObserved],
) -> (ToolBatchPlan, BTreeMap<String, ToolCallStatus>) {
    let mut planned_calls = Vec::with_capacity(calls.len());
    let mut call_status = BTreeMap::new();

    for call in calls {
        if let Some(tool) = state.effective_tools.tool_by_name(&call.tool_name) {
            planned_calls.push(PlannedToolCall {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                arguments_ref: call.arguments_ref.clone(),
                provider_call_id: call.provider_call_id.clone(),
                executor: tool.executor.clone(),
                parallel_safe: tool.parallel_safe,
                resource_key: tool.resource_key.clone(),
                accepted: true,
            });
            call_status.insert(call.call_id.clone(), ToolCallStatus::Pending);
        } else {
            planned_calls.push(PlannedToolCall {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                arguments_ref: call.arguments_ref.clone(),
                provider_call_id: call.provider_call_id.clone(),
                executor: crate::contracts::ToolExecutor::default(),
                parallel_safe: false,
                resource_key: None,
                accepted: false,
            });
            call_status.insert(call.call_id.clone(), ToolCallStatus::Ignored);
        }
    }

    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current_group: Vec<String> = Vec::new();
    let mut current_resources: BTreeSet<String> = BTreeSet::new();

    for call in &planned_calls {
        if !call.accepted {
            continue;
        }

        if !call.parallel_safe {
            flush_group(&mut groups, &mut current_group, &mut current_resources);
            groups.push(vec![call.call_id.clone()]);
            continue;
        }

        if let Some(resource_key) = call.resource_key.as_ref() {
            if current_resources.contains(resource_key) {
                flush_group(&mut groups, &mut current_group, &mut current_resources);
            }
            current_resources.insert(resource_key.clone());
        }

        current_group.push(call.call_id.clone());
    }
    flush_group(&mut groups, &mut current_group, &mut current_resources);

    (
        ToolBatchPlan {
            observed_calls: calls.to_vec(),
            planned_calls,
            execution_plan: ToolExecutionPlan { groups },
        },
        call_status,
    )
}

fn flush_group(
    groups: &mut Vec<Vec<String>>,
    current_group: &mut Vec<String>,
    current_resources: &mut BTreeSet<String>,
) {
    if !current_group.is_empty() {
        groups.push(core::mem::take(current_group));
        current_resources.clear();
    }
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
    if !batch.contains_call(call_id) {
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

fn refresh_effective_tools(
    state: &mut SessionState,
    run_config: Option<&RunConfig>,
) -> Result<(), SessionReduceError> {
    let provider = run_config
        .map(|cfg| cfg.provider.as_str())
        .or_else(|| {
            if state.session_config.provider.trim().is_empty() {
                None
            } else {
                Some(state.session_config.provider.as_str())
            }
        })
        .unwrap_or("openai");

    let profile_id = run_config
        .and_then(|cfg| cfg.tool_profile.clone())
        .or_else(|| state.session_config.default_tool_profile.clone())
        .or_else(|| {
            if state.tool_profile.trim().is_empty() {
                None
            } else {
                Some(state.tool_profile.clone())
            }
        })
        .unwrap_or_else(|| default_tool_profile_for_provider(provider));

    let base_profile = state
        .tool_profiles
        .get(&profile_id)
        .ok_or(SessionReduceError::ToolProfileUnknown)?;

    validate_known_tool_names(state, Some(base_profile.as_slice()))?;

    let enabled_session = state.session_config.default_tool_enable.as_deref();
    let disabled_session = state.session_config.default_tool_disable.as_deref();
    let force_session = state.session_config.default_tool_force.as_deref();

    let enabled_run = run_config.and_then(|cfg| cfg.tool_enable.as_deref());
    let disabled_run = run_config.and_then(|cfg| cfg.tool_disable.as_deref());
    let force_run = run_config.and_then(|cfg| cfg.tool_force.as_deref());

    validate_known_tool_names(state, enabled_session)?;
    validate_known_tool_names(state, disabled_session)?;
    validate_known_tool_names(state, force_session)?;
    validate_known_tool_names(state, enabled_run)?;
    validate_known_tool_names(state, disabled_run)?;
    validate_known_tool_names(state, force_run)?;

    let mut denied = BTreeSet::new();
    for source in [disabled_session, disabled_run] {
        if let Some(items) = source {
            denied.extend(items.iter().cloned());
        }
    }

    let mut enabled = BTreeSet::new();
    enabled.extend(base_profile.iter().cloned());
    for source in [enabled_session, force_session, enabled_run, force_run] {
        if let Some(items) = source {
            enabled.extend(items.iter().cloned());
        }
    }

    for denied_name in denied {
        enabled.remove(&denied_name);
    }

    let mut ordered_names = Vec::new();
    let mut seen = BTreeSet::new();
    for name in base_profile {
        if enabled.contains(name) {
            ordered_names.push(name.clone());
            seen.insert(name.clone());
        }
    }

    let mut extras: Vec<String> = enabled
        .into_iter()
        .filter(|name| !seen.contains(name))
        .collect();
    extras.sort();
    ordered_names.extend(extras);

    let mut ordered_tools = Vec::new();
    for tool_name in ordered_names {
        let Some(spec) = state.tool_registry.get(&tool_name) else {
            return Err(SessionReduceError::UnknownToolOverride);
        };
        if !is_tool_available(spec, &state.tool_runtime_context) {
            continue;
        }
        ordered_tools.push(EffectiveTool {
            tool_name: spec.tool_name.clone(),
            tool_ref: spec.tool_ref.clone(),
            executor: spec.executor.clone(),
            parallel_safe: spec.parallelism_hint.parallel_safe,
            resource_key: spec.parallelism_hint.resource_key.clone(),
        });
    }

    state.tool_profile = profile_id.clone();
    state.effective_tools = EffectiveToolSet {
        profile_id,
        ordered_tools,
    };

    Ok(())
}

fn is_tool_available(spec: &ToolSpec, runtime: &crate::contracts::ToolRuntimeContext) -> bool {
    spec.availability_rules.iter().all(|rule| match rule {
        ToolAvailabilityRule::Always => true,
        ToolAvailabilityRule::HostSessionReady => {
            runtime.host_session_status == Some(crate::contracts::HostSessionStatus::Ready)
        }
    })
}

fn validate_known_tool_names(
    state: &SessionState,
    names: Option<&[String]>,
) -> Result<(), SessionReduceError> {
    if let Some(names) = names {
        for tool_name in names {
            if !state.tool_registry.contains_key(tool_name) {
                return Err(SessionReduceError::UnknownToolOverride);
            }
        }
    }
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
        tool_profile: source.default_tool_profile.clone(),
        tool_enable: source.default_tool_enable.clone(),
        tool_disable: source.default_tool_disable.clone(),
        tool_force: source.default_tool_force.clone(),
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
    hash_cbor(params)
}

fn hash_tool_plan(plan: &ToolBatchPlan) -> String {
    hash_cbor(plan)
}

fn hash_cbor<T: serde::Serialize>(value: &T) -> String {
    let bytes = serde_cbor::to_vec(value).unwrap_or_default();
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
    use crate::contracts::{
        HostSessionStatus, SessionId, SessionIngress, ToolCallObserved, ToolOverrideScope,
    };

    fn fake_hash(ch: char) -> String {
        let mut out = String::from("sha256:");
        for _ in 0..64 {
            out.push(ch);
        }
        out
    }

    fn ingress(observed_at_ns: u64, ingress: SessionIngressKind) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Ingress(SessionIngress {
            session_id: SessionId("s-1".into()),
            observed_at_ns,
            ingress,
        })
    }

    fn run_request_event(ts: u64) -> SessionWorkflowEvent {
        ingress(
            ts,
            SessionIngressKind::RunRequested {
                input_ref: fake_hash('a'),
                run_overrides: Some(SessionConfig {
                    provider: "openai".into(),
                    model: "gpt-5.2".into(),
                    reasoning_effort: None,
                    max_tokens: Some(512),
                    workspace_binding: None,
                    default_prompt_pack: None,
                    default_prompt_refs: None,
                    default_tool_profile: None,
                    default_tool_enable: None,
                    default_tool_disable: None,
                    default_tool_force: None,
                }),
            },
        )
    }

    #[test]
    fn run_request_emits_llm_effect_and_tracks_pending_intent() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert_eq!(out.effects.len(), 1);
        assert_eq!(state.pending_intents.len(), 1);
        assert_eq!(state.in_flight_effects, 1);
        assert_eq!(state.effective_tools.profile_id, "openai");
        assert_eq!(
            state
                .effective_tools
                .ordered_tools
                .iter()
                .map(|tool| tool.tool_name.as_str())
                .collect::<Vec<_>>(),
            vec!["host.session.open"]
        );
    }

    #[test]
    fn host_session_ready_enables_host_fs_and_exec_tools() {
        let mut state = SessionState::default();
        apply_session_workflow_event(
            &mut state,
            &ingress(
                1,
                SessionIngressKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");

        apply_session_workflow_event(&mut state, &run_request_event(2)).expect("run");

        let tools: Vec<&str> = state
            .effective_tools
            .ordered_tools
            .iter()
            .map(|tool| tool.tool_name.as_str())
            .collect();
        assert!(tools.contains(&"host.exec"));
        assert!(tools.contains(&"host.fs.apply_patch"));
    }

    #[test]
    fn tool_calls_observed_builds_deterministic_plan_and_ignores_disabled() {
        let mut state = SessionState::default();
        apply_session_workflow_event(
            &mut state,
            &ingress(
                1,
                SessionIngressKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");
        apply_session_workflow_event(&mut state, &run_request_event(2)).expect("run");

        // Deny exec so it gets ignored even when host session is ready.
        apply_session_workflow_event(
            &mut state,
            &ingress(
                3,
                SessionIngressKind::ToolOverridesSet {
                    scope: ToolOverrideScope::Run,
                    enable: None,
                    disable: Some(vec!["host.exec".into()]),
                    force: None,
                },
            ),
        )
        .expect("overrides");

        let calls = vec![
            ToolCallObserved {
                call_id: "c1".into(),
                tool_name: "host.fs.write_file".into(),
                arguments_ref: fake_hash('w'),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c2".into(),
                tool_name: "host.fs.apply_patch".into(),
                arguments_ref: fake_hash('p'),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c3".into(),
                tool_name: "host.exec".into(),
                arguments_ref: fake_hash('e'),
                provider_call_id: None,
            },
        ];

        apply_session_workflow_event(
            &mut state,
            &ingress(
                4,
                SessionIngressKind::ToolCallsObserved {
                    intent_id: fake_hash('i'),
                    params_hash: Some(fake_hash('h')),
                    calls,
                },
            ),
        )
        .expect("plan");

        let batch = state.active_tool_batch.as_ref().expect("active batch");
        assert_eq!(
            batch.plan.execution_plan.groups,
            vec![
                vec![String::from("c1")],
                vec![String::from("c2")]
            ]
        );
        assert!(matches!(
            batch.call_status.get("c3"),
            Some(ToolCallStatus::Ignored)
        ));
    }
}
