use super::{
    ContextError, ContextRequest, DefaultContextEngine, LlmStepContext, RequestLlm,
    SessionEffectCommand, SessionReduceOutput, allocate_run_id, can_apply_host_command,
    enqueue_host_text, pop_follow_up_if_ready, request_llm, transition_lifecycle,
};
use crate::contracts::{
    ContextBudget, ContextObservation, ContextPlan, EffectiveTool, EffectiveToolSet,
    HostCommandKind, HostSessionOpenConfig, HostTargetConfig, PendingBlobGetKind,
    PendingBlobPutKind, PendingFollowUpTurn, RunCause, RunConfig, RunFailure, RunLifecycle,
    RunOutcome, RunRecord, RunState, SessionConfig, SessionIngressKind, SessionLifecycle,
    SessionState, SessionStatus, SessionWorkflowEvent, ToolAvailabilityRule, ToolBatchPlan,
    ToolCallStatus, ToolOverrideScope, ToolSpec,
};
use crate::helpers::ContextEngine;
use crate::tools::{ToolEffectOp, map_tool_receipt_to_llm_result};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use aos_effects::builtins::{
    HostLocalTarget, HostMount, HostSandboxTarget, HostSessionOpenParams, HostTarget,
    LlmGenerateReceipt,
};
use aos_wasm_sdk::{EffectReceiptEnvelope, EffectReceiptRejected};

mod blob_effects;
mod tool_batch;
mod types;

use self::blob_effects::{
    enqueue_blob_get, enqueue_blob_put, handle_pending_blob_get_receipt,
    handle_pending_blob_put_receipt, has_pending_tool_definition_puts,
};
use self::types::pending_effect_lookup_err_to_session_err;
pub use self::types::{
    CompletedToolBatch, RunToolBatch, RunToolBatchResult, SessionReduceError, SessionRuntimeLimits,
    StartedToolBatch, ToolBatchReceiptMatch,
};

const TOOL_RESULT_BLOB_MAX_BYTES: usize = 8 * 1024;

pub fn run_tool_batch(
    state: &mut SessionState,
    request: RunToolBatch<'_>,
    out: &mut SessionReduceOutput,
) -> Result<RunToolBatchResult, SessionReduceError> {
    let started = tool_batch::run_tool_batch(state, request, out)?;
    let completed = tool_batch::take_completed_tool_batch(state);
    let completion = handle_completed_tool_batch(state, completed, out)?;
    Ok(RunToolBatchResult {
        started,
        completion,
    })
}

