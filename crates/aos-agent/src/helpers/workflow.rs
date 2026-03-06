use super::{
    allocate_run_id, allocate_tool_batch_id, can_apply_host_command, enqueue_host_text,
    pop_follow_up_if_ready, transition_lifecycle,
};
use crate::contracts::{
    ActiveToolBatch, EffectiveTool, EffectiveToolSet, HostCommandKind, PendingBlobGet,
    PendingBlobGetKind, PendingBlobPut, PendingBlobPutKind, PendingFollowUpTurn, PlannedToolCall,
    RunConfig, SessionConfig, SessionIngressKind, SessionLifecycle, SessionState,
    SessionWorkflowEvent, ToolAvailabilityRule, ToolBatchPlan, ToolCallObserved, ToolCallStatus,
    ToolExecutionPlan, ToolExecutor, ToolOverrideScope, ToolSpec,
    default_tool_profile_for_provider,
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
    BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt, LlmGenerateParams,
    LlmGenerateReceipt, LlmOutputEnvelope, LlmToolCallList, LlmToolChoice, TextOrSecretRef,
};
use aos_wasm_sdk::{
    EffectReceiptEnvelope, EffectReceiptRejected, PendingBatch, PendingEffect,
    PendingEffectLookupError, PendingEffectSet,
};

use super::llm::{
    LlmMappingError, LlmStepContext, materialize_llm_generate_params_with_prompt_refs,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffectCommand {
    LlmGenerate {
        params: LlmGenerateParams,
        cap_slot: Option<String>,
        params_hash: String,
    },
    ToolEffect {
        kind: ToolEffectKind,
        params_json: String,
        cap_slot: Option<String>,
        issuer_ref: Option<String>,
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
    pub max_pending_effects: Option<u64>,
}

const TOOL_RESULT_BLOB_MAX_BYTES: usize = 8 * 1024;

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
    EmptyMessageRefs,
    TooManyPendingEffects,
    InvalidHashRef,
    ToolProfileUnknown,
    UnknownToolOverride,
    InvalidToolRegistry,
    AmbiguousPendingToolEffect,
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
            Self::EmptyMessageRefs => "llm message_refs must not be empty",
            Self::TooManyPendingEffects => "too many pending effects",
            Self::InvalidHashRef => "invalid hash ref",
            Self::ToolProfileUnknown => "tool profile unknown",
            Self::UnknownToolOverride => "unknown tool override",
            Self::InvalidToolRegistry => "invalid tool registry",
            Self::AmbiguousPendingToolEffect => "ambiguous pending tool effect",
        }
    }
}

impl core::fmt::Display for SessionReduceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl core::error::Error for SessionReduceError {}

fn pending_effect_lookup_err_to_session_err(_err: PendingEffectLookupError) -> SessionReduceError {
    SessionReduceError::AmbiguousPendingToolEffect
}

fn build_tool_execution(
    groups: Vec<Vec<String>>,
    call_status: &BTreeMap<String, ToolCallStatus>,
) -> PendingBatch<String> {
    let mut execution = PendingBatch::from_groups(groups);
    for (call_id, status) in call_status {
        if status.is_terminal() {
            let _ = execution.mark_terminal(call_id);
        }
    }
    execution
}

fn set_tool_call_status(batch: &mut ActiveToolBatch, call_id: &String, status: ToolCallStatus) {
    if status.is_terminal() {
        let _ = batch.execution.mark_terminal(call_id);
    }
    batch.call_status.insert(call_id.clone(), status);
}

fn fail_tool_call(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    code: &str,
    detail: impl Into<String>,
) {
    set_tool_call_status(
        batch,
        call_id,
        ToolCallStatus::Failed {
            code: code.into(),
            detail: detail.into(),
        },
    );
}

fn transition_to_waiting_input_if_running(
    state: &mut SessionState,
) -> Result<(), SessionReduceError> {
    if matches!(state.lifecycle, SessionLifecycle::Running) {
        transition_lifecycle(state, SessionLifecycle::WaitingInput)
            .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    }
    Ok(())
}

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
        SessionWorkflowEvent::StreamFrame(frame) => {
            let _ = state.pending_effects.observe(frame.into());
        }
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
        LlmMappingError::EmptyMessageRefs => SessionReduceError::EmptyMessageRefs,
        LlmMappingError::InvalidHashRef => SessionReduceError::InvalidHashRef,
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

