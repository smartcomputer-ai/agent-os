use super::{
    LlmStepContext, RequestLlm, SessionEffectCommand, SessionReduceOutput, allocate_run_id,
    can_apply_host_command, enqueue_host_text, pop_follow_up_if_ready, request_llm,
    transition_lifecycle,
};
use crate::contracts::{
    EffectiveTool, EffectiveToolSet, HostCommandKind, PendingBlobGetKind, PendingBlobPutKind,
    PendingFollowUpTurn, RunConfig, SessionConfig, SessionIngressKind, SessionLifecycle,
    SessionState, SessionWorkflowEvent, ToolAvailabilityRule, ToolBatchPlan, ToolCallStatus,
    ToolOverrideScope, ToolSpec, default_tool_profile_for_provider,
};
use crate::tools::{
    ToolEffectOp, map_tool_arguments_to_effect_params, map_tool_receipt_to_llm_result,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use aos_effects::builtins::LlmGenerateReceipt;
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
    state.last_output_ref = None;

    refresh_effective_tools(state, Some(&requested))?;
    state.conversation_message_refs.push(input_ref.into());
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

fn handle_standalone_host_session_open_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    if envelope.effect_op != "sys/host.session.open@1" {
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
        origin_workflow_op_hash: rejected.origin_workflow_op_hash.clone(),
        origin_instance_key: rejected.origin_instance_key.clone(),
        intent_id: rejected.intent_id.clone(),
        effect_op: rejected.effect_op.clone(),
        effect_op_hash: rejected.effect_op_hash.clone(),
        executor_module: rejected.executor_module.clone(),
        executor_module_hash: rejected.executor_module_hash.clone(),
        executor_entrypoint: rejected.executor_entrypoint.clone(),
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
    if handle_pending_blob_get_receipt(state, envelope, out)?
        || handle_pending_blob_put_receipt(state, envelope, out)?
    {
        recompute_in_flight_effects(state);
        return Ok(());
    }

    if let Some(matched) = state.pending_effects.settle(envelope.into()) {
        match matched.pending.effect_op.as_str() {
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
        match matched.pending.effect_op.as_str() {
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
}

fn fail_run(state: &mut SessionState) -> Result<(), SessionReduceError> {
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

    request_llm(
        state,
        out,
        RequestLlm {
            step: LlmStepContext {
                correlation_id: Some(alloc::format!(
                    "run-{run_seq}-turn-{}",
                    state.next_tool_batch_seq + 1
                )),
                message_refs,
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
            cap_slot: Some("llm".into()),
        },
    )?;
    Ok(())
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
        .contains_effect_op("sys/host.session.open@1")
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
    out.effects.push(SessionEffectCommand::ToolEffect {
        kind: ToolEffectOp::HostSessionOpen,
        params_json: serde_json::to_string(&params.params_json).unwrap_or_else(|_| "{}".into()),
        pending: super::begin_pending_effect(
            state,
            "sys/host.session.open@1",
            &params.params_json,
            Some("host".into()),
            None,
        ),
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
        HostSessionStatus, SessionId, SessionIngress, ToolCallObserved, ToolOverrideScope,
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
        effect_op: &str,
        params_hash: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        receipt_event_with_issuer_ref(
            emitted_at_seq,
            effect_op,
            params_hash,
            None,
            status,
            payload,
        )
    }

    fn receipt_event_with_issuer_ref<T: serde::Serialize>(
        emitted_at_seq: u64,
        effect_op: &str,
        params_hash: Option<String>,
        issuer_ref: Option<String>,
        status: &str,
        payload: &T,
    ) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Receipt(aos_wasm_sdk::EffectReceiptEnvelope {
            origin_module_id: "aos.agent/SessionWorkflow@1".into(),
            origin_workflow_op_hash: None,
            origin_instance_key: None,
            intent_id: fake_hash('i'),
            effect_op: effect_op.into(),
            effect_op_hash: None,
            executor_module: None,
            executor_module_hash: None,
            executor_entrypoint: None,
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
            Some(SessionEffectCommand::ToolEffect { pending, .. })
                if pending.effect_op == "sys/host.session.open@1"
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
        let mut state = SessionState::default();
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
