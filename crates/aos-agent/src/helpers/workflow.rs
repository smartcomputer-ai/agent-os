use super::{
    allocate_run_id, allocate_tool_batch_id, can_apply_host_command, enqueue_host_text,
    pop_follow_up_if_ready, transition_lifecycle,
};
use crate::contracts::{
    ActiveToolBatch, EffectiveTool, EffectiveToolSet, HostCommandKind, PendingBlobGet,
    PendingBlobGetKind, PendingBlobPut, PendingBlobPutKind, PendingFollowUpTurn, PendingIntent,
    PlannedToolCall, RunConfig, SessionConfig, SessionIngressKind, SessionLifecycle, SessionState,
    SessionWorkflowEvent, ToolAvailabilityRule, ToolBatchPlan, ToolCallObserved, ToolCallStatus,
    ToolExecutionPlan, ToolExecutor, ToolOverrideScope, ToolSpec, WorkspaceApplyMode,
    WorkspaceBinding, WorkspaceSnapshot, default_tool_profile_for_provider,
};
use crate::tools::{
    ToolEffectKind, map_tool_arguments_to_effect_params, map_tool_receipt_to_llm_result,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use aos_air_types::HashRef;
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, LlmGenerateReceipt,
    LlmOutputEnvelope, LlmToolCallList,
};
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
    ToolEffect {
        kind: ToolEffectKind,
        params_json: String,
        cap_slot: Option<String>,
        params_hash: String,
    },
    BlobPut {
        params: BlobPutParams,
        cap_slot: Option<String>,
        params_hash: String,
    },
    BlobGet {
        params: BlobGetParams,
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
        SessionWorkflowEvent::Receipt(receipt) => on_receipt_envelope(state, receipt, &mut out)?,
        SessionWorkflowEvent::ReceiptRejected(rejected) => {
            on_receipt_rejected(state, rejected, &mut out)?
        }
        SessionWorkflowEvent::StreamFrame(_frame) => {}
        SessionWorkflowEvent::Noop => {}
    }

    recompute_in_flight_effects(state);
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
    state.pending_blob_gets.clear();
    state.pending_blob_puts.clear();
    state.pending_follow_up_turn = None;
    state.queued_llm_message_refs = None;
    state.conversation_message_refs.clear();

    refresh_effective_tools(state, Some(&requested))?;
    state.conversation_message_refs.push(input_ref.into());
    queue_llm_turn(state, state.conversation_message_refs.clone(), out)
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
    out: &mut SessionReduceOutput,
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
        pending_by_params_hash: BTreeMap::new(),
        next_group_index: 0,
        llm_results: BTreeMap::new(),
        results_ref: None,
    });

    dispatch_next_ready_tool_group(state, out)?;
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
                arguments_json: call.arguments_json.clone(),
                arguments_ref: call.arguments_ref.clone(),
                provider_call_id: call.provider_call_id.clone(),
                mapper: tool.mapper,
                executor: tool.executor.clone(),
                parallel_safe: tool.parallel_safe,
                resource_key: tool.resource_key.clone(),
                accepted: true,
            });
            call_status.insert(call.call_id.clone(), ToolCallStatus::Queued);
        } else {
            planned_calls.push(PlannedToolCall {
                call_id: call.call_id.clone(),
                tool_name: call.tool_name.clone(),
                arguments_json: call.arguments_json.clone(),
                arguments_ref: call.arguments_ref.clone(),
                provider_call_id: call.provider_call_id.clone(),
                mapper: crate::contracts::ToolMapper::HostSessionOpen,
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

fn dispatch_next_ready_tool_group(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    loop {
        let Some(mut batch) = state.active_tool_batch.take() else {
            return Ok(());
        };
        let idx = batch.next_group_index as usize;
        if idx >= batch.plan.execution_plan.groups.len() {
            if batch.is_settled() && batch.results_ref.is_none() {
                start_follow_up_for_settled_batch(state, &mut batch, out)?;
            }
            state.active_tool_batch = Some(batch);
            recompute_in_flight_effects(state);
            return Ok(());
        }

        let previous_groups_settled =
            batch
                .plan
                .execution_plan
                .groups
                .iter()
                .take(idx)
                .all(|group| {
                    group.iter().all(|call_id| {
                        batch
                            .call_status
                            .get(call_id)
                            .is_some_and(ToolCallStatus::is_terminal)
                    })
                });
        if !previous_groups_settled {
            state.active_tool_batch = Some(batch);
            recompute_in_flight_effects(state);
            return Ok(());
        }

        let group = batch.plan.execution_plan.groups[idx].clone();
        batch.next_group_index = batch.next_group_index.saturating_add(1);

        let runtime_ctx = state.tool_runtime_context.clone();
        let mut emitted_for_group = 0usize;
        for call_id in group {
            let Some(status) = batch.call_status.get(&call_id).cloned() else {
                continue;
            };
            if status != ToolCallStatus::Queued {
                continue;
            }

            let Some(planned) = batch
                .plan
                .planned_calls
                .iter()
                .find(|call| call.call_id == call_id)
                .cloned()
            else {
                continue;
            };

            match &planned.executor {
                ToolExecutor::HostLoop { .. } => {
                    batch
                        .call_status
                        .insert(call_id.clone(), ToolCallStatus::Pending);
                    continue;
                }
                ToolExecutor::Effect { .. } => {}
            }

            let (effect_kind, cap_slot) = match &planned.executor {
                ToolExecutor::Effect {
                    effect_kind,
                    cap_slot,
                } => (effect_kind.clone(), cap_slot.clone()),
                ToolExecutor::HostLoop { .. } => unreachable!(),
            };
            let kind = if let Some(mapper) =
                crate::tools::mapper_for_effect_kind(effect_kind.as_str())
            {
                crate::tools::effect_kind_for_mapper(mapper)
            } else {
                batch.call_status.insert(
                    call_id.clone(),
                    ToolCallStatus::Failed {
                        code: "executor_unsupported".into(),
                        detail: format!("unsupported effect kind for wasm emit_raw: {effect_kind}"),
                    },
                );
                continue;
            };

            let arguments_json = if !planned.arguments_json.trim().is_empty() {
                planned.arguments_json.clone()
            } else if let Some(arguments_ref) = planned.arguments_ref.clone() {
                let blob_ref = match HashRef::new(arguments_ref) {
                    Ok(value) => value,
                    Err(err) => {
                        batch.call_status.insert(
                            call_id.clone(),
                            ToolCallStatus::Failed {
                                code: "tool_invalid_args_ref".into(),
                                detail: format!(
                                    "invalid arguments_ref for {}: {err}",
                                    planned.tool_name
                                ),
                            },
                        );
                        continue;
                    }
                };
                let blob_get = BlobGetParams { blob_ref };
                let blob_get_hash = hash_cbor(&blob_get);
                let already_pending = state.pending_blob_gets.contains_key(&blob_get_hash);
                state
                    .pending_blob_gets
                    .entry(blob_get_hash.clone())
                    .or_default()
                    .push(PendingBlobGet {
                        kind: PendingBlobGetKind::ToolCallArguments {
                            tool_batch_id: batch.tool_batch_id.clone(),
                            call_id: call_id.clone(),
                        },
                        emitted_at_ns: state.updated_at,
                    });
                if !already_pending
                    && !out.effects.iter().any(|effect| {
                        matches!(
                            effect,
                            SessionEffectCommand::BlobGet { params_hash, .. }
                                if params_hash == &blob_get_hash
                        )
                    })
                {
                    out.effects.push(SessionEffectCommand::BlobGet {
                        params: blob_get,
                        cap_slot: Some("blob".into()),
                        params_hash: blob_get_hash,
                    });
                    emitted_for_group = emitted_for_group.saturating_add(1);
                }
                batch
                    .call_status
                    .insert(call_id.clone(), ToolCallStatus::Pending);
                continue;
            } else {
                batch.call_status.insert(
                    call_id.clone(),
                    ToolCallStatus::Failed {
                        code: "tool_invalid_args".into(),
                        detail: format!(
                            "tool {} missing arguments_json and arguments_ref",
                            planned.tool_name
                        ),
                    },
                );
                continue;
            };

            let params_json = match map_tool_arguments_to_effect_params(
                planned.mapper,
                arguments_json.as_str(),
                &runtime_ctx,
            ) {
                Ok(params) => params,
                Err(err) => {
                    batch
                        .call_status
                        .insert(call_id.clone(), err.to_failed_status());
                    batch.llm_results.insert(
                        call_id.clone(),
                        crate::contracts::ToolCallLlmResult {
                            call_id: call_id.clone(),
                            tool_name: planned.tool_name.clone(),
                            is_error: true,
                            output_json: format!(
                                "{{\"ok\":false,\"error\":\"{}\",\"detail\":{}}}",
                                err.to_code_text(),
                                serde_json::to_string(&err.detail)
                                    .unwrap_or_else(|_| "\"\"".into())
                            ),
                        },
                    );
                    continue;
                }
            };

            let params_hash = hash_cbor(&params_json);
            batch
                .pending_by_params_hash
                .entry(params_hash.clone())
                .or_default()
                .push(call_id.clone());
            batch
                .call_status
                .insert(call_id.clone(), ToolCallStatus::Pending);
            emitted_for_group = emitted_for_group.saturating_add(1);

            out.effects.push(SessionEffectCommand::ToolEffect {
                kind,
                params_json: serde_json::to_string(&params_json).unwrap_or_else(|_| "{}".into()),
                cap_slot,
                params_hash,
            });
        }

        state.active_tool_batch = Some(batch);
        recompute_in_flight_effects(state);
        if emitted_for_group > 0 {
            return Ok(());
        }
    }
}

fn settle_tool_call_from_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(batch) = state.active_tool_batch.as_mut() else {
        return Ok(false);
    };
    let Some(params_hash) = envelope.params_hash.as_ref() else {
        return Ok(false);
    };
    let Some(call_ids) = batch.pending_by_params_hash.get_mut(params_hash) else {
        return Ok(false);
    };
    if call_ids.is_empty() {
        return Ok(false);
    }
    let call_id = call_ids.remove(0);
    let remove_entry = call_ids.is_empty();
    if remove_entry {
        batch.pending_by_params_hash.remove(params_hash);
    }

    let Some(planned) = batch
        .plan
        .planned_calls
        .iter()
        .find(|call| call.call_id == call_id)
        .cloned()
    else {
        return Ok(false);
    };

    let mapped = map_tool_receipt_to_llm_result(
        planned.mapper,
        planned.tool_name.as_str(),
        envelope.status.as_str(),
        envelope.receipt_payload.as_slice(),
    );
    batch
        .call_status
        .insert(call_id.clone(), mapped.status.clone());
    batch.llm_results.insert(
        call_id.clone(),
        crate::contracts::ToolCallLlmResult {
            call_id: call_id.clone(),
            tool_name: planned.tool_name,
            is_error: mapped.is_error,
            output_json: mapped.llm_output_json,
        },
    );
    if let Some(host_session_id) = mapped.runtime_delta.host_session_id {
        state.tool_runtime_context.host_session_id = Some(host_session_id);
    }
    if let Some(host_session_status) = mapped.runtime_delta.host_session_status {
        state.tool_runtime_context.host_session_status = Some(host_session_status);
    }

    dispatch_next_ready_tool_group(state, out)?;
    Ok(true)
}

fn on_receipt_envelope(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if handle_pending_blob_get_receipt(state, envelope, out)? {
        recompute_in_flight_effects(state);
        return Ok(());
    }
    if handle_pending_blob_put_receipt(state, envelope, out)? {
        recompute_in_flight_effects(state);
        return Ok(());
    }
    if settle_tool_call_from_receipt(state, envelope, out)? {
        recompute_in_flight_effects(state);
        return Ok(());
    }

    remove_pending_intent_for_receipt(
        state,
        envelope.params_hash.as_ref(),
        envelope.intent_id.as_str(),
    );

    if envelope.effect_kind == "llm.generate" {
        if envelope.status != "ok" {
            transition_lifecycle(state, SessionLifecycle::Failed)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
            recompute_in_flight_effects(state);
            return Ok(());
        }

        let parsed = serde_cbor::from_slice::<LlmGenerateReceipt>(&envelope.receipt_payload);
        let Ok(receipt) = parsed else {
            transition_lifecycle(state, SessionLifecycle::Failed)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
            recompute_in_flight_effects(state);
            return Ok(());
        };
        if let Err(_err) = enqueue_blob_get(
            state,
            receipt.output_ref,
            PendingBlobGetKind::LlmOutputEnvelope,
            out,
        ) {
            transition_lifecycle(state, SessionLifecycle::Failed)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            clear_active_run(state);
            recompute_in_flight_effects(state);
            return Ok(());
        }
    }

    recompute_in_flight_effects(state);
    Ok(())
}

fn on_receipt_rejected(
    state: &mut SessionState,
    rejected: &crate::contracts::EffectReceiptRejected,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    let payload = serde_json::json!({
        "status": "error",
        "error_code": rejected.error_code,
        "error_message": rejected.error_message,
    });
    let envelope = aos_wasm_sdk::EffectReceiptEnvelope {
        origin_module_id: rejected.origin_module_id.clone(),
        origin_instance_key: rejected.origin_instance_key.clone(),
        intent_id: rejected.intent_id.clone(),
        effect_kind: rejected.effect_kind.clone(),
        params_hash: rejected.params_hash.clone(),
        receipt_payload: serde_cbor::to_vec(&payload).unwrap_or_default(),
        status: rejected.status.clone(),
        emitted_at_seq: rejected.emitted_at_seq,
        adapter_id: rejected.adapter_id.clone(),
        cost_cents: None,
        signature: Vec::new(),
    };
    if handle_pending_blob_get_receipt(state, &envelope, out)?
        || handle_pending_blob_put_receipt(state, &envelope, out)?
        || settle_tool_call_from_receipt(state, &envelope, out)?
    {
        remove_pending_intent_for_receipt(
            state,
            rejected.params_hash.as_ref(),
            rejected.intent_id.as_str(),
        );
        recompute_in_flight_effects(state);
        return Ok(());
    }

    remove_pending_intent_for_receipt(
        state,
        rejected.params_hash.as_ref(),
        rejected.intent_id.as_str(),
    );

    transition_lifecycle(state, SessionLifecycle::Failed)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    recompute_in_flight_effects(state);
    Ok(())
}

fn pop_pending_blob_get(state: &mut SessionState, params_hash: &str) -> Option<PendingBlobGet> {
    let mut should_remove = false;
    let next = if let Some(items) = state.pending_blob_gets.get_mut(params_hash) {
        let value = if items.is_empty() {
            None
        } else {
            Some(items.remove(0))
        };
        should_remove = items.is_empty();
        value
    } else {
        None
    };
    if should_remove {
        state.pending_blob_gets.remove(params_hash);
    }
    next
}

fn pop_pending_blob_put(state: &mut SessionState, params_hash: &str) -> Option<PendingBlobPut> {
    let mut should_remove = false;
    let next = if let Some(items) = state.pending_blob_puts.get_mut(params_hash) {
        let value = if items.is_empty() {
            None
        } else {
            Some(items.remove(0))
        };
        should_remove = items.is_empty();
        value
    } else {
        None
    };
    if should_remove {
        state.pending_blob_puts.remove(params_hash);
    }
    next
}

fn remove_pending_intent_for_receipt(
    state: &mut SessionState,
    params_hash: Option<&String>,
    intent_id: &str,
) {
    if let Some(params_hash) = params_hash {
        state.pending_intents.remove(params_hash);
        return;
    }

    if let Some((key, _intent)) = state
        .pending_intents
        .iter()
        .find(|(_, pending)| pending.intent_id.as_deref() == Some(intent_id))
        .map(|(k, v)| (k.clone(), v.clone()))
    {
        state.pending_intents.remove(&key);
    }
}

fn recompute_in_flight_effects(state: &mut SessionState) {
    let pending_blob_gets = state
        .pending_blob_gets
        .values()
        .map(|items| items.len())
        .sum::<usize>();
    let pending_blob_puts = state
        .pending_blob_puts
        .values()
        .map(|items| items.len())
        .sum::<usize>();
    let pending_tool_effect_receipts = state
        .active_tool_batch
        .as_ref()
        .map(|batch| {
            batch
                .pending_by_params_hash
                .values()
                .map(|items| items.len())
                .sum::<usize>()
        })
        .unwrap_or(0);
    let pending_host_loop_calls = state
        .active_tool_batch
        .as_ref()
        .map(|batch| {
            batch
                .call_status
                .values()
                .filter(|status| matches!(status, ToolCallStatus::Pending))
                .count()
        })
        .unwrap_or(0);

    let total = state.pending_intents.len()
        + pending_blob_gets
        + pending_blob_puts
        + pending_tool_effect_receipts
        + pending_host_loop_calls;
    state.in_flight_effects = total as u64;
}

fn has_pending_tool_definition_puts(state: &SessionState) -> bool {
    state.pending_blob_puts.values().any(|items| {
        items
            .iter()
            .any(|pending| matches!(pending.kind, PendingBlobPutKind::ToolDefinition { .. }))
    })
}

fn enqueue_blob_get(
    state: &mut SessionState,
    blob_ref: HashRef,
    kind: PendingBlobGetKind,
    out: &mut SessionReduceOutput,
) -> Result<String, SessionReduceError> {
    let params = BlobGetParams { blob_ref };
    let params_hash = hash_cbor(&params);
    let already_pending = state.pending_blob_gets.contains_key(&params_hash);
    state
        .pending_blob_gets
        .entry(params_hash.clone())
        .or_default()
        .push(PendingBlobGet {
            kind,
            emitted_at_ns: state.updated_at,
        });
    if !already_pending {
        out.effects.push(SessionEffectCommand::BlobGet {
            params,
            cap_slot: Some("blob".into()),
            params_hash: params_hash.clone(),
        });
    }
    Ok(params_hash)
}

fn enqueue_blob_put(
    state: &mut SessionState,
    bytes: Vec<u8>,
    kind: PendingBlobPutKind,
    out: &mut SessionReduceOutput,
) -> String {
    let params = BlobPutParams {
        bytes,
        blob_ref: None,
        refs: None,
    };
    let params_hash = hash_cbor(&params);
    let already_pending = state.pending_blob_puts.contains_key(&params_hash);
    state
        .pending_blob_puts
        .entry(params_hash.clone())
        .or_default()
        .push(PendingBlobPut {
            kind,
            emitted_at_ns: state.updated_at,
        });
    if !already_pending {
        out.effects.push(SessionEffectCommand::BlobPut {
            params,
            cap_slot: Some("blob".into()),
            params_hash: params_hash.clone(),
        });
    }
    params_hash
}

fn fail_run(state: &mut SessionState) -> Result<(), SessionReduceError> {
    transition_lifecycle(state, SessionLifecycle::Failed)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    Ok(())
}

fn handle_pending_blob_get_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(params_hash) = envelope.params_hash.as_ref() else {
        return Ok(false);
    };
    let Some(pending) = pop_pending_blob_get(state, params_hash.as_str()) else {
        return Ok(false);
    };

    let failed = envelope.status != "ok";
    let receipt = if failed {
        None
    } else {
        serde_cbor::from_slice::<BlobGetReceipt>(&envelope.receipt_payload).ok()
    };

    match pending.kind {
        PendingBlobGetKind::LlmOutputEnvelope => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            let output: LlmOutputEnvelope = match serde_json::from_slice(&receipt.bytes) {
                Ok(value) => value,
                Err(_) => {
                    fail_run(state)?;
                    return Ok(true);
                }
            };
            if let Some(tool_calls_ref) = output.tool_calls_ref {
                enqueue_blob_get(state, tool_calls_ref, PendingBlobGetKind::LlmToolCalls, out)?;
            } else if matches!(state.lifecycle, SessionLifecycle::Running) {
                transition_lifecycle(state, SessionLifecycle::WaitingInput)
                    .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            }
            Ok(true)
        }
        PendingBlobGetKind::LlmToolCalls => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            let calls: LlmToolCallList = match serde_json::from_slice(&receipt.bytes) {
                Ok(value) => value,
                Err(_) => {
                    fail_run(state)?;
                    return Ok(true);
                }
            };
            if calls.is_empty() {
                if matches!(state.lifecycle, SessionLifecycle::Running) {
                    transition_lifecycle(state, SessionLifecycle::WaitingInput)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                }
                return Ok(true);
            }
            let observed = calls
                .into_iter()
                .map(|call| ToolCallObserved {
                    call_id: call.call_id,
                    tool_name: call.tool_name,
                    arguments_json: String::new(),
                    arguments_ref: Some(call.arguments_ref.as_str().to_string()),
                    provider_call_id: call.provider_call_id,
                })
                .collect::<Vec<_>>();
            on_tool_calls_observed(state, envelope.intent_id.as_str(), None, &observed, out)?;
            Ok(true)
        }
        PendingBlobGetKind::ToolCallArguments {
            tool_batch_id,
            call_id,
        } => {
            if failed {
                if let Some(batch) = state.active_tool_batch.as_mut()
                    && batch.tool_batch_id == tool_batch_id
                {
                    batch.call_status.insert(
                        call_id.clone(),
                        ToolCallStatus::Failed {
                            code: "tool_arguments_ref_failed".into(),
                            detail: "blob.get for tool arguments failed".into(),
                        },
                    );
                }
                dispatch_next_ready_tool_group(state, out)?;
                return Ok(true);
            }

            let Some(receipt) = receipt else {
                if let Some(batch) = state.active_tool_batch.as_mut()
                    && batch.tool_batch_id == tool_batch_id
                {
                    batch.call_status.insert(
                        call_id.clone(),
                        ToolCallStatus::Failed {
                            code: "tool_arguments_ref_decode_failed".into(),
                            detail: "failed to decode blob.get receipt payload".into(),
                        },
                    );
                }
                dispatch_next_ready_tool_group(state, out)?;
                return Ok(true);
            };

            let args_json = match serde_json::from_slice::<serde_json::Value>(&receipt.bytes)
                .and_then(|value| serde_json::to_string(&value))
            {
                Ok(value) => value,
                Err(_) => {
                    if let Some(batch) = state.active_tool_batch.as_mut()
                        && batch.tool_batch_id == tool_batch_id
                    {
                        batch.call_status.insert(
                            call_id.clone(),
                            ToolCallStatus::Failed {
                                code: "tool_arguments_not_json".into(),
                                detail: "tool arguments blob must contain JSON".into(),
                            },
                        );
                    }
                    dispatch_next_ready_tool_group(state, out)?;
                    return Ok(true);
                }
            };

            if let Some(batch) = state.active_tool_batch.as_mut()
                && batch.tool_batch_id == tool_batch_id
            {
                if let Some(planned) = batch
                    .plan
                    .planned_calls
                    .iter_mut()
                    .find(|planned| planned.call_id == call_id)
                {
                    planned.arguments_json = args_json;
                }
                batch
                    .call_status
                    .insert(call_id.clone(), ToolCallStatus::Queued);
                if let Some(group_idx) = batch
                    .plan
                    .execution_plan
                    .groups
                    .iter()
                    .position(|group| group.iter().any(|id| id == &call_id))
                {
                    batch.next_group_index = group_idx as u64;
                }
            }
            dispatch_next_ready_tool_group(state, out)?;
            Ok(true)
        }
    }
}