fn validate_tool_registry_payload(
    registry: &BTreeMap<String, ToolSpec>,
    profiles: Option<&BTreeMap<String, Vec<String>>>,
    default_profile: Option<&String>,
) -> Result<(), SessionReduceError> {
    crate::tools::registry::validate_tool_registry(registry)
        .map_err(|_| SessionReduceError::InvalidToolRegistry)?;

    if let Some(profiles) = profiles {
        for tool_ids in profiles.values() {
            for tool_id in tool_ids {
                if !registry.contains_key(tool_id) {
                    return Err(SessionReduceError::InvalidToolRegistry);
                }
            }
        }
        if let Some(profile) = default_profile
            && !profiles.contains_key(profile)
        {
            return Err(SessionReduceError::InvalidToolRegistry);
        }
    }
    Ok(())
}

fn on_tool_registry_set(
    state: &mut SessionState,
    registry: &BTreeMap<String, ToolSpec>,
    profiles: Option<&BTreeMap<String, Vec<String>>>,
    default_profile: Option<&String>,
) -> Result<(), SessionReduceError> {
    validate_tool_registry_payload(registry, profiles, default_profile)?;
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
    let execution = build_tool_execution(plan.execution_plan.groups.clone(), &call_status);

    state.active_tool_batch = Some(ActiveToolBatch {
        tool_batch_id,
        intent_id: intent_id.into(),
        params_hash: params_hash.cloned(),
        plan,
        call_status,
        pending_effects: PendingEffectSet::new(),
        execution,
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
        if let Some(tool) = state.effective_tools.tool_by_llm_name(&call.tool_name) {
            planned_calls.push(PlannedToolCall {
                call_id: call.call_id.clone(),
                tool_id: tool.tool_id.clone(),
                tool_name: tool.tool_name.clone(),
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
                tool_id: String::new(),
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
        batch.execution.advance_completed();
        if batch.execution.is_complete() {
            if batch.is_settled() && batch.results_ref.is_none() {
                start_follow_up_for_settled_batch(state, &mut batch, out)?;
            }
            state.active_tool_batch = Some(batch);
            recompute_in_flight_effects(state);
            return Ok(());
        }

        let group = batch
            .execution
            .current_group_keys()
            .map(|group| group.to_vec())
            .unwrap_or_default();
        let _ = batch.execution.advance();

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
                    set_tool_call_status(&mut batch, &call_id, ToolCallStatus::Pending);
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
            let kind =
                if let Some(mapper) = crate::tools::mapper_for_effect_kind(effect_kind.as_str()) {
                    crate::tools::effect_kind_for_mapper(mapper)
                } else {
                    fail_tool_call(
                        &mut batch,
                        &call_id,
                        "executor_unsupported",
                        format!("unsupported effect kind for wasm emit_raw: {effect_kind}"),
                    );
                    continue;
                };

            let arguments_json = if !planned.arguments_json.trim().is_empty() {
                planned.arguments_json.clone()
            } else if let Some(arguments_ref) = planned.arguments_ref.clone() {
                let blob_ref = match HashRef::new(arguments_ref) {
                    Ok(value) => value,
                    Err(err) => {
                        fail_tool_call(
                            &mut batch,
                            &call_id,
                            "tool_invalid_args_ref",
                            format!("invalid arguments_ref for {}: {err}", planned.tool_name),
                        );
                        continue;
                    }
                };
                let blob_get = BlobGetParams { blob_ref };
                let blob_get_hash = hash_cbor(&blob_get);
                let pending_kind = PendingBlobGetKind::ToolCallArguments {
                    tool_batch_id: batch.tool_batch_id.clone(),
                    call_id: call_id.clone(),
                };
                let already_pending = state.pending_blob_gets.contains_key(&blob_get_hash)
                    || out.effects.iter().any(|effect| {
                        matches!(
                            effect,
                            SessionEffectCommand::BlobGet { params_hash, .. }
                                if params_hash == &blob_get_hash
                        )
                    });
                enqueue_blob_get(state, blob_get.blob_ref, pending_kind, out)?;
                if !already_pending {
                    emitted_for_group = emitted_for_group.saturating_add(1);
                }
                set_tool_call_status(&mut batch, &call_id, ToolCallStatus::Pending);
                continue;
            } else {
                fail_tool_call(
                    &mut batch,
                    &call_id,
                    "tool_invalid_args",
                    format!(
                        "tool {} missing arguments_json and arguments_ref",
                        planned.tool_name
                    ),
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
                    set_tool_call_status(&mut batch, &call_id, err.to_failed_status());
                    batch.llm_results.insert(
                        call_id.clone(),
                        crate::contracts::ToolCallLlmResult {
                            call_id: call_id.clone(),
                            tool_id: planned.tool_id.clone(),
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

            let pending = batch
                .pending_effects
                .begin_with_issuer_ref(
                    call_id.clone(),
                    kind.as_str(),
                    &params_json,
                    cap_slot.clone(),
                    state.updated_at,
                    Some(call_id.clone()),
                )
                .unwrap_or_else(|_| {
                    insert_fallback_pending_tool_effect(
                        &mut batch,
                        &call_id,
                        kind,
                        cap_slot.clone(),
                        state.updated_at,
                    )
                });
            let params_hash = pending.params_hash.clone();
            set_tool_call_status(&mut batch, &call_id, ToolCallStatus::Pending);
            emitted_for_group = emitted_for_group.saturating_add(1);

            out.effects.push(SessionEffectCommand::ToolEffect {
                kind,
                params_json: serde_json::to_string(&params_json).unwrap_or_else(|_| "{}".into()),
                cap_slot,
                issuer_ref: Some(call_id.clone()),
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

fn insert_fallback_pending_tool_effect(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    kind: ToolEffectKind,
    cap_slot: Option<String>,
    emitted_at_ns: u64,
) -> PendingEffect {
    let pending = PendingEffect::new(kind.as_str(), String::new(), cap_slot, emitted_at_ns)
        .with_issuer_ref(call_id.clone());
    batch
        .pending_effects
        .insert(call_id.clone(), pending.clone());
    pending
}

fn settle_tool_call_from_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let (call_id, planned, tool_batch_id) = {
        let Some(batch) = state.active_tool_batch.as_mut() else {
            return Ok(false);
        };
        let Some(matched) = batch
            .pending_effects
            .settle(envelope.into())
            .map_err(pending_effect_lookup_err_to_session_err)?
        else {
            return Ok(false);
        };
        let call_id = matched.key;
        let Some(planned) = batch
            .plan
            .planned_calls
            .iter()
            .find(|call| call.call_id == call_id)
            .cloned()
        else {
            return Ok(false);
        };
        (call_id, planned, batch.tool_batch_id.clone())
    };

    let mapped = map_tool_receipt_to_llm_result(
        planned.mapper,
        planned.tool_name.as_str(),
        envelope.status.as_str(),
        envelope.receipt_payload.as_slice(),
    );
    let expandable_blob_refs = if matches!(mapped.status, ToolCallStatus::Succeeded) {
        collect_blob_refs_from_output_json(mapped.llm_output_json.as_str())
    } else {
        Vec::new()
    };

    let mut queued_blob_refs = Vec::new();
    for blob_ref in expandable_blob_refs {
        if HashRef::new(blob_ref.clone()).is_ok() {
            queued_blob_refs.push(blob_ref);
        }
    }

    if let Some(batch) = state.active_tool_batch.as_mut()
        && batch.tool_batch_id == tool_batch_id
    {
        batch.llm_results.insert(
            call_id.clone(),
            crate::contracts::ToolCallLlmResult {
                call_id: call_id.clone(),
                tool_id: planned.tool_id.clone(),
                tool_name: planned.tool_name,
                is_error: mapped.is_error,
                output_json: mapped.llm_output_json.clone(),
            },
        );
        let initial_status = if !queued_blob_refs.is_empty() {
            ToolCallStatus::Pending
        } else {
            mapped.status.clone()
        };
        set_tool_call_status(batch, &call_id, initial_status);
    }

    for blob_ref in &queued_blob_refs {
        let hash_ref = match HashRef::new(blob_ref.clone()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        enqueue_blob_get(
            state,
            hash_ref,
            PendingBlobGetKind::ToolResultBlob {
                tool_batch_id: tool_batch_id.clone(),
                call_id: call_id.clone(),
                blob_ref: blob_ref.clone(),
            },
            out,
        )?;
    }

    let mut runtime_changed = false;
    if let Some(host_session_id) = mapped.runtime_delta.host_session_id {
        state.tool_runtime_context.host_session_id = Some(host_session_id);
        runtime_changed = true;
    }
    if let Some(host_session_status) = mapped.runtime_delta.host_session_status {
        state.tool_runtime_context.host_session_status = Some(host_session_status);
        runtime_changed = true;
    }
    if runtime_changed {
        let active = state.active_run_config.clone();
        refresh_effective_tools(state, active.as_ref())?;
    }

    dispatch_next_ready_tool_group(state, out)?;
    Ok(true)
}

fn collect_blob_refs_from_output_json(output_json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output_json) else {
        return Vec::new();
    };
    let mut refs = BTreeSet::new();
    collect_blob_refs_from_value(&value, &mut refs);
    refs.into_iter().collect()
}

fn collect_blob_refs_from_value(value: &serde_json::Value, refs: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(blob_ref) = map
                .get("blob")
                .and_then(serde_json::Value::as_object)
                .and_then(|blob| blob.get("blob_ref"))
                .and_then(serde_json::Value::as_str)
            {
                refs.insert(blob_ref.to_string());
            }
            for child in map.values() {
                collect_blob_refs_from_value(child, refs);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_blob_refs_from_value(item, refs);
            }
        }
        _ => {}
    }
}

fn decode_blob_inline_text(bytes: &[u8]) -> (String, bool) {
    let truncated = bytes.len() > TOOL_RESULT_BLOB_MAX_BYTES;
    let capped = if truncated {
        &bytes[..TOOL_RESULT_BLOB_MAX_BYTES]
    } else {
        bytes
    };
    (String::from_utf8_lossy(capped).to_string(), truncated)
}

fn inject_blob_inline_text_into_output_json(
    output_json: &str,
    blob_ref: &str,
    inline_text: &str,
    truncated: bool,
    error: Option<&str>,
) -> Option<String> {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(output_json) else {
        return None;
    };
    let changed =
        inject_blob_inline_text_into_value(&mut value, blob_ref, inline_text, truncated, error);
    if !changed {
        return None;
    }
    serde_json::to_string(&value).ok()
}

fn inject_blob_inline_text_into_value(
    value: &mut serde_json::Value,
    blob_ref: &str,
    inline_text: &str,
    truncated: bool,
    error: Option<&str>,
) -> bool {
    let mut changed = false;
    match value {
        serde_json::Value::Object(map) => {
            if let Some(blob_obj) = map.get("blob").and_then(serde_json::Value::as_object)
                && blob_obj
                    .get("blob_ref")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|current| current == blob_ref)
            {
                map.insert(
                    "inline_text".into(),
                    serde_json::json!({ "text": inline_text }),
                );
                if truncated {
                    map.insert(
                        "inline_text_truncated".into(),
                        serde_json::Value::Bool(true),
                    );
                }
                if let Some(error_text) = error {
                    map.insert(
                        "inline_text_error".into(),
                        serde_json::Value::String(error_text.to_string()),
                    );
                }
                changed = true;
            }

            for child in map.values_mut() {
                changed |= inject_blob_inline_text_into_value(
                    child,
                    blob_ref,
                    inline_text,
                    truncated,
                    error,
                );
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                changed |= inject_blob_inline_text_into_value(
                    item,
                    blob_ref,
                    inline_text,
                    truncated,
                    error,
                );
            }
        }
        _ => {}
    }
    changed
}

fn has_pending_tool_result_blob_get(
    state: &SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &str,
) -> bool {
    state.pending_blob_gets.values().any(|items| {
        items.iter().any(|pending| {
            matches!(
                &pending.kind,
                PendingBlobGetKind::ToolResultBlob {
                    tool_batch_id: pending_batch,
                    call_id: pending_call,
                    ..
                } if pending_batch == tool_batch_id && pending_call == call_id
            )
        })
    })
}

fn handle_standalone_host_session_open_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    if envelope.effect_kind != "host.session.open" {
        return Ok(false);
    }

    let mapped = map_tool_receipt_to_llm_result(
        crate::contracts::ToolMapper::HostSessionOpen,
        "open_session",
        envelope.status.as_str(),
        envelope.receipt_payload.as_slice(),
    );
    if let Some(host_session_id) = mapped.runtime_delta.host_session_id {
        state.tool_runtime_context.host_session_id = Some(host_session_id);
    }
    if let Some(host_session_status) = mapped.runtime_delta.host_session_status {
        state.tool_runtime_context.host_session_status = Some(host_session_status);
    }
    let active = state.active_run_config.clone();
    refresh_effective_tools(state, active.as_ref())?;

    if mapped.is_error {
        fail_run(state)?;
        return Ok(true);
    }

    dispatch_queued_llm_turn(state, out)?;
    Ok(true)
}

fn handle_llm_generate_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if envelope.status != "ok" {
        fail_run(state)?;
        return Ok(());
    }

    let Some(receipt): Option<LlmGenerateReceipt> = envelope.decode_receipt_payload().ok() else {
        fail_run(state)?;
        return Ok(());
    };

    if enqueue_blob_get(
        state,
        receipt.output_ref,
        PendingBlobGetKind::LlmOutputEnvelope,
        out,
    )
    .is_err()
    {
        fail_run(state)?;
    }

    Ok(())
}

fn rejected_as_error_envelope(rejected: &EffectReceiptRejected) -> EffectReceiptEnvelope {
    let payload = serde_json::json!({
        "status": "error",
        "error_code": rejected.error_code,
        "error_message": rejected.error_message,
    });

    EffectReceiptEnvelope {
        origin_module_id: rejected.origin_module_id.clone(),
        origin_instance_key: rejected.origin_instance_key.clone(),
        intent_id: rejected.intent_id.clone(),
        effect_kind: rejected.effect_kind.clone(),
        params_hash: rejected.params_hash.clone(),
        issuer_ref: rejected.issuer_ref.clone(),
        receipt_payload: serde_cbor::to_vec(&payload).unwrap_or_default(),
        status: rejected.status.clone(),
        emitted_at_seq: rejected.emitted_at_seq,
        adapter_id: rejected.adapter_id.clone(),
        cost_cents: None,
        signature: Vec::new(),
    }
}

fn on_receipt_envelope(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if let Some(matched) = state.pending_effects.settle(envelope.into()) {
        match matched.pending.effect_kind.as_str() {
            "blob.get" => {
                let _ = handle_pending_blob_get_receipt(state, envelope, out)?;
            }
            "blob.put" => {
                let _ = handle_pending_blob_put_receipt(state, envelope, out)?;
            }
            "host.session.open" => {
                let _ = handle_standalone_host_session_open_receipt(state, envelope, out)?;
            }
            "llm.generate" => handle_llm_generate_receipt(state, envelope, out)?,
            _ => {}
        }
        recompute_in_flight_effects(state);
        return Ok(());
    }

    if settle_tool_call_from_receipt(state, envelope, out)? {
        recompute_in_flight_effects(state);
        return Ok(());
    }

    recompute_in_flight_effects(state);
    Ok(())
}

fn on_receipt_rejected(
    state: &mut SessionState,
    rejected: &EffectReceiptRejected,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    let envelope = rejected_as_error_envelope(rejected);
    if let Some(matched) = state.pending_effects.settle(rejected.into()) {
        match matched.pending.effect_kind.as_str() {
            "blob.get" => {
                let _ = handle_pending_blob_get_receipt(state, &envelope, out)?;
            }
            "blob.put" => {
                let _ = handle_pending_blob_put_receipt(state, &envelope, out)?;
            }
            "host.session.open" => {
                let _ = handle_standalone_host_session_open_receipt(state, &envelope, out)?;
            }
            "llm.generate" => fail_run(state)?,
            _ => {}
        }
        recompute_in_flight_effects(state);
        return Ok(());
    }

    if !settle_tool_call_from_receipt(state, &envelope, out)? {
        fail_run(state)?;
    }
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

fn recompute_in_flight_effects(state: &mut SessionState) {
    let pending_tool_effect_receipts = state
        .active_tool_batch
        .as_ref()
        .map(|batch| batch.pending_effects.len())
        .unwrap_or(0);
    let pending_host_loop_calls = state
        .active_tool_batch
        .as_ref()
        .map(|batch| {
            batch
                .call_status
                .iter()
                .filter(|(call_id, status)| {
                    matches!(status, ToolCallStatus::Pending)
                        && !batch.pending_effects.contains_key(*call_id)
                })
                .count()
        })
        .unwrap_or(0);

    let total =
        state.pending_effects.len() + pending_tool_effect_receipts + pending_host_loop_calls;
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
    let pending = pending_effect_from_params(state, "blob.get", &params, Some("blob"));
    let params_hash = pending.params_hash.clone();
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
        state.pending_effects.insert(pending);
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
    let pending = pending_effect_from_params(state, "blob.put", &params, Some("blob"));
    let params_hash = pending.params_hash.clone();
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
        state.pending_effects.insert(pending);
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
        envelope.decode_receipt_payload::<BlobGetReceipt>().ok()
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
            } else {
                transition_to_waiting_input_if_running(state)?;
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
                transition_to_waiting_input_if_running(state)?;
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
                    set_tool_call_status(
                        batch,
                        &call_id,
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
                    set_tool_call_status(
                        batch,
                        &call_id,
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
                        set_tool_call_status(
                            batch,
                            &call_id,
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
                set_tool_call_status(batch, &call_id, ToolCallStatus::Queued);
                let _ = batch.execution.rewind_to_group_containing(&call_id);
            }
            dispatch_next_ready_tool_group(state, out)?;
            Ok(true)
        }
        PendingBlobGetKind::ToolResultBlob {
            tool_batch_id,
            call_id,
            blob_ref,
        } => {
            let (inline_text, truncated, error_text) = if let Some(receipt) = receipt {
                let (text, truncated) = decode_blob_inline_text(&receipt.bytes);
                (text, truncated, None)
            } else {
                (
                    String::new(),
                    false,
                    Some(String::from("blob.get failed for tool result output")),
                )
            };

            if let Some(batch) = state.active_tool_batch.as_mut()
                && batch.tool_batch_id == tool_batch_id
                && let Some(result) = batch.llm_results.get_mut(&call_id)
                && let Some(updated_output) = inject_blob_inline_text_into_output_json(
                    result.output_json.as_str(),
                    blob_ref.as_str(),
                    inline_text.as_str(),
                    truncated,
                    error_text.as_deref(),
                )
            {
                result.output_json = updated_output;
            }

            let pending = has_pending_tool_result_blob_get(state, &tool_batch_id, call_id.as_str());
            if !pending
                && let Some(batch) = state.active_tool_batch.as_mut()
                && batch.tool_batch_id == tool_batch_id
                && matches!(
                    batch.call_status.get(&call_id),
                    Some(ToolCallStatus::Pending)
                )
            {
                set_tool_call_status(batch, &call_id, ToolCallStatus::Succeeded);
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
        envelope.decode_receipt_payload::<BlobPutReceipt>().ok()
    };

    match pending.kind {
        PendingBlobPutKind::ToolDefinition { tool_id } => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            let blob_ref = receipt.blob_ref.as_str().to_string();
            if let Some(spec) = state.tool_registry.get_mut(&tool_id) {
                spec.tool_ref = blob_ref.clone();
            }
            for tool in &mut state.effective_tools.ordered_tools {
                if tool.tool_id == tool_id {
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
    if !state.pending_effects.is_empty() || state.pending_follow_up_turn.is_some() {
        return Ok(());
    }

    if should_auto_open_host_session(state) {
        emit_auto_host_session_open(state, out)?;
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
                    tool_id: tool.tool_id,
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
        api_key: provider_secret_ref(run_config.provider.as_str()),
    };

    let params = materialize_llm_generate_params_with_prompt_refs(&run_config, step_ctx)
        .map_err(map_llm_mapping_error)?;
    let params_hash = begin_pending_effect(state, "llm.generate", &params, Some("llm"));
    out.effects.push(SessionEffectCommand::LlmGenerate {
        params,
        cap_slot: Some("llm".into()),
        params_hash,
    });
    Ok(())
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

fn should_auto_open_host_session(state: &SessionState) -> bool {
    if state.active_run_id.is_none() {
        return false;
    }
    if !state.effective_tools.profile_requires_host_session {
        return false;
    }
    if state.tool_runtime_context.host_session_status
        == Some(crate::contracts::HostSessionStatus::Ready)
    {
        return false;
    }
    !state
        .pending_effects
        .contains_effect_kind("host.session.open")
}

fn emit_auto_host_session_open(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    let params = map_tool_arguments_to_effect_params(
        crate::contracts::ToolMapper::HostSessionOpen,
        "{}",
        &state.tool_runtime_context,
    )
    .map_err(|_| SessionReduceError::InvalidToolRegistry)?;
    let params_hash = begin_pending_effect(state, "host.session.open", &params, Some("host"));
    out.effects.push(SessionEffectCommand::ToolEffect {
        kind: ToolEffectKind::HostSessionOpen,
        params_json: serde_json::to_string(&params).unwrap_or_else(|_| "{}".into()),
        cap_slot: Some("host".into()),
        issuer_ref: None,
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
        transition_to_waiting_input_if_running(state)?;
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
    let mut profile_requires_host_session = false;
    for tool_id in ordered_names {
        let Some(spec) = state.tool_registry.get(&tool_id) else {
            return Err(SessionReduceError::UnknownToolOverride);
        };
        if spec
            .availability_rules
            .iter()
            .any(|rule| matches!(rule, ToolAvailabilityRule::HostSessionReady))
        {
            profile_requires_host_session = true;
        }
        if !is_tool_available(spec, &state.tool_runtime_context) {
            continue;
        }
        ordered_tools.push(EffectiveTool {
            tool_id: spec.tool_id.clone(),
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
        profile_requires_host_session,
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
        ToolAvailabilityRule::HostSessionNotReady => {
            runtime.host_session_status != Some(crate::contracts::HostSessionStatus::Ready)
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
    state.pending_effects.clear();
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
    if let Some(max) = limits.max_pending_effects {
        if state.pending_effects.len() as u64 > max {
            return Err(SessionReduceError::TooManyPendingEffects);
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

fn hash_tool_plan(plan: &ToolBatchPlan) -> String {
    hash_cbor(plan)
}

fn pending_effect_from_params<T: serde::Serialize>(
    state: &SessionState,
    effect_kind: &'static str,
    params: &T,
    cap_slot: Option<&str>,
) -> PendingEffect {
    let cap_slot = cap_slot.map(ToString::to_string);
    PendingEffect::from_params(effect_kind, params, cap_slot.clone(), state.updated_at)
        .unwrap_or_else(|_| {
            PendingEffect::new(effect_kind, String::new(), cap_slot, state.updated_at)
        })
}

fn begin_pending_effect<T: serde::Serialize>(
    state: &mut SessionState,
    effect_kind: &'static str,
    params: &T,
    cap_slot: Option<&str>,
) -> String {
    let cap_slot = cap_slot.map(ToString::to_string);
    match state
        .pending_effects
        .begin(effect_kind, params, cap_slot.clone(), state.updated_at)
    {
        Ok(pending) => pending.params_hash,
        Err(_) => {
            let pending =
                PendingEffect::new(effect_kind, String::new(), cap_slot, state.updated_at);
            let params_hash = pending.params_hash.clone();
            state.pending_effects.insert(pending);
            params_hash
        }
    }
}

fn hash_cbor<T: serde::Serialize>(value: &T) -> String {
    aos_wasm_sdk::effect_params_hash(value).unwrap_or_default()
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
        receipt_event_with_issuer_ref(
            emitted_at_seq,
            effect_kind,
            params_hash,
            None,
            status,
            payload,
        )
    }

    fn receipt_event_with_issuer_ref<T: serde::Serialize>(
        emitted_at_seq: u64,
        effect_kind: &str,
        params_hash: Option<String>,
        issuer_ref: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Receipt(aos_wasm_sdk::EffectReceiptEnvelope {
            origin_module_id: "aos.agent/SessionWorkflow@1".into(),
            origin_instance_key: None,
            intent_id: fake_hash('i'),
            effect_kind: effect_kind.into(),
            params_hash,
            issuer_ref,
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
    fn run_request_auto_opens_host_session_when_missing() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert_eq!(out.effects.len(), 1);
        assert!(matches!(
            out.effects.first(),
            Some(SessionEffectCommand::ToolEffect {
                kind: ToolEffectKind::HostSessionOpen,
                ..
            })
        ));
        assert_eq!(state.pending_effects.len(), 1);
        assert!(state.pending_blob_puts.is_empty());
        assert!(state.queued_llm_message_refs.is_some());
        assert_eq!(state.in_flight_effects, 1);
        assert_eq!(state.effective_tools.profile_id, "openai");
        assert!(state.effective_tools.ordered_tools.is_empty());
        assert!(state.effective_tools.profile_requires_host_session);
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
        assert!(tools.contains(&"shell"));
        assert!(tools.contains(&"apply_patch"));
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
                tool_name: "write_file".into(),
                arguments_json: "{\"path\":\"a.txt\",\"text\":\"x\"}".into(),
                arguments_ref: Some(fake_hash('w')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c2".into(),
                tool_name: "apply_patch".into(),
                arguments_json: "{\"patch\":\"*** Begin Patch\\n*** End Patch\\n\"}".into(),
                arguments_ref: Some(fake_hash('p')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "c3".into(),
                tool_name: "shell".into(),
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
        apply_session_workflow_event(
            &mut state,
            &ingress(
                0,
                SessionIngressKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        let blob_put_hashes = out
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
        assert!(!blob_put_hashes.is_empty(), "expected blob.put effects");

        let mut last_out = SessionReduceOutput::default();
        for (idx, hash) in blob_put_hashes.iter().enumerate() {
            last_out = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    2 + idx as u64,
                    "blob.put",
                    Some(hash.clone()),
                    "ok",
                    &BlobPutReceipt {
                        blob_ref: hash_ref_for_index(idx),
                        edge_ref: hash_ref_for_index(idx + 1),
                        size: 42,
                    },
                ),
            )
            .expect("blob.put receipt");
        }

        assert!(
            last_out
                .effects
                .iter()
                .any(|effect| matches!(effect, SessionEffectCommand::LlmGenerate { .. }))
        );
        assert_eq!(state.pending_effects.len(), 1);
    }

    #[test]
    fn llm_tool_calls_are_resolved_executed_and_queued_for_follow_up() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        apply_session_workflow_event(
            &mut state,
            &ingress(
                0,
                SessionIngressKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");

        let out1 = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        let tool_def_put_hashes = out1
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
        assert!(
            !tool_def_put_hashes.is_empty(),
            "expected initial blob.put effects"
        );
        let mut out2 = SessionReduceOutput::default();
        for (idx, hash) in tool_def_put_hashes.iter().enumerate() {
            out2 = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    2 + idx as u64,
                    "blob.put",
                    Some(hash.clone()),
                    "ok",
                    &BlobPutReceipt {
                        blob_ref: hash_ref_for_index(idx),
                        edge_ref: hash_ref_for_index(idx + 1),
                        size: 111,
                    },
                ),
            )
            .expect("tool def put receipt");
        }
        let llm_params_hash = out2
            .effects
            .iter()
            .find_map(|effect| {
                if let SessionEffectCommand::LlmGenerate { params_hash, .. } = effect {
                    Some(params_hash.clone())
                } else {
                    None
                }
            })
            .expect("expected llm.generate");

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
            tool_name: "shell".into(),
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

        let args_bytes = br#"{"argv":["pwd"],"output_mode":"require_inline"}"#.to_vec();
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
        let (tool_effect_hash, tool_effect_issuer_ref) = out6
            .effects
            .iter()
            .find_map(|effect| {
                if let SessionEffectCommand::ToolEffect {
                    params_hash,
                    issuer_ref,
                    ..
                } = effect
                {
                    Some((params_hash.clone(), issuer_ref.clone()))
                } else {
                    None
                }
            })
            .expect("expected tool effect");

        let out7 = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                7,
                "host.exec",
                Some(tool_effect_hash),
                tool_effect_issuer_ref,
                "ok",
                &serde_json::json!({
                    "status": "ok",
                    "stdout": "/tmp\n"
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

    #[test]
    fn collect_blob_refs_from_output_json_finds_nested_blob_refs() {
        let output_json = serde_json::json!({
            "tool": "read_file",
            "ok": true,
            "result": {
                "content": {
                    "blob": {
                        "blob_ref": fake_hash('a'),
                        "size_bytes": 100
                    }
                },
                "entries": {
                    "blob": {
                        "blob_ref": fake_hash('b'),
                        "size_bytes": 200
                    }
                }
            }
        })
        .to_string();

        let refs = collect_blob_refs_from_output_json(&output_json);
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|value| value == &fake_hash('a')));
        assert!(refs.iter().any(|value| value == &fake_hash('b')));
    }

    #[test]
    fn inject_blob_inline_text_into_output_json_adds_inline_payload() {
        let blob_ref = fake_hash('c');
        let output_json = serde_json::json!({
            "tool": "glob",
            "ok": true,
            "result": {
                "paths": {
                    "blob": {
                        "blob_ref": blob_ref,
                        "size_bytes": 300
                    }
                }
            }
        })
        .to_string();

        let updated = inject_blob_inline_text_into_output_json(
            output_json.as_str(),
            blob_ref.as_str(),
            "alpha\nbeta",
            true,
            None,
        )
        .expect("blob inline injection");
        let value = serde_json::from_str::<serde_json::Value>(&updated).expect("decode json");
        let inline_text = value
            .get("result")
            .and_then(|v| v.get("paths"))
            .and_then(|v| v.get("inline_text"))
            .and_then(|v| v.get("text"))
            .and_then(serde_json::Value::as_str)
            .expect("inline text");
        assert_eq!(inline_text, "alpha\nbeta");
        let truncated = value
            .get("result")
            .and_then(|v| v.get("paths"))
            .and_then(|v| v.get("inline_text_truncated"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        assert!(truncated);
    }
}