pub fn continue_tool_batch(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<Option<CompletedToolBatch>, SessionReduceError> {
    let completion = tool_batch::advance_tool_batch(state, out)?;
    handle_completed_tool_batch(state, completion, out)
}

pub fn settle_tool_batch_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<ToolBatchReceiptMatch, SessionReduceError> {
    match tool_batch::settle_tool_batch_receipt(state, envelope, out)? {
        ToolBatchReceiptMatch::Unmatched => Ok(ToolBatchReceiptMatch::Unmatched),
        ToolBatchReceiptMatch::Matched { completion } => Ok(ToolBatchReceiptMatch::Matched {
            completion: handle_completed_tool_batch(state, completion, out)?,
        }),
    }
}

fn transition_to_waiting_input_if_running(
    state: &mut SessionState,
) -> Result<(), SessionReduceError> {
    if matches!(state.lifecycle, SessionLifecycle::Running) {
        transition_lifecycle(state, SessionLifecycle::WaitingInput)
            .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    }
    if let Some(run) = state.current_run.as_mut()
        && matches!(run.lifecycle, RunLifecycle::Running)
    {
        run.lifecycle = RunLifecycle::WaitingInput;
        run.updated_at = state.updated_at;
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
                    let cause = RunCause::direct_input(input_ref.clone());
                    validate_run_request_catalog(
                        state,
                        run_overrides.as_ref(),
                        allowed_providers,
                        allowed_models,
                    )?;
                    on_run_start_requested(state, &cause, run_overrides.as_ref(), &mut out)?;
                }
                SessionIngressKind::RunStartRequested {
                    cause,
                    run_overrides,
                } => {
                    validate_run_request_catalog(
                        state,
                        run_overrides.as_ref(),
                        allowed_providers,
                        allowed_models,
                    )?;
                    on_run_start_requested(state, cause, run_overrides.as_ref(), &mut out)?;
                }
                SessionIngressKind::SessionOpened { config } => {
                    on_session_opened(state, config.as_ref())?;
                }
                SessionIngressKind::SessionConfigUpdated { config } => {
                    state.session_config = config.clone();
                    let active = state.active_run_config.clone();
                    refresh_effective_tools(state, active.as_ref())?;
                }
                SessionIngressKind::SessionPaused => {
                    transition_session_status(state, SessionStatus::Paused)?;
                }
                SessionIngressKind::SessionResumed => {
                    transition_session_status(state, SessionStatus::Open)?;
                }
                SessionIngressKind::SessionArchived => {
                    transition_session_status(state, SessionStatus::Archived)?;
                }
                SessionIngressKind::SessionExpired => {
                    transition_session_status(state, SessionStatus::Expired)?;
                }
                SessionIngressKind::SessionClosed => {
                    transition_session_status(state, SessionStatus::Closed)?;
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
                SessionIngressKind::ContextObserved(observation) => {
                    on_context_observed(state, observation);
                }
                SessionIngressKind::HostSessionUpdated {
                    host_session_id,
                    host_session_status,
                } => {
                    on_host_session_updated(state, host_session_id.as_ref(), *host_session_status)?
                }
                SessionIngressKind::RunCompleted => {
                    finish_current_run(
                        state,
                        RunLifecycle::Completed,
                        Some(RunOutcome {
                            output_ref: state.last_output_ref.clone(),
                            failure: None,
                            cancelled_reason: None,
                        }),
                    );
                    transition_lifecycle(state, SessionLifecycle::Completed)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                }
                SessionIngressKind::RunFailed { code, detail } => {
                    finish_current_run(
                        state,
                        RunLifecycle::Failed,
                        Some(RunOutcome {
                            output_ref: state.last_output_ref.clone(),
                            failure: Some(RunFailure {
                                code: code.clone(),
                                detail: detail.clone(),
                            }),
                            cancelled_reason: None,
                        }),
                    );
                    transition_lifecycle(state, SessionLifecycle::Failed)
                        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                }
                SessionIngressKind::RunCancelled { reason } => {
                    finish_current_run(
                        state,
                        RunLifecycle::Cancelled,
                        Some(RunOutcome {
                            output_ref: state.last_output_ref.clone(),
                            failure: None,
                            cancelled_reason: reason.clone(),
                        }),
                    );
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

fn on_session_opened(
    state: &mut SessionState,
    config: Option<&SessionConfig>,
) -> Result<(), SessionReduceError> {
    if let Some(config) = config {
        state.session_config = config.clone();
    }
    transition_session_status(state, SessionStatus::Open)
}

fn transition_session_status(
    state: &mut SessionState,
    next: SessionStatus,
) -> Result<(), SessionReduceError> {
    if state.status == next {
        return Ok(());
    }
    if state.active_run_id.is_some()
        && matches!(
            next,
            SessionStatus::Archived | SessionStatus::Expired | SessionStatus::Closed
        )
    {
        return Err(SessionReduceError::RunAlreadyActive);
    }
    state.status = next;
    Ok(())
}

fn on_run_start_requested(
    state: &mut SessionState,
    cause: &RunCause,
    run_overrides: Option<&SessionConfig>,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if !state.status.accepts_new_runs() {
        return Err(SessionReduceError::InvalidLifecycleTransition);
    }
    if state.active_run_id.is_some() {
        return Err(SessionReduceError::RunAlreadyActive);
    }
    if cause.input_refs.is_empty() {
        return Err(SessionReduceError::EmptyMessageRefs);
    }

    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_config(&requested)?;

    transition_lifecycle(state, SessionLifecycle::Running)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;

    let run_id = allocate_run_id(state);
    state.active_run_id = Some(run_id.clone());
    state.active_run_config = Some(requested.clone());
    state.current_run = Some(RunState {
        run_id: run_id.clone(),
        lifecycle: RunLifecycle::Running,
        cause: cause.clone(),
        config: requested.clone(),
        input_refs: cause.input_refs.clone(),
        context_plan: None,
        active_tool_batch: None,
        pending_effects: aos_wasm_sdk::PendingEffects::new(),
        pending_blob_gets: aos_wasm_sdk::SharedBlobGets::new(),
        pending_blob_puts: aos_wasm_sdk::SharedBlobPuts::new(),
        pending_follow_up_turn: None,
        queued_llm_message_refs: None,
        last_output_ref: None,
        tool_refs_materialized: false,
        in_flight_effects: 0,
        outcome: None,
        started_at: state.updated_at,
        updated_at: state.updated_at,
    });
    state.active_tool_batch = None;
    state.pending_blob_gets.clear();
    state.pending_blob_puts.clear();
    state.pending_follow_up_turn = None;
    state.queued_llm_message_refs = None;
    state.last_output_ref = None;
    state.tool_refs_materialized = false;

    refresh_effective_tools(state, Some(&requested))?;
    state
        .transcript_message_refs
        .extend(cause.input_refs.iter().cloned());
    state.conversation_message_refs = state.transcript_message_refs.clone();
    queue_llm_turn(state, state.conversation_message_refs.clone(), out)
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
            transition_session_status(state, SessionStatus::Paused)?;
            transition_lifecycle(state, SessionLifecycle::Paused)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Resume => {
            transition_session_status(state, SessionStatus::Open)?;
            transition_lifecycle(state, SessionLifecycle::Running)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Cancel { .. } => {
            transition_lifecycle(state, SessionLifecycle::Cancelling)
                .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
            finish_current_run(
                state,
                RunLifecycle::Cancelled,
                Some(RunOutcome {
                    output_ref: state.last_output_ref.clone(),
                    failure: None,
                    cancelled_reason: Some("host command cancel".into()),
                }),
            );
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

fn on_context_observed(state: &mut SessionState, observation: &ContextObservation) {
    match observation {
        ContextObservation::SummaryCompleted {
            summary_ref,
            input_refs: _,
        } => {
            if !state
                .context_state
                .summary_refs
                .iter()
                .any(|existing| existing == summary_ref)
            {
                state.context_state.summary_refs.push(summary_ref.clone());
            }
        }
        ContextObservation::InputPinned(input) => {
            state
                .context_state
                .pinned_inputs
                .retain(|existing| existing.input_id != input.input_id);
            state.context_state.pinned_inputs.push(input.clone());
        }
        ContextObservation::InputRemoved { input_id } => {
            state
                .context_state
                .pinned_inputs
                .retain(|existing| existing.input_id != *input_id);
        }
        ContextObservation::Noop => {}
    }
}

fn handle_standalone_host_session_open_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    if envelope.effect != "sys/host.session.open@1" {
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

    state.last_output_ref = Some(receipt.output_ref.as_str().into());

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
        origin_workflow_hash: rejected.origin_workflow_hash.clone(),
        origin_instance_key: rejected.origin_instance_key.clone(),
        intent_id: rejected.intent_id.clone(),
        effect: rejected.effect.clone(),
        effect_hash: rejected.effect_hash.clone(),
        executor_module: rejected.executor_module.clone(),
        executor_module_hash: rejected.executor_module_hash.clone(),
        executor_entrypoint: rejected.executor_entrypoint.clone(),
        params_hash: rejected.params_hash.clone(),
        issuer_ref: rejected.issuer_ref.clone(),
        receipt_payload: serde_cbor::to_vec(&payload).unwrap_or_default(),
        status: rejected.status.clone(),
        emitted_at_seq: rejected.emitted_at_seq,
        cost_cents: None,
        signature: Vec::new(),
    }
}

fn on_receipt_envelope(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if handle_pending_blob_get_receipt(state, envelope, out)?
        || handle_pending_blob_put_receipt(state, envelope, out)?
    {
        recompute_in_flight_effects(state);
        return Ok(());
    }

    if let Some(matched) = state.pending_effects.settle(envelope.into()) {
        match matched.pending.effect.as_str() {
            "sys/host.session.open@1" => {
                let _ = handle_standalone_host_session_open_receipt(state, envelope, out)?;
            }
            "sys/llm.generate@1" => handle_llm_generate_receipt(state, envelope, out)?,
            _ => {}
        }
        recompute_in_flight_effects(state);
        return Ok(());
    }

    match settle_tool_batch_receipt(state, envelope, out)? {
        ToolBatchReceiptMatch::Unmatched => {}
        ToolBatchReceiptMatch::Matched { .. } => {
            recompute_in_flight_effects(state);
            return Ok(());
        }
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
    if handle_pending_blob_get_receipt(state, &envelope, out)?
        || handle_pending_blob_put_receipt(state, &envelope, out)?
    {
        recompute_in_flight_effects(state);
        return Ok(());
    }

    if let Some(matched) = state.pending_effects.settle(rejected.into()) {
        match matched.pending.effect.as_str() {
            "sys/host.session.open@1" => {
                let _ = handle_standalone_host_session_open_receipt(state, &envelope, out)?;
            }
            "sys/llm.generate@1" => fail_run(state)?,
            _ => {}
        }
        recompute_in_flight_effects(state);
        return Ok(());
    }

    match settle_tool_batch_receipt(state, &envelope, out)? {
        ToolBatchReceiptMatch::Unmatched => fail_run(state)?,
        ToolBatchReceiptMatch::Matched { .. } => {}
    }
    recompute_in_flight_effects(state);
    Ok(())
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

    let total = state.pending_effects.len()
        + state.pending_blob_gets.len()
        + state.pending_blob_puts.len()
        + pending_tool_effect_receipts
        + pending_host_loop_calls;
    state.in_flight_effects = total as u64;
    sync_current_run_execution(state);
}

fn sync_current_run_execution(state: &mut SessionState) {
    let Some(run) = state.current_run.as_mut() else {
        return;
    };
    run.active_tool_batch = state.active_tool_batch.clone();
    run.pending_effects = state.pending_effects.clone();
    run.pending_blob_gets = state.pending_blob_gets.clone();
    run.pending_blob_puts = state.pending_blob_puts.clone();
    run.pending_follow_up_turn = state.pending_follow_up_turn.clone();
    run.queued_llm_message_refs = state.queued_llm_message_refs.clone();
    run.last_output_ref = state.last_output_ref.clone();
    run.tool_refs_materialized = state.tool_refs_materialized;
    run.in_flight_effects = state.in_flight_effects;
    run.updated_at = state.updated_at;
}

fn fail_run(state: &mut SessionState) -> Result<(), SessionReduceError> {
    finish_current_run(
        state,
        RunLifecycle::Failed,
        Some(RunOutcome {
            output_ref: state.last_output_ref.clone(),
            failure: Some(RunFailure {
                code: "run_failed".into(),
                detail: "run failed while handling effect receipt".into(),
            }),
            cancelled_reason: None,
        }),
    );
    transition_lifecycle(state, SessionLifecycle::Failed)
        .map_err(|_| SessionReduceError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    Ok(())
}

fn handle_completed_tool_batch(
    state: &mut SessionState,
    completion: Option<CompletedToolBatch>,
    out: &mut SessionReduceOutput,
) -> Result<Option<CompletedToolBatch>, SessionReduceError> {
    let Some(completion) = completion else {
        return Ok(None);
    };

    let messages = build_tool_batch_follow_up_messages(&completion);
    if messages.is_empty() {
        transition_to_waiting_input_if_running(state)?;
        return Ok(Some(completion));
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
        tool_batch_id: completion.tool_batch_id.clone(),
        base_message_refs: state.conversation_message_refs.clone(),
        expected_messages,
        blob_refs_by_index: BTreeMap::new(),
    });
    Ok(Some(completion))
}

fn build_tool_batch_follow_up_messages(completion: &CompletedToolBatch) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    if !completion.accepted_calls.is_empty() {
        let tool_calls = completion
            .accepted_calls
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

    for result in &completion.ordered_results {
        let output = serde_json::from_str::<serde_json::Value>(&result.output_json)
            .unwrap_or_else(|_| serde_json::Value::String(result.output_json.clone()));
        messages.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": result.call_id,
            "output": output,
            "is_error": result.is_error,
        }));
    }
    messages
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
    dispatch_queued_llm_turn_with_engine(
        state,
        out,
        &DefaultContextEngine,
        ContextBudget {
            max_refs: None,
            reserve_output_tokens: None,
        },
    )
}

pub fn dispatch_queued_llm_turn_with_engine<E: ContextEngine>(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
    engine: &E,
    budget: ContextBudget,
) -> Result<(), SessionReduceError> {
    if state.queued_llm_message_refs.is_none() {
        return Ok(());
    }
    if !state.pending_effects.is_empty()
        || !state.pending_blob_gets.is_empty()
        || !state.pending_blob_puts.is_empty()
        || state.pending_follow_up_turn.is_some()
    {
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
    let run_seq = state
        .active_run_id
        .as_ref()
        .map(|id| id.run_seq)
        .unwrap_or(0);
    let planned_message_refs =
        build_context_for_turn_with_engine(state, message_refs, engine, budget)?;

    request_llm(
        state,
        out,
        RequestLlm {
            step: LlmStepContext {
                correlation_id: Some(alloc::format!(
                    "run-{run_seq}-turn-{}",
                    state.next_tool_batch_seq + 1
                )),
                message_refs: planned_message_refs,
                temperature: None,
                top_p: None,
                tool_refs: state.effective_tools.tool_refs(),
                tool_choice: Some(aos_effects::builtins::LlmToolChoice::Auto),
                stop_sequences: None,
                metadata: None,
                provider_options_ref: None,
                response_format_ref: None,
                api_key: None,
            },
        },
    )?;
    Ok(())
}

pub fn build_context_for_turn_with_engine<E: ContextEngine>(
    state: &mut SessionState,
    turn_refs: Vec<String>,
    engine: &E,
    budget: ContextBudget,
) -> Result<Vec<String>, SessionReduceError> {
    let (run_id, prompt_refs, cause) = {
        let run = state
            .current_run
            .as_ref()
            .ok_or(SessionReduceError::RunNotActive)?;
        (
            run.run_id.clone(),
            run.config.prompt_refs.clone().unwrap_or_default(),
            run.cause.clone(),
        )
    };
    let transcript_refs = state
        .transcript_message_refs
        .iter()
        .filter(|value| !turn_refs.iter().any(|turn| turn == *value))
        .cloned()
        .collect::<Vec<_>>();
    let plan = engine
        .build_plan(ContextRequest {
            session_id: &state.session_id,
            run_id: &run_id,
            run_cause: Some(&cause),
            budget,
            session_context: &state.context_state,
            prompt_refs: &prompt_refs,
            transcript_refs: &transcript_refs,
            turn_refs: &turn_refs,
        })
        .map_err(context_error_to_reduce_error)?;
    apply_context_plan_to_state(state, plan)
}

fn apply_context_plan_to_state(
    state: &mut SessionState,
    plan: ContextPlan,
) -> Result<Vec<String>, SessionReduceError> {
    if plan.selected_refs.is_empty() {
        return Err(SessionReduceError::EmptyMessageRefs);
    }
    let selected_refs = plan.selected_refs.clone();
    state.context_state.last_report = Some(plan.report.clone());
    if let Some(run) = state.current_run.as_mut() {
        run.context_plan = Some(plan);
    }
    Ok(selected_refs)
}

fn context_error_to_reduce_error(err: ContextError) -> SessionReduceError {
    match err {
        ContextError::EmptySelection => SessionReduceError::EmptyMessageRefs,
    }
}

fn should_auto_open_host_session(state: &SessionState) -> bool {
    if state.active_run_id.is_none() {
        return false;
    }
    if effective_host_session_open_config(state).is_none() {
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
        .contains_effect("sys/host.session.open@1")
}

fn emit_auto_host_session_open(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    let config =
        effective_host_session_open_config(state).ok_or(SessionReduceError::InvalidToolRegistry)?;
    let params = host_session_open_params_from_config(config);
    let params_json =
        serde_json::to_value(&params).map_err(|_| SessionReduceError::InvalidToolRegistry)?;
    out.effects.push(SessionEffectCommand::ToolEffect {
        kind: ToolEffectOp::HostSessionOpen,
        params_json: serde_json::to_string(&params_json).unwrap_or_else(|_| "{}".into()),
        pending: super::begin_pending_effect(state, "sys/host.session.open@1", &params_json, None),
    });
    Ok(())
}

fn effective_host_session_open_config(state: &SessionState) -> Option<&HostSessionOpenConfig> {
    state
        .active_run_config
        .as_ref()
        .and_then(|config| config.host_session_open.as_ref())
        .or_else(|| state.session_config.default_host_session_open.as_ref())
}

fn host_session_open_params_from_config(config: &HostSessionOpenConfig) -> HostSessionOpenParams {
    HostSessionOpenParams {
        target: host_target_from_config(&config.target),
        session_ttl_ns: config.session_ttl_ns,
        labels: config.labels.clone(),
    }
}

fn host_target_from_config(config: &HostTargetConfig) -> HostTarget {
    match config {
        HostTargetConfig::Local {
            mounts,
            workdir,
            env,
            network_mode,
        } => HostTarget::local(HostLocalTarget {
            mounts: mounts.as_ref().map(|items| {
                items
                    .iter()
                    .map(|mount| HostMount {
                        host_path: mount.host_path.clone(),
                        guest_path: mount.guest_path.clone(),
                        mode: mount.mode.clone(),
                    })
                    .collect()
            }),
            workdir: workdir.clone(),
            env: env.clone(),
            network_mode: network_mode.clone().unwrap_or_else(|| "none".into()),
        }),
        HostTargetConfig::Sandbox {
            image,
            runtime_class,
            workdir,
            env,
            network_mode,
            mounts,
            cpu_limit_millis,
            memory_limit_bytes,
        } => HostTarget::sandbox(HostSandboxTarget {
            image: image.clone(),
            runtime_class: runtime_class.clone(),
            workdir: workdir.clone(),
            env: env.clone(),
            network_mode: network_mode.clone(),
            mounts: mounts.as_ref().map(|items| {
                items
                    .iter()
                    .map(|mount| HostMount {
                        host_path: mount.host_path.clone(),
                        guest_path: mount.guest_path.clone(),
                        mode: mount.mode.clone(),
                    })
                    .collect()
            }),
            cpu_limit_millis: *cpu_limit_millis,
            memory_limit_bytes: *memory_limit_bytes,
        }),
    }
}

fn refresh_effective_tools(
    state: &mut SessionState,
    run_config: Option<&RunConfig>,
) -> Result<(), SessionReduceError> {
    let configured_profile_id = run_config
        .and_then(|cfg| cfg.tool_profile.clone())
        .or_else(|| state.session_config.default_tool_profile.clone())
        .or_else(|| {
            if state.tool_profile.trim().is_empty() {
                None
            } else {
                Some(state.tool_profile.clone())
            }
        });

    let base_profile = if let Some(profile_id) = configured_profile_id.as_ref() {
        let base_profile = state
            .tool_profiles
            .get(profile_id)
            .ok_or(SessionReduceError::ToolProfileUnknown)?;
        validate_known_tool_names(state, Some(base_profile.as_slice()))?;
        base_profile.as_slice()
    } else {
        &[]
    };

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

    let profile_id = configured_profile_id.unwrap_or_default();
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
        host_session_open: source.default_host_session_open.clone(),
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

fn finish_current_run(
    state: &mut SessionState,
    lifecycle: RunLifecycle,
    outcome: Option<RunOutcome>,
) {
    let Some(mut run) = state.current_run.take() else {
        return;
    };
    run.lifecycle = lifecycle;
    run.updated_at = state.updated_at;
    run.outcome = outcome.clone();
    state.run_history.push(RunRecord {
        run_id: run.run_id,
        lifecycle,
        cause: run.cause,
        input_refs: run.input_refs,
        outcome,
        started_at: run.started_at,
        ended_at: state.updated_at,
    });
}

fn clear_active_run(state: &mut SessionState) {
    state.active_run_id = None;
    state.active_run_config = None;
    state.current_run = None;
    state.active_tool_batch = None;
    state.pending_effects.clear();
    state.pending_blob_gets.clear();
    state.pending_blob_puts.clear();
    state.pending_follow_up_turn = None;
    state.queued_llm_message_refs = None;
    state.tool_refs_materialized = false;
    state.in_flight_effects = 0;
}

fn enforce_runtime_limits(
    state: &SessionState,
    limits: SessionRuntimeLimits,
) -> Result<(), SessionReduceError> {
    if let Some(max) = limits.max_pending_effects {
        let total_pending = state.pending_effects.len()
            + state.pending_blob_gets.len()
            + state.pending_blob_puts.len();
        if total_pending as u64 > max {
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

fn hash_cbor<T: serde::Serialize>(value: &T) -> String {
    aos_wasm_sdk::effect_params_hash(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        CauseRef, ContextInput, ContextInputKind, ContextInputScope, ContextPriority,
        ContextReport, ContextSelection, HostSessionOpenConfig, HostSessionStatus,
        HostTargetConfig, RunCauseOrigin, RunId, SessionId, SessionIngress, ToolCallObserved,
        ToolOverrideScope, ToolProfileBuilder, ToolRegistryBuilder,
        local_coding_agent_tool_profile_for_provider, local_coding_agent_tool_profiles,
        local_coding_agent_tool_registry, tool_bundle_host_sandbox,
    };
    use alloc::string::ToString;
    use alloc::vec;
    use aos_air_types::HashRef;
    use aos_effect_types::workspace::{
        WorkspaceEmptyRootReceipt, WorkspaceResolveReceipt, WorkspaceWriteRefReceipt,
    };
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
        effect: &str,
        params_hash: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        receipt_event_with_issuer_ref(emitted_at_seq, effect, params_hash, None, status, payload)
    }

    fn receipt_event_with_issuer_ref<T: serde::Serialize>(
        emitted_at_seq: u64,
        effect: &str,
        params_hash: Option<String>,
        issuer_ref: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Receipt(aos_wasm_sdk::EffectReceiptEnvelope {
            origin_module_id: "aos.agent/SessionWorkflow@1".into(),
            origin_workflow_hash: None,
            origin_instance_key: None,
            intent_id: fake_hash('i'),
            effect: effect.into(),
            effect_hash: None,
            executor_module: None,
            executor_module_hash: None,
            executor_entrypoint: None,
            params_hash,
            issuer_ref,
            receipt_payload: serde_cbor::to_vec(payload).expect("encode payload"),
            status: status.into(),
            emitted_at_seq,
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
                    default_host_session_open: None,
                }),
            },
        )
    }

    fn local_coding_state() -> SessionState {
        let mut state = SessionState::default();
        state.tool_registry = local_coding_agent_tool_registry();
        state.tool_profiles = local_coding_agent_tool_profiles();
        state.tool_profile = local_coding_agent_tool_profile_for_provider("openai");
        state.session_config.default_host_session_open = Some(local_host_session_open_config());
        state
    }

    fn local_host_session_open_config() -> HostSessionOpenConfig {
        HostSessionOpenConfig {
            target: HostTargetConfig::Local {
                mounts: None,
                workdir: None,
                env: None,
                network_mode: Some("none".into()),
            },
            session_ttl_ns: None,
            labels: None,
        }
    }

    fn sandbox_host_session_open_config() -> HostSessionOpenConfig {
        HostSessionOpenConfig {
            target: HostTargetConfig::Sandbox {
                image: "aos-agent-test:latest".into(),
                runtime_class: Some("fabric".into()),
                workdir: Some("/workspace".into()),
                env: None,
                network_mode: Some("none".into()),
                mounts: None,
                cpu_limit_millis: Some(2000),
                memory_limit_bytes: Some(512 * 1024 * 1024),
            },
            session_ttl_ns: Some(60_000_000_000),
            labels: None,
        }
    }

    #[test]
    fn run_request_auto_opens_host_session_when_missing() {
        let mut state = local_coding_state();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert_eq!(out.effects.len(), 1);
        assert!(matches!(
            out.effects.first(),
            Some(SessionEffectCommand::ToolEffect { pending, .. })
                if pending.effect == "sys/host.session.open@1"
        ));
        assert_eq!(state.pending_effects.len(), 1);
        assert!(state.pending_blob_puts.is_empty());
        assert!(state.queued_llm_message_refs.is_some());
        assert_eq!(state.in_flight_effects, 1);
        assert_eq!(state.effective_tools.profile_id, "openai");
        let tools: Vec<&str> = state
            .effective_tools
            .ordered_tools
            .iter()
            .map(|tool| tool.tool_name.as_str())
            .collect();
        assert!(tools.contains(&"inspect_world"));
        assert!(tools.contains(&"inspect_workflow"));
        assert!(state.effective_tools.profile_requires_host_session);
    }

    #[test]
    fn run_request_does_not_auto_open_host_session_without_config() {
        let mut state = local_coding_state();
        state.session_config.default_host_session_open = None;
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
        assert!(!out.effects.iter().any(|effect| {
            matches!(
                effect,
                SessionEffectCommand::ToolEffect { pending, .. }
                    if pending.effect == "sys/host.session.open@1"
            )
        }));
        assert!(state.effective_tools.profile_requires_host_session);
    }

    #[test]
    fn run_request_auto_opens_sandbox_host_session_from_config() {
        let registry = ToolRegistryBuilder::new()
            .with_bundle(tool_bundle_host_sandbox())
            .build()
            .expect("registry");
        let profile = ToolProfileBuilder::new()
            .with_tool_id("host.exec")
            .build_for_registry(&registry)
            .expect("profile");

        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        state.tool_registry = registry;
        state.tool_profiles.insert("sandbox".into(), profile);
        state.tool_profile = "sandbox".into();
        state.session_config.default_host_session_open = Some(sandbox_host_session_open_config());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");
        let params_json = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::ToolEffect {
                    params_json,
                    pending,
                    ..
                } if pending.effect == "sys/host.session.open@1" => Some(params_json.as_str()),
                _ => None,
            })
            .expect("host session open effect");
        let params: serde_json::Value = serde_json::from_str(params_json).expect("params json");

        assert_eq!(params["target"]["$tag"], "sandbox");
        assert_eq!(params["target"]["$value"]["image"], "aos-agent-test:latest");
        assert_eq!(params["target"]["$value"]["runtime_class"], "fabric");
        assert_eq!(params["target"]["$value"]["workdir"], "/workspace");
        assert_eq!(params["session_ttl_ns"], 60_000_000_000_u64);
    }

    #[test]
    fn run_request_with_empty_registry_is_chat_only() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let out = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");

        assert!(state.effective_tools.ordered_tools.is_empty());
        assert!(!state.effective_tools.profile_requires_host_session);
        assert!(!out.effects.iter().any(|effect| {
            matches!(
                effect,
                SessionEffectCommand::ToolEffect { pending, .. }
                    if pending.effect == "sys/host.session.open@1"
            )
        }));
        assert!(out.effects.iter().any(|effect| {
            matches!(
                effect,
                SessionEffectCommand::LlmGenerate { params, .. }
                    if params.runtime.tool_refs.is_none()
            )
        }));
    }

    #[test]
    fn run_request_records_context_plan_and_preserves_prompt_refs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        let prompt_ref = fake_hash('b');
        let input_ref = fake_hash('a');

        let out = apply_session_workflow_event(
            &mut state,
            &ingress(
                1,
                SessionIngressKind::RunRequested {
                    input_ref: input_ref.clone(),
                    run_overrides: Some(SessionConfig {
                        provider: "openai".into(),
                        model: "gpt-5.2".into(),
                        reasoning_effort: None,
                        max_tokens: Some(512),
                        default_prompt_refs: Some(vec![prompt_ref.clone()]),
                        default_tool_profile: None,
                        default_tool_enable: None,
                        default_tool_disable: None,
                        default_tool_force: None,
                        default_host_session_open: None,
                    }),
                },
            ),
        )
        .expect("reduce");

        let params = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::LlmGenerate { params, .. } => Some(params),
                _ => None,
            })
            .expect("expected llm.generate");
        let message_refs = params
            .message_refs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(message_refs, vec![prompt_ref.clone(), input_ref.clone()]);

        let plan = state
            .current_run
            .as_ref()
            .and_then(|run| run.context_plan.as_ref())
            .expect("context plan");
        assert_eq!(plan.selected_refs, vec![prompt_ref, input_ref]);
        assert_eq!(plan.report.engine, "aos.agent/default");
        assert_eq!(state.context_state.last_report.as_ref(), Some(&plan.report));
    }

    struct RepoBootstrapFirstEngine;

    impl ContextEngine for RepoBootstrapFirstEngine {
        fn build_plan(&self, request: ContextRequest<'_>) -> Result<ContextPlan, ContextError> {
            let mut selected_refs = Vec::new();
            let mut selections = Vec::new();
            let mut selected_count = 0_u64;
            let mut dropped_count = 0_u64;

            for input in &request.session_context.pinned_inputs {
                let is_repo_bootstrap = input.source_kind.as_deref() == Some("repo_bootstrap")
                    || matches!(
                        &input.scope,
                        ContextInputScope::Custom { kind } if kind == "repo_bootstrap"
                    );
                if is_repo_bootstrap {
                    selected_refs.push(input.content_ref.clone());
                    selected_count = selected_count.saturating_add(1);
                    selections.push(ContextSelection {
                        input_id: input.input_id.clone(),
                        selected: true,
                        reason: "repo_bootstrap_first".into(),
                        content_ref: input.content_ref.clone(),
                    });
                    break;
                }
            }

            for (idx, value) in request.transcript_refs.iter().enumerate() {
                dropped_count = dropped_count.saturating_add(1);
                selections.push(ContextSelection {
                    input_id: alloc::format!("transcript:{idx}"),
                    selected: false,
                    reason: "custom_engine_ignored_transcript".into(),
                    content_ref: value.clone(),
                });
            }

            for (idx, value) in request.turn_refs.iter().enumerate() {
                selected_refs.push(value.clone());
                selected_count = selected_count.saturating_add(1);
                selections.push(ContextSelection {
                    input_id: alloc::format!("turn:{idx}"),
                    selected: true,
                    reason: "current_turn_required".into(),
                    content_ref: value.clone(),
                });
            }

            if selected_refs.is_empty() {
                return Err(ContextError::EmptySelection);
            }

            Ok(ContextPlan {
                selected_refs,
                selections,
                actions: Vec::new(),
                report: ContextReport {
                    engine: "test/repo-bootstrap-first".into(),
                    selected_count,
                    dropped_count,
                    budget: request.budget,
                    decisions: vec!["selected repo bootstrap before current turn".into()],
                    unresolved: Vec::new(),
                    compaction_recommended: false,
                    compaction_required: false,
                },
            })
        }
    }

    #[test]
    fn custom_context_engine_can_reuse_llm_dispatch() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let bootstrap_ref = fake_hash('b');
        let transcript_ref = fake_hash('c');
        let input_ref = fake_hash('a');
        state.context_state.pinned_inputs.push(ContextInput {
            input_id: "repo-bootstrap".into(),
            kind: ContextInputKind::WorkspaceRef,
            scope: ContextInputScope::Custom {
                kind: "repo_bootstrap".into(),
            },
            priority: ContextPriority::Required,
            content_ref: bootstrap_ref.clone(),
            label: Some("repo bootstrap".into()),
            source_kind: Some("repo_bootstrap".into()),
            source_id: Some("repo://main".into()),
            correlation_id: None,
        });

        let run_config = RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            max_tokens: Some(512),
            ..RunConfig::default()
        };
        let run_id = RunId {
            session_id: state.session_id.clone(),
            run_seq: 1,
        };
        let cause = RunCause::direct_input(input_ref.clone());
        state.active_run_id = Some(run_id.clone());
        state.active_run_config = Some(run_config.clone());
        state.current_run = Some(RunState {
            run_id,
            lifecycle: RunLifecycle::Running,
            cause,
            config: run_config,
            input_refs: vec![input_ref.clone()],
            ..RunState::default()
        });
        state.transcript_message_refs = vec![transcript_ref.clone(), input_ref.clone()];
        state.queued_llm_message_refs = Some(vec![input_ref.clone()]);

        let mut out = SessionReduceOutput::default();
        dispatch_queued_llm_turn_with_engine(
            &mut state,
            &mut out,
            &RepoBootstrapFirstEngine,
            ContextBudget {
                max_refs: Some(2),
                reserve_output_tokens: Some(128),
            },
        )
        .expect("dispatch");

        let params = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::LlmGenerate { params, .. } => Some(params),
                _ => None,
            })
            .expect("expected llm.generate");
        let message_refs = params
            .message_refs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(message_refs, vec![bootstrap_ref.clone(), input_ref.clone()]);

        let plan = state
            .current_run
            .as_ref()
            .and_then(|run| run.context_plan.as_ref())
            .expect("context plan");
        assert_eq!(plan.selected_refs, vec![bootstrap_ref, input_ref]);
        assert_eq!(plan.report.engine, "test/repo-bootstrap-first");
        assert_eq!(plan.report.dropped_count, 1);
        assert!(plan.selections.iter().any(|selection| {
            selection.content_ref == transcript_ref
                && !selection.selected
                && selection.reason == "custom_engine_ignored_transcript"
        }));
        assert_eq!(state.context_state.last_report.as_ref(), Some(&plan.report));
    }

    #[test]
    fn session_can_exist_with_no_runs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(
            &mut state,
            &ingress(1, SessionIngressKind::SessionOpened { config: None }),
        )
        .expect("open session");

        assert_eq!(state.status, SessionStatus::Open);
        assert!(state.current_run.is_none());
        assert!(state.run_history.is_empty());
        assert!(state.transcript_message_refs.is_empty());
    }

    #[test]
    fn session_preserves_transcript_across_sequential_runs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("first run");
        assert_eq!(state.transcript_message_refs, vec![fake_hash('a')]);
        assert!(state.current_run.is_some());
        apply_session_workflow_event(&mut state, &ingress(2, SessionIngressKind::RunCompleted))
            .expect("complete first run");

        let second_input = fake_hash('b');
        apply_session_workflow_event(
            &mut state,
            &ingress(
                3,
                SessionIngressKind::RunRequested {
                    input_ref: second_input.clone(),
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
                        default_host_session_open: None,
                    }),
                },
            ),
        )
        .expect("second run");

        assert_eq!(state.run_history.len(), 1);
        assert_eq!(state.next_run_seq, 2);
        assert_eq!(
            state.transcript_message_refs,
            vec![fake_hash('a'), second_input]
        );
        assert_eq!(
            state.conversation_message_refs,
            state.transcript_message_refs
        );
        assert_eq!(
            state.current_run.as_ref().map(|run| run.run_id.run_seq),
            Some(2)
        );
    }

    #[test]
    fn failed_run_can_be_followed_by_later_run() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        apply_session_workflow_event(
            &mut state,
            &ingress(
                2,
                SessionIngressKind::RunFailed {
                    code: "boom".into(),
                    detail: "failed".into(),
                },
            ),
        )
        .expect("fail run");
        assert!(state.current_run.is_none());
        assert_eq!(state.run_history[0].lifecycle, RunLifecycle::Failed);

        apply_session_workflow_event(&mut state, &run_request_event(3)).expect("later run");
        assert_eq!(state.status, SessionStatus::Open);
        assert_eq!(
            state.current_run.as_ref().map(|run| run.run_id.run_seq),
            Some(2)
        );
    }

    #[test]
    fn paused_and_closed_session_boundaries_are_separate_from_runs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &ingress(1, SessionIngressKind::SessionPaused))
            .expect("pause");
        assert_eq!(state.status, SessionStatus::Paused);
        assert!(state.current_run.is_none());
        assert!(apply_session_workflow_event(&mut state, &run_request_event(2)).is_err());

        apply_session_workflow_event(&mut state, &ingress(3, SessionIngressKind::SessionResumed))
            .expect("resume");
        apply_session_workflow_event(&mut state, &run_request_event(4)).expect("run");
        apply_session_workflow_event(&mut state, &ingress(5, SessionIngressKind::RunCompleted))
            .expect("complete");

        apply_session_workflow_event(&mut state, &ingress(6, SessionIngressKind::SessionClosed))
            .expect("close");
        assert_eq!(state.status, SessionStatus::Closed);
        assert!(apply_session_workflow_event(&mut state, &run_request_event(7)).is_err());
    }

    #[test]
    fn domain_event_cause_starts_normal_run() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        let input_ref = fake_hash('d');
        let event_ref = fake_hash('e');

        apply_session_workflow_event(
            &mut state,
            &ingress(
                1,
                SessionIngressKind::RunStartRequested {
                    cause: RunCause {
                        kind: "example/work_item_ready".into(),
                        origin: RunCauseOrigin::DomainEvent {
                            schema: "example/WorkItemReady@1".into(),
                            event_ref: Some(event_ref.clone()),
                            key: Some("work-1".into()),
                        },
                        input_refs: vec![input_ref.clone()],
                        payload_schema: Some("example/WorkItemReady@1".into()),
                        payload_ref: Some(event_ref),
                        subject_refs: vec![CauseRef {
                            kind: "example/work_item".into(),
                            id: "work-1".into(),
                            ref_: None,
                        }],
                    },
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
                        default_host_session_open: None,
                    }),
                },
            ),
        )
        .expect("domain run");

        let run = state.current_run.as_ref().expect("current run");
        assert_eq!(run.cause.kind, "example/work_item_ready");
        assert_eq!(run.input_refs, vec![input_ref]);
        assert_eq!(state.lifecycle, SessionLifecycle::Running);
    }

    #[test]
    fn host_session_ready_enables_host_fs_and_exec_tools() {
        let mut state = local_coding_state();
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
        let mut state = local_coding_state();
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
        let started = run_tool_batch(
            &mut state,
            RunToolBatch {
                intent_id: fake_hash('i').as_str(),
                params_hash: Some(&params_hash),
                calls: &calls,
            },
            &mut out,
        )
        .expect("plan");

        assert_eq!(started.started.plan.execution_plan.groups.len(), 2);
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
        let mut state = local_coding_state();
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
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
            .collect::<Vec<_>>();
        assert!(!blob_put_hashes.is_empty(), "expected blob.put effects");

        let mut last_out = SessionReduceOutput::default();
        for (idx, hash) in blob_put_hashes.iter().enumerate() {
            last_out = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    2 + idx as u64,
                    "sys/blob.put@1",
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
    fn llm_receipt_settles_by_issuer_ref_when_params_hash_differs() {
        let mut state = local_coding_state();
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
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
            .collect::<Vec<_>>();

        let mut llm_pending = None;
        for (idx, hash) in blob_put_hashes.iter().enumerate() {
            let out = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    2 + idx as u64,
                    "sys/blob.put@1",
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
            llm_pending = out.effects.iter().find_map(|effect| match effect {
                SessionEffectCommand::LlmGenerate { pending, .. } => Some(pending.clone()),
                _ => None,
            });
        }

        let llm_pending = llm_pending.expect("expected llm.generate");
        let issuer_ref = llm_pending
            .issuer_ref
            .clone()
            .expect("llm pending issuer_ref");
        assert_eq!(state.pending_effects.len(), 1);

        let out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                3,
                "sys/llm.generate@1",
                Some(fake_hash('z')),
                Some(issuer_ref),
                "ok",
                &LlmGenerateReceipt {
                    output_ref: hash_ref('e'),
                    raw_output_ref: None,
                    provider_response_id: None,
                    finish_reason: LlmFinishReason {
                        reason: "stop".into(),
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

        assert!(state.pending_effects.is_empty());
        assert_eq!(
            state.last_output_ref.as_deref(),
            Some(hash_ref('e').as_str())
        );
        assert!(matches!(
            out.effects.first(),
            Some(SessionEffectCommand::BlobGet { .. })
        ));
    }

    #[test]
    fn llm_tool_calls_are_resolved_executed_and_queued_for_follow_up() {
        let mut state = local_coding_state();
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
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
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
                    "sys/blob.put@1",
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
            .find(|effect| matches!(effect, SessionEffectCommand::LlmGenerate { .. }))
            .map(|effect| effect.params_hash().to_string())
            .expect("expected llm.generate");

        let out3 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                3,
                "sys/llm.generate@1",
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
        assert_eq!(
            state.last_output_ref.as_deref(),
            Some(hash_ref('e').as_str())
        );
        let output_blob_get_hash = match out3.effects.first() {
            Some(effect) if matches!(effect, SessionEffectCommand::BlobGet { .. }) => {
                effect.params_hash().to_string()
            }
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
                "sys/blob.get@1",
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
            Some(effect) if matches!(effect, SessionEffectCommand::BlobGet { .. }) => {
                effect.params_hash().to_string()
            }
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
                "sys/blob.get@1",
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
            Some(effect) if matches!(effect, SessionEffectCommand::BlobGet { .. }) => {
                effect.params_hash().to_string()
            }
            _ => panic!("expected blob.get for tool arguments"),
        };

        let args_bytes = br#"{"argv":["pwd"],"output_mode":"require_inline"}"#.to_vec();
        let out6 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                6,
                "sys/blob.get@1",
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
                if let SessionEffectCommand::ToolEffect { pending, .. } = effect {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                } else {
                    None
                }
            })
            .expect("expected tool effect");

        let out7 = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                7,
                "sys/host.exec@1",
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
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
            .collect::<Vec<_>>();
        assert!(!followup_hashes.is_empty());

        let mut emitted_llm = false;
        for (idx, hash) in followup_hashes.iter().enumerate() {
            let out = apply_session_workflow_event(
                &mut state,
                &receipt_event(
                    8 + idx as u64,
                    "sys/blob.put@1",
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
    fn llm_no_tools_completion_keeps_last_output_ref_for_lifecycle_event() {
        let mut state = local_coding_state();
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
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
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
                    "sys/blob.put@1",
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
            .find(|effect| matches!(effect, SessionEffectCommand::LlmGenerate { .. }))
            .map(|effect| effect.params_hash().to_string())
            .expect("expected llm.generate");

        let out3 = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                3,
                "sys/llm.generate@1",
                Some(llm_params_hash),
                "ok",
                &LlmGenerateReceipt {
                    output_ref: hash_ref('e'),
                    raw_output_ref: None,
                    provider_response_id: None,
                    finish_reason: LlmFinishReason {
                        reason: "stop".into(),
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
        assert_eq!(
            state.last_output_ref.as_deref(),
            Some(hash_ref('e').as_str())
        );

        let output_blob_get_hash = match out3.effects.first() {
            Some(effect) if matches!(effect, SessionEffectCommand::BlobGet { .. }) => {
                effect.params_hash().to_string()
            }
            _ => panic!("expected blob.get for llm output"),
        };

        let output_bytes = serde_json::to_vec(&LlmOutputEnvelope {
            assistant_text: Some("done".into()),
            tool_calls_ref: None,
            reasoning_ref: None,
        })
        .expect("encode output envelope");

        let prev_lifecycle = state.lifecycle;
        let prev_run_id = state.active_run_id.clone();
        state.last_output_ref = None;
        apply_session_workflow_event(
            &mut state,
            &receipt_event(
                4,
                "sys/blob.get@1",
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

        assert_eq!(state.lifecycle, SessionLifecycle::WaitingInput);
        assert_eq!(
            state.last_output_ref.as_deref(),
            Some(hash_ref('e').as_str())
        );

        let changed = crate::helpers::primitives::session_lifecycle_changed_payload(
            &state,
            prev_lifecycle,
            prev_run_id,
            state.updated_at,
        )
        .expect("lifecycle payload");
        assert_eq!(changed.output_ref.as_deref(), Some(hash_ref('e').as_str()));
    }

    #[test]
    fn workspace_apply_composite_tool_runs_to_completion() {
        let mut state = local_coding_state();
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
        let _ = apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");

        let calls = vec![ToolCallObserved {
            call_id: "workspace-call-1".into(),
            tool_name: "workspace_apply".into(),
            arguments_json: serde_json::json!({
                "workspace": "draft",
                "operations": [
                    {
                        "op": "write",
                        "path": "draft.txt",
                        "text": "draft body"
                    },
                    {
                        "op": "write",
                        "path": "linked.bin",
                        "blob_hash": fake_hash('9')
                    }
                ]
            })
            .to_string(),
            arguments_ref: None,
            provider_call_id: None,
        }];
        let params_hash = fake_hash('h');
        let mut out = SessionReduceOutput::default();
        let started = run_tool_batch(
            &mut state,
            RunToolBatch {
                intent_id: fake_hash('i').as_str(),
                params_hash: Some(&params_hash),
                calls: &calls,
            },
            &mut out,
        )
        .expect("start tool batch");
        assert_eq!(
            started.started.plan.execution_plan.groups,
            vec![vec![String::from("workspace-call-1")]]
        );

        let (resolve_hash, resolve_issuer_ref) = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::ToolEffect { kind, pending, .. }
                    if *kind == ToolEffectOp::WorkspaceResolve =>
                {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                }
                _ => None,
            })
            .expect("workspace.resolve effect");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                2,
                "sys/workspace.resolve@1",
                Some(resolve_hash),
                resolve_issuer_ref,
                "ok",
                &WorkspaceResolveReceipt {
                    exists: false,
                    resolved_version: None,
                    head: None,
                    root_hash: None,
                },
            ),
        )
        .expect("workspace.resolve receipt");

        let (empty_root_hash, empty_root_issuer_ref) = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::ToolEffect { kind, pending, .. }
                    if *kind == ToolEffectOp::WorkspaceEmptyRoot =>
                {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                }
                _ => None,
            })
            .expect("workspace.empty_root effect");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                3,
                "sys/workspace.empty_root@1",
                Some(empty_root_hash),
                empty_root_issuer_ref,
                "ok",
                &WorkspaceEmptyRootReceipt {
                    root_hash: hash_ref('c'),
                },
            ),
        )
        .expect("workspace.empty_root receipt");

        let (blob_put_hash, blob_put_issuer_ref) = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::BlobPut { pending, .. } => {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                }
                _ => None,
            })
            .expect("blob.put effect");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                4,
                "sys/blob.put@1",
                Some(blob_put_hash),
                blob_put_issuer_ref,
                "ok",
                &BlobPutReceipt {
                    blob_ref: hash_ref('d'),
                    edge_ref: hash_ref('e'),
                    size: 10,
                },
            ),
        )
        .expect("blob.put receipt");

        let (write_ref_hash_1, write_ref_issuer_ref_1) = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::ToolEffect { kind, pending, .. }
                    if *kind == ToolEffectOp::WorkspaceWriteRef =>
                {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                }
                _ => None,
            })
            .expect("first workspace.write_ref effect");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                5,
                "sys/workspace.write_ref@1",
                Some(write_ref_hash_1),
                write_ref_issuer_ref_1,
                "ok",
                &WorkspaceWriteRefReceipt {
                    new_root_hash: hash_ref('f'),
                    blob_hash: hash_ref('d'),
                },
            ),
        )
        .expect("first workspace.write_ref receipt");

        let (write_ref_hash_2, write_ref_issuer_ref_2) = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::ToolEffect { kind, pending, .. }
                    if *kind == ToolEffectOp::WorkspaceWriteRef =>
                {
                    Some((pending.params_hash.clone(), pending.issuer_ref.clone()))
                }
                _ => None,
            })
            .expect("second workspace.write_ref effect");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event_with_issuer_ref(
                6,
                "sys/workspace.write_ref@1",
                Some(write_ref_hash_2),
                write_ref_issuer_ref_2,
                "ok",
                &WorkspaceWriteRefReceipt {
                    new_root_hash: hash_ref('a'),
                    blob_hash: hash_ref('9'),
                },
            ),
        )
        .expect("second workspace.write_ref receipt");

        assert!(
            out.effects
                .iter()
                .any(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. })),
            "expected follow-up blob.put effects after tool completion"
        );
    }

    #[test]
    fn workspace_commit_immediate_tool_allows_next_group_to_run() {
        let mut state = local_coding_state();
        state.session_id = SessionId("s-1".into());
        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");

        let calls = vec![
            ToolCallObserved {
                call_id: "workspace-commit".into(),
                tool_name: "workspace_commit".into(),
                arguments_json: serde_json::json!({
                    "workspace": "draft",
                    "root_hash": fake_hash('e'),
                    "owner": "agent"
                })
                .to_string(),
                arguments_ref: None,
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "workspace-diff".into(),
                tool_name: "workspace_diff".into(),
                arguments_json: serde_json::json!({
                    "left": { "root_hash": fake_hash('a') },
                    "right": { "root_hash": fake_hash('b') }
                })
                .to_string(),
                arguments_ref: None,
                provider_call_id: None,
            },
        ];
        let params_hash = fake_hash('h');
        let mut out = SessionReduceOutput::default();
        let started = run_tool_batch(
            &mut state,
            RunToolBatch {
                intent_id: fake_hash('i').as_str(),
                params_hash: Some(&params_hash),
                calls: &calls,
            },
            &mut out,
        )
        .expect("run tool batch");
        assert_eq!(
            started.started.plan.execution_plan.groups,
            vec![
                vec![String::from("workspace-commit")],
                vec![String::from("workspace-diff")]
            ]
        );
        assert_eq!(
            out.domain_events.len(),
            1,
            "expected workspace commit event"
        );
        assert!(
            out.effects
                .iter()
                .any(|effect| matches!(effect, SessionEffectCommand::ToolEffect { kind, .. } if *kind == ToolEffectOp::WorkspaceDiff)),
            "expected workspace.diff effect after immediate commit group"
        );
    }

    #[test]
    fn workspace_commit_with_arguments_ref_rewinds_into_next_group() {
        let mut state = local_coding_state();
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
        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");

        let calls = vec![
            ToolCallObserved {
                call_id: "workspace-commit".into(),
                tool_name: "workspace_commit".into(),
                arguments_json: String::new(),
                arguments_ref: Some(fake_hash('c')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "workspace-diff".into(),
                tool_name: "workspace_diff".into(),
                arguments_json: String::new(),
                arguments_ref: Some(fake_hash('d')),
                provider_call_id: None,
            },
            ToolCallObserved {
                call_id: "shell".into(),
                tool_name: "shell".into(),
                arguments_json: String::new(),
                arguments_ref: Some(fake_hash('e')),
                provider_call_id: None,
            },
        ];
        let params_hash = fake_hash('h');
        let mut out = SessionReduceOutput::default();
        run_tool_batch(
            &mut state,
            RunToolBatch {
                intent_id: fake_hash('i').as_str(),
                params_hash: Some(&params_hash),
                calls: &calls,
            },
            &mut out,
        )
        .expect("run tool batch");

        let commit_args_hash = out
            .effects
            .iter()
            .find(|effect| matches!(effect, SessionEffectCommand::BlobGet { .. }))
            .map(|effect| effect.params_hash().to_string())
            .expect("expected blob.get for commit args");

        let mut out = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                2,
                "sys/blob.get@1",
                Some(commit_args_hash),
                "ok",
                &BlobGetReceipt {
                    blob_ref: hash_ref('c'),
                    size: 80,
                    bytes: serde_json::to_vec(&serde_json::json!({
                        "workspace": "draft",
                        "root_hash": fake_hash('e'),
                        "owner": "agent",
                    }))
                    .expect("commit args"),
                },
            ),
        )
        .expect("commit args blob.get receipt");

        assert_eq!(out.domain_events.len(), 1, "expected commit event");
        assert!(
            out.effects
                .iter()
                .any(|effect| matches!(effect, SessionEffectCommand::BlobGet { .. })),
            "expected next group blob.get after commit"
        );
        assert!(
            !out.effects.iter().any(|effect| matches!(
                effect,
                SessionEffectCommand::ToolEffect { kind, .. } if *kind == ToolEffectOp::HostExec
            )),
            "did not expect shell execution before diff args resolve"
        );

        let diff_args_hash = out
            .effects
            .iter()
            .find(|effect| matches!(effect, SessionEffectCommand::BlobGet { .. }))
            .map(|effect| effect.params_hash().to_string())
            .expect("expected blob.get for diff args");

        out = apply_session_workflow_event(
            &mut state,
            &receipt_event(
                3,
                "sys/blob.get@1",
                Some(diff_args_hash),
                "ok",
                &BlobGetReceipt {
                    blob_ref: hash_ref('d'),
                    size: 96,
                    bytes: serde_json::to_vec(&serde_json::json!({
                        "left": { "root_hash": fake_hash('a') },
                        "right": { "root_hash": fake_hash('b') },
                    }))
                    .expect("diff args"),
                },
            ),
        )
        .expect("diff args blob.get receipt");

        assert!(
            out.effects.iter().any(|effect| matches!(
                effect,
                SessionEffectCommand::ToolEffect { kind, .. } if *kind == ToolEffectOp::WorkspaceDiff
            )),
            "expected workspace.diff effect after diff args resolve"
        );
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

        let refs = self::tool_batch::collect_blob_refs_from_output_json(&output_json);
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

        let updated = self::blob_effects::inject_blob_inline_text_into_output_json(
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