fn handle_pending_blob_put_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(params_hash) = envelope.params_hash.as_ref() else {
        return Ok(false);
    };
    let Some(pending) = pop_pending_blob_put(state, params_hash.as_str()) else {
        return Ok(false);
    };

    let failed = envelope.status != "ok";
    let receipt = if failed {
        None
    } else {
        serde_cbor::from_slice::<BlobPutReceipt>(&envelope.receipt_payload).ok()
    };

    match pending.kind {
        PendingBlobPutKind::ToolDefinition { tool_name } => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            let blob_ref = receipt.blob_ref.as_str().to_string();
            if let Some(spec) = state.tool_registry.get_mut(&tool_name) {
                spec.tool_ref = blob_ref.clone();
            }
            for tool in &mut state.effective_tools.ordered_tools {
                if tool.tool_name == tool_name {
                    tool.tool_ref = blob_ref.clone();
                }
            }
            if !has_pending_tool_definition_puts(state) {
                state.tool_refs_materialized = true;
                dispatch_queued_llm_turn(state, out)?;
            }
            Ok(true)
        }
        PendingBlobPutKind::FollowUpMessage { index } => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            if let Some(turn) = state.pending_follow_up_turn.as_mut() {
                turn.blob_refs_by_index
                    .insert(index, receipt.blob_ref.as_str().to_string());
                if turn.blob_refs_by_index.len() as u64 >= turn.expected_messages {
                    let mut refs = Vec::new();
                    for idx in 0..turn.expected_messages {
                        if let Some(value) = turn.blob_refs_by_index.get(&idx) {
                            refs.push(value.clone());
                        }
                    }
                    let mut next_refs = turn.base_message_refs.clone();
                    next_refs.extend(refs);
                    state.conversation_message_refs = next_refs.clone();
                    state.pending_follow_up_turn = None;
                    queue_llm_turn(state, next_refs, out)?;
                }
            }
            Ok(true)
        }
    }
}

fn queue_llm_turn(
    state: &mut SessionState,
    message_refs: Vec<String>,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    state.queued_llm_message_refs = Some(message_refs);
    dispatch_queued_llm_turn(state, out)
}

fn dispatch_queued_llm_turn(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if state.queued_llm_message_refs.is_none() {
        return Ok(());
    }
    if !state.pending_intents.is_empty() || state.pending_follow_up_turn.is_some() {
        return Ok(());
    }

    if !state.tool_refs_materialized {
        for tool in state.effective_tools.ordered_tools.clone() {
            let bytes = crate::tools::registry::tool_definition_bytes(
                tool.tool_name.as_str(),
                tool.description.as_str(),
                tool.args_schema_json.as_str(),
            );
            enqueue_blob_put(
                state,
                bytes,
                PendingBlobPutKind::ToolDefinition {
                    tool_name: tool.tool_name,
                },
                out,
            );
        }
        if has_pending_tool_definition_puts(state) {
            return Ok(());
        }
        state.tool_refs_materialized = true;
    }

    let Some(message_refs) = state.queued_llm_message_refs.take() else {
        return Ok(());
    };
    let Some(run_config) = state.active_run_config.clone() else {
        return Ok(());
    };
    let run_seq = state
        .active_run_id
        .as_ref()
        .map(|id| id.run_seq)
        .unwrap_or(0);

    let step_ctx = LlmStepContext {
        correlation_id: Some(alloc::format!(
            "run-{run_seq}-turn-{}",
            state.next_tool_batch_seq + 1
        )),
        message_refs,
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
        &run_config,
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
    out.effects.push(SessionEffectCommand::LlmGenerate {
        params,
        cap_slot: Some("llm".into()),
        params_hash,
    });
    Ok(())
}

fn start_follow_up_for_settled_batch(
    state: &mut SessionState,
    batch: &mut ActiveToolBatch,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    let mut ordered_results = Vec::new();
    for observed in &batch.plan.observed_calls {
        if let Some(result) = batch.llm_results.get(&observed.call_id) {
            ordered_results.push(result.clone());
        }
    }
    batch.results_ref = Some(hash_cbor(&ordered_results));

    let accepted_calls = batch
        .plan
        .planned_calls
        .iter()
        .filter(|planned| planned.accepted)
        .cloned()
        .collect::<Vec<_>>();

    let mut messages = Vec::new();
    if !accepted_calls.is_empty() {
        let tool_calls = accepted_calls
            .iter()
            .map(|call| {
                let parsed_args = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                    .unwrap_or_else(|_| serde_json::json!({}));
                serde_json::json!({
                    "id": call.call_id,
                    "name": call.tool_name,
                    "arguments": parsed_args,
                })
            })
            .collect::<Vec<_>>();
        messages.push(serde_json::json!({
            "role": "assistant",
            "tool_calls": tool_calls,
        }));
    }

    for result in ordered_results {
        let output = serde_json::from_str::<serde_json::Value>(&result.output_json)
            .unwrap_or_else(|_| serde_json::Value::String(result.output_json.clone()));
        messages.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": result.call_id,
            "output": output,
            "is_error": result.is_error,
        }));
    }

    if messages.is_empty() {
        if matches!(state.lifecycle, SessionLifecycle::Running) {
            transition_lifecycle(state, SessionLifecycle::WaitingInput)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        return Ok(());
    }

    let mut expected_messages = 0_u64;
    for (idx, message) in messages.into_iter().enumerate() {
        let bytes = serde_json::to_vec(&message).unwrap_or_else(|_| b"{}".to_vec());
        enqueue_blob_put(
            state,
            bytes,
            PendingBlobPutKind::FollowUpMessage { index: idx as u64 },
            out,
        );
        expected_messages = expected_messages.saturating_add(1);
    }
    state.pending_follow_up_turn = Some(PendingFollowUpTurn {
        tool_batch_id: batch.tool_batch_id.clone(),
        base_message_refs: state.conversation_message_refs.clone(),
        expected_messages,
        blob_refs_by_index: BTreeMap::new(),
    });
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
            description: spec.description.clone(),
            args_schema_json: spec.args_schema_json.clone(),
            mapper: spec.mapper,
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
    state.tool_refs_materialized = state.effective_tools.ordered_tools.is_empty();

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
    state.pending_blob_gets.clear();
    state.pending_blob_puts.clear();
    state.pending_follow_up_turn = None;
    state.queued_llm_message_refs = None;
    state.conversation_message_refs.clear();
    state.tool_refs_materialized = false;
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
    use aos_air_types::HashRef;
    use aos_effects::builtins::{
        BlobGetReceipt, BlobPutReceipt, LlmFinishReason, LlmGenerateReceipt, LlmOutputEnvelope,
        LlmToolCall, TokenUsage,
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

    fn hash_ref(ch: char) -> HashRef {
        HashRef::new(fake_hash(ch)).expect("valid hash ref")
    }

    fn hash_ref_for_index(idx: usize) -> HashRef {
        let alphabet = [
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd',
        ];
        hash_ref(alphabet[idx % alphabet.len()])
    }

    fn receipt_event<T: serde::Serialize>(
        emitted_at_seq: u64,
        effect_kind: &str,
        params_hash: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Receipt(aos_wasm_sdk::EffectReceiptEnvelope {
            origin_module_id: "aos.agent/SessionWorkflow@1".into(),
            origin_instance_key: None,
            intent_id: fake_hash('i'),
            effect_kind: effect_kind.into(),
            params_hash,
            receipt_payload: serde_cbor::to_vec(payload).expect("encode payload"),
            status: status.into(),
            emitted_at_seq,
            adapter_id: "adapter.mock".into(),
            cost_cents: None,
            signature: Vec::new(),
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
        assert!(matches!(
            out.effects.first(),
            Some(SessionEffectCommand::BlobPut { .. })
        ));
        assert_eq!(state.pending_intents.len(), 0);
        assert!(!state.pending_blob_puts.is_empty());
        assert!(state.queued_llm_message_refs.is_some());
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
                arguments_json: "{\"path\":\"a.txt\",\"text\":\"x\"}".into(),
                arguments_ref: Some(fake_hash('w')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c2".into(),
                tool_name: "host.fs.apply_patch".into(),
                arguments_json: "{\"patch\":\"*** Begin Patch\\n*** End Patch\\n\"}".into(),
                arguments_ref: Some(fake_hash('p')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c3".into(),
                tool_name: "host.exec".into(),
                arguments_json: "{\"argv\":[\"ls\"]}".into(),
                arguments_ref: Some(fake_hash('e')),
                provider_call_id: None,
            },
        ];

        let mut out = SessionReduceOutput::default();
        let params_hash = fake_hash('h');
        on_tool_calls_observed(
            &mut state,
            fake_hash('i').as_str(),
            Some(&params_hash),
            &calls,
            &mut out,
        )
        .expect("plan");

        let batch = state.active_tool_batch.as_ref().expect("active batch");
        assert_eq!(
            batch.plan.execution_plan.groups,
            vec![vec![String::from("c1")], vec![String::from("c2")]]
        );
        assert!(matches!(
            batch.call_status.get("c3"),
            Some(ToolCallStatus::Ignored)
        ));
    }

    #[test]
    fn run_request_materializes_tool_refs_before_llm_generate() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        let blob_put_hash = match out.effects.first() {
            Some(SessionEffectCommand::BlobPut { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected blob.put effect"),
        };

        let out = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                2,
                "blob.put",
                Some(blob_put_hash),
                "ok",
                &BlobPutReceipt {
                    blob_ref: hash_ref('a'),
                    edge_ref: hash_ref('b'),
                    size: 42,
                },
            ),
        )
        .expect("blob.put receipt");

        assert!(matches!(
            out.effects.first(),
            Some(SessionEffectCommand::LlmGenerate { .. })
        ));
        assert_eq!(state.pending_intents.len(), 1);
    }

    #[test]
    fn llm_tool_calls_are_resolved_executed_and_queued_for_follow_up() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out1 = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        let tool_def_put_hash = match out1.effects.first() {
            Some(SessionEffectCommand::BlobPut { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected initial blob.put"),
        };

        let out2 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                2,
                "blob.put",
                Some(tool_def_put_hash),
                "ok",
                &BlobPutReceipt {
                    blob_ref: hash_ref('c'),
                    edge_ref: hash_ref('d'),
                    size: 111,
                },
            ),
        )
        .expect("tool def put receipt");
        let llm_params_hash = match out2.effects.first() {
            Some(SessionEffectCommand::LlmGenerate { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected llm.generate"),
        };

        let out3 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                3,
                "llm.generate",
                Some(llm_params_hash),
                "ok",
                &LlmGenerateReceipt {
                    output_ref: hash_ref('e'),
                    raw_output_ref: None,
                    provider_response_id: None,
                    finish_reason: LlmFinishReason {
                        reason: "tool_calls".into(),
                        raw: None,
                    },
                    token_usage: TokenUsage {
                        prompt: 0,
                        completion: 0,
                        total: Some(0),
                    },
                    usage_details: None,
                    warnings_ref: None,
                    rate_limit_ref: None,
                    cost_cents: None,
                    provider_id: "openai-responses".into(),
                },
            ),
        )
        .expect("llm receipt");
        let output_blob_get_hash = match out3.effects.first() {
            Some(SessionEffectCommand::BlobGet { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected blob.get for llm output"),
        };

        let output_bytes = serde_json::to_vec(&LlmOutputEnvelope {
            assistant_text: None,
            tool_calls_ref: Some(hash_ref('c')),
            reasoning_ref: None,
        })
        .expect("encode output envelope");
        let out4 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                4,
                "blob.get",
                Some(output_blob_get_hash),
                "ok",
                &BlobGetReceipt {
                    blob_ref: hash_ref('e'),
                    size: output_bytes.len() as u64,
                    bytes: output_bytes,
                },
            ),
        )
        .expect("output blob.get receipt");
        let calls_blob_get_hash = match out4.effects.first() {
            Some(SessionEffectCommand::BlobGet { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected blob.get for tool calls"),
        };

        let call_list = vec![LlmToolCall {
            call_id: "call-1".into(),
            tool_name: "host.session.open".into(),
            arguments_ref: hash_ref('d'),
            provider_call_id: None,
        }];
        let calls_bytes = serde_json::to_vec(&call_list).expect("encode tool calls");
        let out5 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                5,
                "blob.get",
                Some(calls_blob_get_hash),
                "ok",
                &BlobGetReceipt {
                    blob_ref: hash_ref('c'),
                    size: calls_bytes.len() as u64,
                    bytes: calls_bytes,
                },
            ),
        )
        .expect("tool calls blob.get receipt");
        let args_blob_get_hash = match out5.effects.first() {
            Some(SessionEffectCommand::BlobGet { params_hash, .. }) => params_hash.clone(),
            _ => panic!("expected blob.get for tool arguments"),
        };

        let args_bytes = br#"{"target":{"local":{"network_mode":"off"}}}"#.to_vec();
        let out6 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                6,
                "blob.get",
                Some(args_blob_get_hash),
                "ok",
                &BlobGetReceipt {
                    blob_ref: hash_ref('d'),
                    size: args_bytes.len() as u64,
                    bytes: args_bytes,
                },
            ),
        )
        .expect("tool args blob.get receipt");
        let tool_effect_hash = out6
            .effects
            .iter()
            .find_map(|effect| {
                if let SessionEffectCommand::ToolEffect { params_hash, .. } = effect {
                    Some(params_hash.clone())
                } else {
                    None
                }
            })
            .expect("expected tool effect");

        let out7 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                7,
                "host.session.open",
                Some(tool_effect_hash),
                "ok",
                &serde_json::json!({
                    "status": "ready",
                    "session_id": "hs_1"
                }),
            ),
        )
        .expect("tool receipt");
        assert!(
            out7.effects
                .iter()
                .any(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. })),
            "expected follow-up blob.put effects"
        );

        let followup_hashes = out7
            .effects
            .iter()
            .filter_map(|effect| {
                if let SessionEffectCommand::BlobPut { params_hash, .. } = effect {
                    Some(params_hash.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert!(!followup_hashes.is_empty());

        let mut emitted_llm = false;
        for (idx, hash) in followup_hashes.iter().enumerate() {
            let out = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    8 + idx as u64,
                    "blob.put",
                    Some(hash.clone()),
                    "ok",
                    &BlobPutReceipt {
                        blob_ref: hash_ref_for_index(idx),
                        edge_ref: hash_ref_for_index(idx + 1),
                        size: 32,
                    },
                ),
            )
            .expect("follow-up blob.put receipt");
            emitted_llm = emitted_llm
                || out
                    .effects
                    .iter()
                    .any(|effect| matches!(effect, SessionEffectCommand::LlmGenerate { .. }));
        }
        assert!(emitted_llm, "expected queued follow-up llm.generate");
    }
}
