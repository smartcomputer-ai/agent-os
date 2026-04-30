use super::{
    DefaultTurnPlanner, LlmStepContext, RequestLlm, SessionEffectCommand, SessionWorkflowOutput,
    TurnPlanError, TurnRequest, allocate_run_id, can_apply_host_command, pop_follow_up_if_ready,
    request_llm, transition_lifecycle,
};
use crate::contracts::{
    HostCommandKind, HostSessionOpenConfig, HostTargetConfig, PendingBlobGetKind,
    PendingBlobPutKind, QueuedRunStart, RunCause, RunConfig, RunFailure, RunInterrupt,
    RunLifecycle, RunOutcome, RunRecord, RunState, RunTrace, RunTraceEntry, RunTraceEntryKind,
    RunTraceRef, RunTraceSummary, SessionConfig, SessionInputKind, SessionLifecycle, SessionState,
    SessionStatus, SessionWorkflowEvent, StagedToolFollowUpTurn, ToolBatchPlan, ToolCallStatus,
    ToolOverrideScope, ToolSpec, TurnBudget, TurnInput, TurnInputKind, TurnInputLane,
    TurnObservation, TurnPlan, TurnPrerequisiteKind, TurnPriority, TurnToolChoice, TurnToolInput,
};
use crate::helpers::TurnPlanner;
use crate::tools::{ToolEffectOp, map_tool_receipt_to_llm_result};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
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
    enqueue_blob_get, enqueue_blob_put, handle_blob_get_receipt, handle_blob_put_receipt,
    has_open_tool_definition_puts,
};
use self::types::pending_effect_lookup_err_to_session_err;
pub use self::types::{
    CompletedToolBatch, RunToolBatch, RunToolBatchResult, SessionRuntimeLimits,
    SessionWorkflowError, StartedToolBatch, ToolBatchReceiptMatch,
};

const TOOL_RESULT_BLOB_MAX_BYTES: usize = 8 * 1024;

pub(super) fn trace_ref(kind: impl Into<String>, value: impl Into<String>) -> RunTraceRef {
    let value = value.into();
    if value.starts_with("sha256:") {
        RunTraceRef {
            kind: kind.into(),
            ref_: Some(value),
            value: None,
        }
    } else {
        RunTraceRef {
            kind: kind.into(),
            ref_: None,
            value: Some(value),
        }
    }
}

pub(super) fn push_run_trace(
    state: &mut SessionState,
    kind: RunTraceEntryKind,
    summary: impl Into<String>,
    refs: Vec<RunTraceRef>,
    metadata: BTreeMap<String, String>,
) {
    let observed_at_ns = state.updated_at;
    let Some(run) = state.current_run.as_mut() else {
        return;
    };
    push_trace_entry(
        &mut run.trace,
        observed_at_ns,
        kind,
        summary.into(),
        refs,
        metadata,
    );
    run.updated_at = observed_at_ns;
}

fn push_trace_entry(
    trace: &mut RunTrace,
    observed_at_ns: u64,
    kind: RunTraceEntryKind,
    summary: String,
    refs: Vec<RunTraceRef>,
    metadata: BTreeMap<String, String>,
) {
    let max_entries = if trace.max_entries == 0 {
        crate::contracts::DEFAULT_RUN_TRACE_MAX_ENTRIES
    } else {
        trace.max_entries
    } as usize;
    let seq = trace.next_seq;
    trace.next_seq = trace.next_seq.saturating_add(1);
    if max_entries > 0 {
        while trace.entries.len() >= max_entries {
            trace.entries.remove(0);
            trace.dropped_entries = trace.dropped_entries.saturating_add(1);
        }
        trace.entries.push(RunTraceEntry {
            seq,
            observed_at_ns,
            kind,
            summary,
            refs,
            metadata,
        });
    } else {
        trace.dropped_entries = trace.dropped_entries.saturating_add(1);
    }
}

fn summarize_trace(trace: &RunTrace) -> RunTraceSummary {
    let first = trace.entries.first();
    let last = trace.entries.last();
    RunTraceSummary {
        entry_count: trace.entries.len() as u64,
        dropped_entries: trace.dropped_entries,
        first_seq: first.map(|entry| entry.seq),
        last_seq: last.map(|entry| entry.seq),
        last_kind: last.map(|entry| entry.kind.clone()),
        last_summary: last.map(|entry| entry.summary.clone()),
        last_observed_at_ns: last.map(|entry| entry.observed_at_ns),
    }
}

pub fn run_tool_batch(
    state: &mut SessionState,
    request: RunToolBatch<'_>,
    out: &mut SessionWorkflowOutput,
) -> Result<RunToolBatchResult, SessionWorkflowError> {
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
    out: &mut SessionWorkflowOutput,
) -> Result<Option<CompletedToolBatch>, SessionWorkflowError> {
    let completion = tool_batch::advance_tool_batch(state, out)?;
    handle_completed_tool_batch(state, completion, out)
}

pub fn settle_tool_batch_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionWorkflowOutput,
) -> Result<ToolBatchReceiptMatch, SessionWorkflowError> {
    match tool_batch::settle_tool_batch_receipt(state, envelope, out)? {
        ToolBatchReceiptMatch::Unmatched => Ok(ToolBatchReceiptMatch::Unmatched),
        ToolBatchReceiptMatch::Matched { completion } => Ok(ToolBatchReceiptMatch::Matched {
            completion: handle_completed_tool_batch(state, completion, out)?,
        }),
    }
}

fn transition_to_waiting_input_if_running(
    state: &mut SessionState,
) -> Result<(), SessionWorkflowError> {
    if matches!(state.lifecycle, SessionLifecycle::Running) {
        transition_lifecycle(state, SessionLifecycle::WaitingInput)
            .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
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
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
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
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
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
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
    stamp_timestamps(state, event);

    let mut out = SessionWorkflowOutput::default();
    match event {
        SessionWorkflowEvent::Input(input) => {
            if state.session_id.0.is_empty() {
                state.session_id = input.session_id.clone();
            }
            match &input.input {
                SessionInputKind::RunRequested {
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
                SessionInputKind::RunStartRequested {
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
                SessionInputKind::FollowUpInputAppended {
                    input_ref,
                    run_overrides,
                } => {
                    validate_run_request_catalog(
                        state,
                        run_overrides.as_ref(),
                        allowed_providers,
                        allowed_models,
                    )?;
                    on_follow_up_input_appended(
                        state,
                        input_ref,
                        run_overrides.as_ref(),
                        &mut out,
                    )?;
                }
                SessionInputKind::RunSteerRequested { instruction_ref } => {
                    on_run_steer_requested(state, instruction_ref)?;
                }
                SessionInputKind::RunInterruptRequested { reason_ref } => {
                    on_run_interrupt_requested(state, reason_ref.clone(), &mut out)?;
                }
                SessionInputKind::SessionOpened { config } => {
                    on_session_opened(state, config.as_ref())?;
                }
                SessionInputKind::SessionConfigUpdated { config } => {
                    state.session_config = config.clone();
                }
                SessionInputKind::SessionPaused => {
                    transition_session_status(state, SessionStatus::Paused)?;
                }
                SessionInputKind::SessionResumed => {
                    transition_session_status(state, SessionStatus::Open)?;
                }
                SessionInputKind::SessionArchived => {
                    transition_session_status(state, SessionStatus::Archived)?;
                }
                SessionInputKind::SessionExpired => {
                    transition_session_status(state, SessionStatus::Expired)?;
                }
                SessionInputKind::SessionClosed => {
                    transition_session_status(state, SessionStatus::Closed)?;
                }
                SessionInputKind::HostCommandReceived(command) => {
                    on_host_command(state, command, &mut out)?
                }
                SessionInputKind::ToolRegistrySet {
                    registry,
                    profiles,
                    default_profile,
                } => on_tool_registry_set(
                    state,
                    registry,
                    profiles.as_ref(),
                    default_profile.as_ref(),
                )?,
                SessionInputKind::ToolProfileSelected { profile_id } => {
                    on_tool_profile_selected(state, profile_id)?
                }
                SessionInputKind::ToolOverridesSet {
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
                SessionInputKind::TurnObserved(observation) => {
                    on_turn_observed(state, observation);
                }
                SessionInputKind::HostSessionUpdated {
                    host_session_id,
                    host_session_status,
                } => {
                    on_host_session_updated(state, host_session_id.as_ref(), *host_session_status)?
                }
                SessionInputKind::RunCompleted => {
                    finish_current_run(
                        state,
                        RunLifecycle::Completed,
                        Some(RunOutcome {
                            output_ref: state.last_output_ref.clone(),
                            failure: None,
                            cancelled_reason: None,
                            interrupted_reason_ref: None,
                        }),
                    );
                    transition_lifecycle(state, SessionLifecycle::Completed)
                        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                    start_next_queued_run(state, &mut out)?;
                }
                SessionInputKind::RunFailed { code, detail } => {
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
                            interrupted_reason_ref: None,
                        }),
                    );
                    transition_lifecycle(state, SessionLifecycle::Failed)
                        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                    start_next_queued_run(state, &mut out)?;
                }
                SessionInputKind::RunCancelled { reason } => {
                    finish_current_run(
                        state,
                        RunLifecycle::Cancelled,
                        Some(RunOutcome {
                            output_ref: state.last_output_ref.clone(),
                            failure: None,
                            cancelled_reason: reason.clone(),
                            interrupted_reason_ref: None,
                        }),
                    );
                    transition_lifecycle(state, SessionLifecycle::Cancelled)
                        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
                    clear_active_run(state);
                    start_next_queued_run(state, &mut out)?;
                }
                SessionInputKind::Noop => {}
            }
        }
        SessionWorkflowEvent::Receipt(receipt) => on_receipt_envelope(state, receipt, &mut out)?,
        SessionWorkflowEvent::ReceiptRejected(rejected) => {
            on_receipt_rejected(state, rejected, &mut out)?
        }
        SessionWorkflowEvent::StreamFrame(frame) => {
            let _ = state.pending_effects.observe(frame.into());
            trace_stream_frame(state, frame);
        }
        SessionWorkflowEvent::Noop => {}
    }

    trace_workflow_output(state, &out);
    recompute_in_flight_effects(state);
    finish_interrupted_run_if_quiescent(state, &mut out)?;
    recompute_in_flight_effects(state);
    enforce_runtime_limits(state, limits)?;
    Ok(out)
}

pub fn apply_session_event(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
    apply_session_workflow_event(state, event)
}

pub fn apply_session_event_with_catalog(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
    apply_session_workflow_event_with_catalog(state, event, allowed_providers, allowed_models)
}

pub fn apply_session_event_with_catalog_and_limits(
    state: &mut SessionState,
    event: &SessionWorkflowEvent,
    allowed_providers: &[&str],
    allowed_models: &[&str],
    limits: SessionRuntimeLimits,
) -> Result<SessionWorkflowOutput, SessionWorkflowError> {
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
) -> Result<(), SessionWorkflowError> {
    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_catalog(&requested, allowed_providers, allowed_models)
}

pub fn validate_run_catalog(
    config: &RunConfig,
    allowed_providers: &[&str],
    allowed_models: &[&str],
) -> Result<(), SessionWorkflowError> {
    validate_run_config(config)?;

    if !allowed_providers.is_empty()
        && !allowed_providers
            .iter()
            .any(|value| config.provider.trim() == value.trim())
    {
        return Err(SessionWorkflowError::UnknownProvider);
    }

    if !allowed_models.is_empty()
        && !allowed_models
            .iter()
            .any(|value| config.model.trim() == value.trim())
    {
        return Err(SessionWorkflowError::UnknownModel);
    }

    Ok(())
}

fn on_session_opened(
    state: &mut SessionState,
    config: Option<&SessionConfig>,
) -> Result<(), SessionWorkflowError> {
    if let Some(config) = config {
        state.session_config = config.clone();
    }
    transition_session_status(state, SessionStatus::Open)
}

fn transition_session_status(
    state: &mut SessionState,
    next: SessionStatus,
) -> Result<(), SessionWorkflowError> {
    if state.status == next {
        return Ok(());
    }
    if state.active_run_id.is_some()
        && matches!(
            next,
            SessionStatus::Archived | SessionStatus::Expired | SessionStatus::Closed
        )
    {
        return Err(SessionWorkflowError::RunAlreadyActive);
    }
    state.status = next;
    Ok(())
}

fn on_run_start_requested(
    state: &mut SessionState,
    cause: &RunCause,
    run_overrides: Option<&SessionConfig>,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    if !state.status.accepts_new_runs() {
        return Err(SessionWorkflowError::InvalidLifecycleTransition);
    }
    if state.active_run_id.is_some() {
        return Err(SessionWorkflowError::RunAlreadyActive);
    }
    if cause.input_refs.is_empty() {
        return Err(SessionWorkflowError::EmptyMessageRefs);
    }

    let requested = select_run_config(&state.session_config, run_overrides);
    validate_run_config(&requested)?;

    transition_lifecycle(state, SessionLifecycle::Running)
        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;

    let run_id = allocate_run_id(state);
    state.active_run_id = Some(run_id.clone());
    state.active_run_config = Some(requested.clone());
    state.current_run = Some(RunState {
        run_id: run_id.clone(),
        lifecycle: RunLifecycle::Running,
        cause: cause.clone(),
        config: requested.clone(),
        input_refs: cause.input_refs.clone(),
        turn_plan: None,
        queued_steer_refs: Vec::new(),
        interrupt: None,
        active_tool_batch: None,
        pending_effects: aos_wasm_sdk::PendingEffects::new(),
        pending_blob_gets: aos_wasm_sdk::SharedBlobGets::new(),
        pending_blob_puts: aos_wasm_sdk::SharedBlobPuts::new(),
        staged_tool_follow_up_turn: None,
        pending_llm_turn_refs: None,
        last_output_ref: None,
        tool_refs_materialized: false,
        in_flight_effects: 0,
        outcome: None,
        trace: RunTrace::default(),
        started_at: state.updated_at,
        updated_at: state.updated_at,
    });
    let mut refs = cause
        .input_refs
        .iter()
        .cloned()
        .map(|value| trace_ref("input_ref", value))
        .collect::<Vec<_>>();
    if let Some(payload_ref) = cause.payload_ref.as_ref() {
        refs.push(trace_ref("payload_ref", payload_ref.clone()));
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("cause_kind".into(), cause.kind.clone());
    push_run_trace(
        state,
        RunTraceEntryKind::RunStarted,
        "run started",
        refs,
        metadata,
    );
    state.active_tool_batch = None;
    state.pending_blob_gets.clear();
    state.pending_blob_puts.clear();
    state.staged_tool_follow_up_turn = None;
    state.pending_llm_turn_refs = None;
    state.queued_steer_refs.clear();
    state.run_interrupt = None;
    state.last_output_ref = None;
    state.tool_refs_materialized = false;

    state
        .transcript_message_refs
        .extend(cause.input_refs.iter().cloned());
    set_pending_llm_turn(state, state.transcript_message_refs.clone(), out)
}

fn on_follow_up_input_appended(
    state: &mut SessionState,
    input_ref: &str,
    run_overrides: Option<&SessionConfig>,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    let queued = QueuedRunStart {
        cause: RunCause::direct_input(input_ref.into()),
        run_overrides: run_overrides.cloned(),
        queued_at: state.updated_at,
    };
    if state.active_run_id.is_none() {
        return start_queued_run(state, queued, out);
    }

    let mut refs = queued
        .cause
        .input_refs
        .iter()
        .cloned()
        .map(|value| trace_ref("input_ref", value))
        .collect::<Vec<_>>();
    refs.push(trace_ref("queue", "follow_up"));
    push_run_trace(
        state,
        RunTraceEntryKind::InterventionRequested,
        "follow-up input queued",
        refs,
        BTreeMap::new(),
    );
    state.queued_follow_up_runs.push(queued);
    Ok(())
}

fn on_run_steer_requested(
    state: &mut SessionState,
    instruction_ref: &str,
) -> Result<(), SessionWorkflowError> {
    if state.active_run_id.is_none() {
        return Err(SessionWorkflowError::RunNotActive);
    }
    let refs = vec![trace_ref("instruction_ref", instruction_ref.to_string())];
    push_run_trace(
        state,
        RunTraceEntryKind::InterventionRequested,
        "steer instruction queued for next LLM turn",
        refs,
        BTreeMap::new(),
    );
    state.queued_steer_refs.push(instruction_ref.into());
    sync_current_run_execution(state);
    Ok(())
}

fn on_run_interrupt_requested(
    state: &mut SessionState,
    reason_ref: Option<String>,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    if state.active_run_id.is_none() {
        return Err(SessionWorkflowError::RunNotActive);
    }
    let interrupt = RunInterrupt {
        reason_ref,
        requested_at: state.updated_at,
    };
    let refs = interrupt
        .reason_ref
        .iter()
        .cloned()
        .map(|value| trace_ref("reason_ref", value))
        .collect::<Vec<_>>();
    push_run_trace(
        state,
        RunTraceEntryKind::InterventionRequested,
        "run interrupt requested",
        refs,
        BTreeMap::new(),
    );
    state.run_interrupt = Some(interrupt);
    state.pending_llm_turn_refs = None;
    sync_current_run_execution(state);
    finish_interrupted_run_if_quiescent(state, out)
}

fn on_host_command(
    state: &mut SessionState,
    command: &crate::contracts::HostCommand,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    if !can_apply_host_command(state, &command.command) {
        return Err(SessionWorkflowError::HostCommandRejected);
    }

    let mut metadata = BTreeMap::new();
    metadata.insert("command_id".into(), command.command_id.clone());
    metadata.insert("issued_at".into(), command.issued_at.to_string());
    metadata.insert("command".into(), alloc::format!("{:?}", command.command));
    push_run_trace(
        state,
        RunTraceEntryKind::InterventionRequested,
        "host command received",
        Vec::new(),
        metadata,
    );

    match &command.command {
        HostCommandKind::Pause => {
            transition_session_status(state, SessionStatus::Paused)?;
            transition_lifecycle(state, SessionLifecycle::Paused)
                .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Resume => {
            transition_session_status(state, SessionStatus::Open)?;
            transition_lifecycle(state, SessionLifecycle::Running)
                .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
        }
        HostCommandKind::Cancel { .. } => {
            transition_lifecycle(state, SessionLifecycle::Cancelling)
                .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
            finish_current_run(
                state,
                RunLifecycle::Cancelled,
                Some(RunOutcome {
                    output_ref: state.last_output_ref.clone(),
                    failure: None,
                    cancelled_reason: Some("host command cancel".into()),
                    interrupted_reason_ref: None,
                }),
            );
            transition_lifecycle(state, SessionLifecycle::Cancelled)
                .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
            clear_active_run(state);
            start_next_queued_run(state, out)?;
        }
        HostCommandKind::Noop => {}
    }

    Ok(())
}

fn validate_tool_registry_payload(
    registry: &BTreeMap<String, ToolSpec>,
    profiles: Option<&BTreeMap<String, Vec<String>>>,
    default_profile: Option<&String>,
) -> Result<(), SessionWorkflowError> {
    crate::tools::registry::validate_tool_registry(registry)
        .map_err(|_| SessionWorkflowError::InvalidToolRegistry)?;

    if let Some(profiles) = profiles {
        for tool_ids in profiles.values() {
            for tool_id in tool_ids {
                if !registry.contains_key(tool_id) {
                    return Err(SessionWorkflowError::InvalidToolRegistry);
                }
            }
        }
        if let Some(profile) = default_profile
            && !profiles.contains_key(profile)
        {
            return Err(SessionWorkflowError::InvalidToolRegistry);
        }
    }
    Ok(())
}

fn on_tool_registry_set(
    state: &mut SessionState,
    registry: &BTreeMap<String, ToolSpec>,
    profiles: Option<&BTreeMap<String, Vec<String>>>,
    default_profile: Option<&String>,
) -> Result<(), SessionWorkflowError> {
    validate_tool_registry_payload(registry, profiles, default_profile)?;
    state.tool_registry = registry.clone();
    if let Some(profiles) = profiles {
        state.tool_profiles = profiles.clone();
    }
    if let Some(default_profile) = default_profile {
        state.tool_profile = default_profile.clone();
    }

    Ok(())
}

fn on_tool_profile_selected(
    state: &mut SessionState,
    profile_id: &str,
) -> Result<(), SessionWorkflowError> {
    if !state.tool_profiles.contains_key(profile_id) {
        return Err(SessionWorkflowError::ToolProfileUnknown);
    }
    state.tool_profile = profile_id.into();
    Ok(())
}

fn on_tool_overrides_set(
    state: &mut SessionState,
    scope: ToolOverrideScope,
    enable: Option<&[String]>,
    disable: Option<&[String]>,
    force: Option<&[String]>,
) -> Result<(), SessionWorkflowError> {
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
                .ok_or(SessionWorkflowError::RunNotActive)?;
            active.tool_enable = enable.map(|items| items.to_vec());
            active.tool_disable = disable.map(|items| items.to_vec());
            active.tool_force = force.map(|items| items.to_vec());
        }
    }

    Ok(())
}

fn on_host_session_updated(
    state: &mut SessionState,
    host_session_id: Option<&String>,
    host_session_status: Option<crate::contracts::HostSessionStatus>,
) -> Result<(), SessionWorkflowError> {
    state.tool_runtime_context.host_session_id = host_session_id.cloned();
    state.tool_runtime_context.host_session_status = host_session_status;
    Ok(())
}

fn on_turn_observed(state: &mut SessionState, observation: &TurnObservation) {
    match observation {
        TurnObservation::InputObserved(input) => {
            state
                .turn_state
                .durable_inputs
                .retain(|existing| existing.input_id != input.input_id);
            state.turn_state.durable_inputs.push(input.clone());
        }
        TurnObservation::InputRemoved { input_id } => {
            state
                .turn_state
                .durable_inputs
                .retain(|existing| existing.input_id != *input_id);
        }
        TurnObservation::CustomStateRefUpdated(value) => {
            state.turn_state.custom_state_refs.retain(|existing| {
                existing.planner_id != value.planner_id || existing.key != value.key
            });
            state.turn_state.custom_state_refs.push(value.clone());
        }
        TurnObservation::CustomStateRefRemoved { planner_id, key } => {
            state
                .turn_state
                .custom_state_refs
                .retain(|existing| existing.planner_id != *planner_id || existing.key != *key);
        }
        TurnObservation::Noop => {}
    }
}

fn handle_standalone_host_session_open_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionWorkflowOutput,
) -> Result<bool, SessionWorkflowError> {
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
    if mapped.is_error {
        fail_run(state)?;
        return Ok(true);
    }

    dispatch_pending_llm_turn(state, out)?;
    Ok(true)
}

fn handle_llm_generate_receipt(
    state: &mut SessionState,
    envelope: &EffectReceiptEnvelope,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    if envelope.status != "ok" {
        fail_run(state)?;
        return Ok(());
    }

    let Some(receipt): Option<LlmGenerateReceipt> = envelope.decode_receipt_payload().ok() else {
        fail_run(state)?;
        return Ok(());
    };

    state.last_output_ref = Some(receipt.output_ref.as_str().into());
    let refs = vec![trace_ref("output_ref", receipt.output_ref.to_string())];
    let mut metadata = BTreeMap::new();
    metadata.insert("provider_id".into(), receipt.provider_id.clone());
    metadata.insert("finish_reason".into(), receipt.finish_reason.reason.clone());
    metadata.insert("status".into(), envelope.status.clone());
    push_run_trace(
        state,
        RunTraceEntryKind::LlmReceived,
        "llm receipt received",
        refs,
        metadata,
    );

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
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    trace_receipt_envelope(state, envelope);
    if handle_blob_get_receipt(state, envelope, out)?
        || handle_blob_put_receipt(state, envelope, out)?
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
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    trace_receipt_rejected(state, rejected);
    let envelope = rejected_as_error_envelope(rejected);
    if handle_blob_get_receipt(state, &envelope, out)?
        || handle_blob_put_receipt(state, &envelope, out)?
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
    run.staged_tool_follow_up_turn = state.staged_tool_follow_up_turn.clone();
    run.pending_llm_turn_refs = state.pending_llm_turn_refs.clone();
    run.queued_steer_refs = state.queued_steer_refs.clone();
    run.interrupt = state.run_interrupt.clone();
    run.last_output_ref = state.last_output_ref.clone();
    run.tool_refs_materialized = state.tool_refs_materialized;
    run.in_flight_effects = state.in_flight_effects;
    run.updated_at = state.updated_at;
}

fn fail_run(state: &mut SessionState) -> Result<(), SessionWorkflowError> {
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
            interrupted_reason_ref: None,
        }),
    );
    transition_lifecycle(state, SessionLifecycle::Failed)
        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    Ok(())
}

fn start_queued_run(
    state: &mut SessionState,
    queued: QueuedRunStart,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    on_run_start_requested(state, &queued.cause, queued.run_overrides.as_ref(), out)
}

fn start_next_queued_run(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    let Some(queued) = pop_follow_up_if_ready(state) else {
        return Ok(());
    };
    start_queued_run(state, queued, out)
}

fn has_open_runtime_work(state: &SessionState) -> bool {
    !state.pending_effects.is_empty()
        || !state.pending_blob_gets.is_empty()
        || !state.pending_blob_puts.is_empty()
        || state.staged_tool_follow_up_turn.is_some()
        || state
            .active_tool_batch
            .as_ref()
            .is_some_and(|batch| !batch.is_settled())
}

pub(super) fn finish_interrupted_run_if_quiescent(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    if state.run_interrupt.is_none() || has_open_runtime_work(state) {
        return Ok(());
    }
    let reason_ref = state
        .run_interrupt
        .as_ref()
        .and_then(|interrupt| interrupt.reason_ref.clone());
    finish_current_run(
        state,
        RunLifecycle::Interrupted,
        Some(RunOutcome {
            output_ref: state.last_output_ref.clone(),
            failure: None,
            cancelled_reason: None,
            interrupted_reason_ref: reason_ref,
        }),
    );
    transition_lifecycle(state, SessionLifecycle::Interrupted)
        .map_err(|_| SessionWorkflowError::InvalidLifecycleTransition)?;
    clear_active_run(state);
    start_next_queued_run(state, out)
}

fn handle_completed_tool_batch(
    state: &mut SessionState,
    completion: Option<CompletedToolBatch>,
    out: &mut SessionWorkflowOutput,
) -> Result<Option<CompletedToolBatch>, SessionWorkflowError> {
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
            PendingBlobPutKind::ToolFollowUpMessage { index: idx as u64 },
            out,
        );
        expected_messages = expected_messages.saturating_add(1);
    }
    state.staged_tool_follow_up_turn = Some(StagedToolFollowUpTurn {
        tool_batch_id: completion.tool_batch_id.clone(),
        base_message_refs: state.transcript_message_refs.clone(),
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

fn set_pending_llm_turn(
    state: &mut SessionState,
    message_refs: Vec<String>,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    state.pending_llm_turn_refs = Some(message_refs);
    dispatch_pending_llm_turn(state, out)
}

fn dispatch_pending_llm_turn(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    dispatch_pending_llm_turn_with_planner(
        state,
        out,
        &DefaultTurnPlanner,
        TurnBudget {
            max_input_tokens: None,
            reserve_output_tokens: None,
            max_message_refs: None,
            max_tool_refs: None,
        },
    )
}

pub fn dispatch_pending_llm_turn_with_planner<P: TurnPlanner>(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
    planner: &P,
    budget: TurnBudget,
) -> Result<(), SessionWorkflowError> {
    if state.pending_llm_turn_refs.is_none() {
        return Ok(());
    }
    if state.run_interrupt.is_some() {
        return Ok(());
    }
    if !state.pending_effects.is_empty()
        || !state.pending_blob_gets.is_empty()
        || !state.pending_blob_puts.is_empty()
        || state.staged_tool_follow_up_turn.is_some()
    {
        return Ok(());
    }

    let Some(message_refs) = state.pending_llm_turn_refs.clone() else {
        return Ok(());
    };
    let steer_refs = state.queued_steer_refs.clone();
    let plan = build_turn_plan_for_pending_turn(state, message_refs, planner, budget)?;

    if plan
        .prerequisites
        .iter()
        .any(|item| matches!(item.kind, TurnPrerequisiteKind::OpenHostSession))
    {
        if !state
            .pending_effects
            .contains_effect("sys/host.session.open@1")
        {
            emit_auto_host_session_open(state, out)?;
        }
        return Ok(());
    }

    if plan
        .prerequisites
        .iter()
        .any(|item| matches!(item.kind, TurnPrerequisiteKind::MaterializeToolDefinitions))
    {
        for tool_id in plan.selected_tool_ids.clone() {
            let Some(tool) = state.tool_registry.get(&tool_id).cloned() else {
                return Err(SessionWorkflowError::UnknownToolOverride);
            };
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
        if has_open_tool_definition_puts(state) {
            return Ok(());
        }
        state.tool_refs_materialized = true;
    }

    let run_seq = state
        .active_run_id
        .as_ref()
        .map(|id| id.run_seq)
        .unwrap_or(0);
    state.pending_llm_turn_refs = None;

    request_llm(
        state,
        out,
        RequestLlm {
            step: LlmStepContext {
                correlation_id: Some(alloc::format!(
                    "run-{run_seq}-turn-{}",
                    state.next_tool_batch_seq + 1
                )),
                message_refs: plan.message_refs.clone(),
                temperature: None,
                top_p: None,
                tool_refs: selected_tool_refs(state, &plan),
                tool_choice: plan.tool_choice.clone().map(turn_tool_choice_to_llm),
                stop_sequences: None,
                metadata: None,
                provider_options_ref: plan.provider_options_ref.clone(),
                response_format_ref: plan.response_format_ref.clone(),
                api_key: None,
            },
        },
    )?;
    if !steer_refs.is_empty() {
        let refs = steer_refs
            .iter()
            .cloned()
            .map(|value| trace_ref("instruction_ref", value))
            .collect::<Vec<_>>();
        push_run_trace(
            state,
            RunTraceEntryKind::InterventionApplied,
            "steer instruction injected into LLM turn",
            refs,
            BTreeMap::new(),
        );
        state.queued_steer_refs.clear();
        sync_current_run_execution(state);
    }
    Ok(())
}

pub fn build_turn_plan_for_pending_turn<P: TurnPlanner>(
    state: &mut SessionState,
    mut turn_refs: Vec<String>,
    planner: &P,
    budget: TurnBudget,
) -> Result<TurnPlan, SessionWorkflowError> {
    let steer_refs = state.queued_steer_refs.clone();
    if !steer_refs.is_empty() {
        turn_refs.extend(steer_refs.iter().cloned());
    }
    let (run_id, prompt_refs, cause, mut run_config) = {
        let run = state
            .current_run
            .as_ref()
            .ok_or(SessionWorkflowError::RunNotActive)?;
        (
            run.run_id.clone(),
            run.config.prompt_refs.clone().unwrap_or_default(),
            run.cause.clone(),
            run.config.clone(),
        )
    };
    if run_config.host_session_open.is_none() {
        run_config.host_session_open = state.session_config.default_host_session_open.clone();
    }
    let transcript_refs = state
        .transcript_message_refs
        .iter()
        .filter(|value| !turn_refs.iter().any(|turn| turn == *value))
        .cloned()
        .collect::<Vec<_>>();
    let inputs = build_turn_inputs(&prompt_refs, &transcript_refs, &turn_refs, Some(&cause));
    let tools = build_turn_tool_inputs(state, Some(&run_config))?;
    let mut plan = planner
        .build_turn(TurnRequest {
            session_id: &state.session_id,
            run_id: &run_id,
            run_cause: Some(&cause),
            run_config: &run_config,
            budget,
            state: &state.turn_state,
            inputs: &inputs,
            tools: &tools,
            registry: &state.tool_registry,
            profiles: &state.tool_profiles,
            runtime: &state.tool_runtime_context,
        })
        .map_err(turn_plan_error_to_workflow_error)?;
    add_workflow_prerequisites(state, &mut plan);
    apply_turn_plan_to_state(state, plan)
}

fn add_workflow_prerequisites(state: &SessionState, plan: &mut TurnPlan) {
    if !state.tool_refs_materialized && !plan.selected_tool_ids.is_empty() {
        plan.prerequisites.push(crate::contracts::TurnPrerequisite {
            prerequisite_id: "tool_definitions:materialize".into(),
            kind: TurnPrerequisiteKind::MaterializeToolDefinitions,
            reason: "selected tool definitions must be materialized before llm dispatch".into(),
            input_ids: Vec::new(),
            tool_ids: plan.selected_tool_ids.clone(),
        });
        if !plan
            .report
            .unresolved
            .iter()
            .any(|item| item == "tool_definitions_pending")
        {
            plan.report
                .unresolved
                .push("tool_definitions_pending".into());
        }
    }
}

fn apply_turn_plan_to_state(
    state: &mut SessionState,
    plan: TurnPlan,
) -> Result<TurnPlan, SessionWorkflowError> {
    if plan.message_refs.is_empty() {
        return Err(SessionWorkflowError::EmptyMessageRefs);
    }
    crate::helpers::apply_turn_state_updates(&mut state.turn_state, &plan.state_updates);
    state.turn_state.last_report = Some(plan.report.clone());
    let refs = plan
        .message_refs
        .iter()
        .cloned()
        .map(|value| trace_ref("selected_ref", value))
        .collect::<Vec<_>>();
    let mut metadata = BTreeMap::new();
    metadata.insert("planner".into(), plan.report.planner.clone());
    metadata.insert(
        "selected_message_count".into(),
        plan.report.selected_message_count.to_string(),
    );
    metadata.insert(
        "dropped_message_count".into(),
        plan.report.dropped_message_count.to_string(),
    );
    metadata.insert(
        "selected_tool_count".into(),
        plan.report.selected_tool_count.to_string(),
    );
    push_run_trace(
        state,
        RunTraceEntryKind::TurnPlanned,
        "turn plan selected model inputs and tools",
        refs,
        metadata,
    );
    if let Some(run) = state.current_run.as_mut() {
        run.turn_plan = Some(plan.clone());
    }
    Ok(plan)
}

fn turn_plan_error_to_workflow_error(err: TurnPlanError) -> SessionWorkflowError {
    match err {
        TurnPlanError::EmptySelection => SessionWorkflowError::EmptyMessageRefs,
        TurnPlanError::UnknownTool => SessionWorkflowError::UnknownToolOverride,
    }
}

fn build_turn_inputs(
    prompt_refs: &[String],
    transcript_refs: &[String],
    turn_refs: &[String],
    cause: Option<&RunCause>,
) -> Vec<TurnInput> {
    let mut inputs = Vec::new();
    for (idx, value) in prompt_refs.iter().enumerate() {
        inputs.push(turn_input(
            format!("prompt:{idx}"),
            TurnInputLane::System,
            TurnInputKind::MessageRef,
            TurnPriority::Required,
            value.clone(),
            Some("prompt".into()),
        ));
    }
    if let Some(cause) = cause {
        if let Some(payload_ref) = cause.payload_ref.as_ref() {
            inputs.push(turn_input(
                "cause:payload".into(),
                TurnInputLane::Domain,
                TurnInputKind::MessageRef,
                TurnPriority::High,
                payload_ref.clone(),
                cause.payload_schema.clone(),
            ));
        }
        for (idx, subject) in cause.subject_refs.iter().enumerate() {
            if let Some(value) = subject.ref_.as_ref() {
                inputs.push(turn_input(
                    format!("cause:subject:{idx}"),
                    TurnInputLane::Domain,
                    TurnInputKind::MessageRef,
                    TurnPriority::High,
                    value.clone(),
                    Some(subject.kind.clone()),
                ));
            }
        }
    }
    for (idx, value) in transcript_refs.iter().enumerate() {
        inputs.push(turn_input(
            format!("transcript:{idx}"),
            TurnInputLane::Conversation,
            TurnInputKind::MessageRef,
            TurnPriority::Normal,
            value.clone(),
            Some("transcript".into()),
        ));
    }
    for (idx, value) in turn_refs.iter().enumerate() {
        inputs.push(turn_input(
            format!("turn:{idx}"),
            TurnInputLane::Conversation,
            TurnInputKind::MessageRef,
            TurnPriority::Required,
            value.clone(),
            Some("turn".into()),
        ));
    }
    inputs
}

fn turn_input(
    input_id: String,
    lane: TurnInputLane,
    kind: TurnInputKind,
    priority: TurnPriority,
    content_ref: String,
    source_kind: Option<String>,
) -> TurnInput {
    TurnInput {
        input_id,
        lane,
        kind,
        priority,
        content_ref,
        estimated_tokens: None,
        source_kind,
        source_id: None,
        correlation_id: None,
        tags: Vec::new(),
    }
}

fn build_turn_tool_inputs(
    state: &mut SessionState,
    run_config: Option<&RunConfig>,
) -> Result<Vec<TurnToolInput>, SessionWorkflowError> {
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
            .ok_or(SessionWorkflowError::ToolProfileUnknown)?;
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

    state.tool_profile = configured_profile_id.unwrap_or_default();
    Ok(ordered_names
        .into_iter()
        .map(|tool_id| TurnToolInput {
            tool_id,
            priority: TurnPriority::Normal,
            estimated_tokens: None,
            source_kind: Some("tool_profile".into()),
            source_id: if state.tool_profile.is_empty() {
                None
            } else {
                Some(state.tool_profile.clone())
            },
            tags: Vec::new(),
        })
        .collect())
}

fn selected_tool_refs(state: &SessionState, plan: &TurnPlan) -> Option<Vec<String>> {
    if plan.selected_tool_ids.is_empty() {
        return None;
    }
    Some(
        plan.selected_tool_ids
            .iter()
            .filter_map(|tool_id| state.tool_registry.get(tool_id))
            .map(|tool| tool.tool_ref.clone())
            .collect(),
    )
}

fn turn_tool_choice_to_llm(value: TurnToolChoice) -> aos_effects::builtins::LlmToolChoice {
    match value {
        TurnToolChoice::Auto => aos_effects::builtins::LlmToolChoice::Auto,
        TurnToolChoice::NoneChoice => aos_effects::builtins::LlmToolChoice::NoneChoice,
        TurnToolChoice::Required => aos_effects::builtins::LlmToolChoice::Required,
        TurnToolChoice::Tool { name } => aos_effects::builtins::LlmToolChoice::Tool { name },
    }
}

fn emit_auto_host_session_open(
    state: &mut SessionState,
    out: &mut SessionWorkflowOutput,
) -> Result<(), SessionWorkflowError> {
    let config = effective_host_session_open_config(state)
        .ok_or(SessionWorkflowError::InvalidToolRegistry)?;
    let params = host_session_open_params_from_config(config);
    let params_json =
        serde_json::to_value(&params).map_err(|_| SessionWorkflowError::InvalidToolRegistry)?;
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

fn validate_known_tool_names(
    state: &SessionState,
    names: Option<&[String]>,
) -> Result<(), SessionWorkflowError> {
    if let Some(names) = names {
        for tool_name in names {
            if !state.tool_registry.contains_key(tool_name) {
                return Err(SessionWorkflowError::UnknownToolOverride);
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

fn validate_run_config(config: &RunConfig) -> Result<(), SessionWorkflowError> {
    if config.provider.trim().is_empty() {
        return Err(SessionWorkflowError::MissingProvider);
    }
    if config.model.trim().is_empty() {
        return Err(SessionWorkflowError::MissingModel);
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
    let mut refs = Vec::new();
    if let Some(output_ref) = outcome.as_ref().and_then(|value| value.output_ref.as_ref()) {
        refs.push(trace_ref("output_ref", output_ref.clone()));
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("lifecycle".into(), alloc::format!("{lifecycle:?}"));
    if let Some(failure) = outcome.as_ref().and_then(|value| value.failure.as_ref()) {
        metadata.insert("failure_code".into(), failure.code.clone());
    }
    if let Some(reason) = outcome
        .as_ref()
        .and_then(|value| value.cancelled_reason.as_ref())
    {
        metadata.insert("cancelled_reason".into(), reason.clone());
    }
    if let Some(reason_ref) = outcome
        .as_ref()
        .and_then(|value| value.interrupted_reason_ref.as_ref())
    {
        refs.push(trace_ref("interrupted_reason_ref", reason_ref.clone()));
    }
    push_trace_entry(
        &mut run.trace,
        state.updated_at,
        RunTraceEntryKind::RunFinished,
        "run finished".into(),
        refs,
        metadata,
    );
    let trace_summary = summarize_trace(&run.trace);
    state.run_history.push(RunRecord {
        run_id: run.run_id,
        lifecycle,
        cause: run.cause,
        input_refs: run.input_refs,
        outcome,
        trace_summary,
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
    state.staged_tool_follow_up_turn = None;
    state.pending_llm_turn_refs = None;
    state.queued_steer_refs.clear();
    state.run_interrupt = None;
    state.tool_refs_materialized = false;
    state.in_flight_effects = 0;
}

fn enforce_runtime_limits(
    state: &SessionState,
    limits: SessionRuntimeLimits,
) -> Result<(), SessionWorkflowError> {
    if let Some(max) = limits.max_pending_effects {
        let total_pending = state.pending_effects.len()
            + state.pending_blob_gets.len()
            + state.pending_blob_puts.len();
        if total_pending as u64 > max {
            return Err(SessionWorkflowError::TooManyPendingEffects);
        }
    }
    Ok(())
}

fn stamp_timestamps(state: &mut SessionState, event: &SessionWorkflowEvent) {
    let ts = match event {
        SessionWorkflowEvent::Input(input) => input.observed_at_ns,
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

fn trace_workflow_output(state: &mut SessionState, out: &SessionWorkflowOutput) {
    for effect in &out.effects {
        match effect {
            SessionEffectCommand::LlmGenerate { params, pending } => {
                let mut refs = Vec::new();
                refs.push(trace_ref("params_hash", pending.params_hash.clone()));
                if let Some(issuer_ref) = pending.issuer_ref.as_ref() {
                    refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
                }
                refs.extend(
                    params
                        .message_refs
                        .iter()
                        .map(|value| trace_ref("message_ref", value.to_string())),
                );
                if let Some(tool_refs) = params.runtime.tool_refs.as_ref() {
                    refs.extend(
                        tool_refs
                            .iter()
                            .map(|value| trace_ref("tool_ref", value.to_string())),
                    );
                }
                let mut metadata = BTreeMap::new();
                metadata.insert("provider".into(), params.provider.clone());
                metadata.insert("model".into(), params.model.clone());
                if let Some(correlation_id) = params.correlation_id.as_ref() {
                    metadata.insert("correlation_id".into(), correlation_id.clone());
                }
                push_run_trace(
                    state,
                    RunTraceEntryKind::LlmRequested,
                    "llm turn requested",
                    refs,
                    metadata,
                );
            }
            SessionEffectCommand::ToolEffect { kind, pending, .. } => {
                let mut refs = vec![trace_ref("params_hash", pending.params_hash.clone())];
                if let Some(issuer_ref) = pending.issuer_ref.as_ref() {
                    refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
                }
                let mut metadata = BTreeMap::new();
                metadata.insert("effect".into(), kind.as_str().into());
                push_run_trace(
                    state,
                    RunTraceEntryKind::EffectEmitted,
                    "tool effect emitted",
                    refs,
                    metadata,
                );
            }
            SessionEffectCommand::BlobPut { pending, .. } => {
                trace_effect_command(state, "sys/blob.put@1", pending);
            }
            SessionEffectCommand::BlobGet { pending, .. } => {
                trace_effect_command(state, "sys/blob.get@1", pending);
            }
        }
    }

    for event in &out.domain_events {
        let mut refs = Vec::new();
        if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload_json) {
            refs.push(trace_ref("payload_hash", hash_cbor(&payload)));
        }
        let mut metadata = BTreeMap::new();
        metadata.insert("schema".into(), event.schema.into());
        push_run_trace(
            state,
            RunTraceEntryKind::DomainEventEmitted,
            "domain event emitted",
            refs,
            metadata,
        );
    }
}

fn trace_effect_command(
    state: &mut SessionState,
    effect: &str,
    pending: &aos_wasm_sdk::PendingEffect,
) {
    let mut refs = vec![trace_ref("params_hash", pending.params_hash.clone())];
    if let Some(issuer_ref) = pending.issuer_ref.as_ref() {
        refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("effect".into(), effect.into());
    push_run_trace(
        state,
        RunTraceEntryKind::EffectEmitted,
        "effect emitted",
        refs,
        metadata,
    );
}

fn trace_receipt_envelope(state: &mut SessionState, envelope: &EffectReceiptEnvelope) {
    let mut refs = Vec::new();
    if let Some(params_hash) = envelope.params_hash.as_ref() {
        refs.push(trace_ref("params_hash", params_hash.clone()));
    }
    if let Some(issuer_ref) = envelope.issuer_ref.as_ref() {
        refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
    }
    refs.push(trace_ref("intent_id", envelope.intent_id.clone()));
    let mut metadata = BTreeMap::new();
    metadata.insert("effect".into(), envelope.effect.clone());
    metadata.insert("status".into(), envelope.status.clone());
    metadata.insert("emitted_at_seq".into(), envelope.emitted_at_seq.to_string());
    push_run_trace(
        state,
        RunTraceEntryKind::ReceiptSettled,
        "effect receipt admitted",
        refs,
        metadata,
    );
}

fn trace_receipt_rejected(state: &mut SessionState, rejected: &EffectReceiptRejected) {
    let mut refs = Vec::new();
    if let Some(params_hash) = rejected.params_hash.as_ref() {
        refs.push(trace_ref("params_hash", params_hash.clone()));
    }
    if let Some(issuer_ref) = rejected.issuer_ref.as_ref() {
        refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
    }
    refs.push(trace_ref("intent_id", rejected.intent_id.clone()));
    let mut metadata = BTreeMap::new();
    metadata.insert("effect".into(), rejected.effect.clone());
    metadata.insert("status".into(), rejected.status.clone());
    metadata.insert("error_code".into(), rejected.error_code.clone());
    metadata.insert("emitted_at_seq".into(), rejected.emitted_at_seq.to_string());
    push_run_trace(
        state,
        RunTraceEntryKind::ReceiptSettled,
        "effect receipt rejected",
        refs,
        metadata,
    );
}

fn trace_stream_frame(state: &mut SessionState, frame: &aos_wasm_sdk::EffectStreamFrameEnvelope) {
    let mut refs = Vec::new();
    if let Some(params_hash) = frame.params_hash.as_ref() {
        refs.push(trace_ref("params_hash", params_hash.clone()));
    }
    if let Some(issuer_ref) = frame.issuer_ref.as_ref() {
        refs.push(trace_ref("issuer_ref", issuer_ref.clone()));
    }
    if let Some(payload_ref) = frame.payload_ref.as_ref() {
        refs.push(trace_ref("payload_ref", payload_ref.clone()));
    }
    refs.push(trace_ref("intent_id", frame.intent_id.clone()));
    let mut metadata = BTreeMap::new();
    metadata.insert("effect".into(), frame.effect.clone());
    metadata.insert("kind".into(), frame.kind.clone());
    metadata.insert("seq".into(), frame.seq.to_string());
    metadata.insert("payload_size".into(), frame.payload.len().to_string());
    push_run_trace(
        state,
        RunTraceEntryKind::StreamFrameObserved,
        "effect stream frame observed",
        refs,
        metadata,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        CauseRef, HostSessionOpenConfig, HostSessionStatus, HostTargetConfig, RunCauseOrigin,
        RunId, SessionId, SessionInput, ToolCallObserved, ToolProfileBuilder, ToolRegistryBuilder,
        TurnInput, TurnInputKind, TurnInputLane, TurnPrerequisiteKind, TurnPriority, TurnReport,
        TurnToolChoice, local_coding_agent_tool_profile_for_provider,
        local_coding_agent_tool_profiles, local_coding_agent_tool_registry,
        tool_bundle_host_sandbox,
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

    fn session_input(observed_at_ns: u64, input: SessionInputKind) -> SessionWorkflowEvent {
        SessionWorkflowEvent::Input(SessionInput {
            session_id: SessionId("s-1".into()),
            observed_at_ns,
            input,
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
        session_input(
            ts,
            SessionInputKind::RunRequested {
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
        assert!(state.pending_llm_turn_refs.is_some());
        assert_eq!(state.in_flight_effects, 1);
        let plan = state
            .current_run
            .as_ref()
            .and_then(|run| run.turn_plan.as_ref())
            .expect("turn plan");
        assert!(matches!(
            plan.prerequisites.first().map(|item| &item.kind),
            Some(TurnPrerequisiteKind::OpenHostSession)
        ));
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
        assert!(
            state
                .current_run
                .as_ref()
                .and_then(|run| run.turn_plan.as_ref())
                .is_some_and(|plan| plan
                    .report
                    .unresolved
                    .iter()
                    .any(|item| item == "host_session_not_ready"))
        );
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
        state.lifecycle = SessionLifecycle::Running;
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
    fn run_request_records_turn_plan_and_preserves_prompt_refs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        let prompt_ref = fake_hash('b');
        let input_ref = fake_hash('a');

        let out = apply_session_workflow_event(
            &mut state,
            &session_input(
                1,
                SessionInputKind::RunRequested {
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
            .and_then(|run| run.turn_plan.as_ref())
            .expect("turn plan");
        assert_eq!(plan.message_refs, vec![prompt_ref, input_ref]);
        assert_eq!(plan.report.planner, "aos.agent/default-turn");
        assert_eq!(state.turn_state.last_report.as_ref(), Some(&plan.report));
    }

    #[test]
    fn run_request_records_turn_and_llm_trace_entries() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("reduce");

        let trace = &state.current_run.as_ref().expect("current run").trace;
        let kinds = trace
            .entries
            .iter()
            .map(|entry| entry.kind.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                RunTraceEntryKind::RunStarted,
                RunTraceEntryKind::TurnPlanned,
                RunTraceEntryKind::LlmRequested,
            ]
        );
        assert!(trace.entries.iter().any(|entry| {
            matches!(entry.kind, RunTraceEntryKind::LlmRequested)
                && entry
                    .refs
                    .iter()
                    .any(|trace_ref| trace_ref.kind == "message_ref")
        }));
    }

    #[test]
    fn completed_run_keeps_bounded_trace_summary() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        if let Some(run) = state.current_run.as_mut() {
            run.trace.max_entries = 2;
        }
        push_run_trace(
            &mut state,
            RunTraceEntryKind::Custom {
                kind: "test/extra".into(),
            },
            "extra trace entry",
            Vec::new(),
            BTreeMap::new(),
        );
        apply_session_workflow_event(
            &mut state,
            &session_input(2, SessionInputKind::RunCompleted),
        )
        .expect("complete");

        let record = state.run_history.first().expect("run record");
        assert_eq!(record.trace_summary.entry_count, 2);
        assert!(record.trace_summary.dropped_entries > 0);
        assert!(matches!(
            record.trace_summary.last_kind,
            Some(RunTraceEntryKind::RunFinished)
        ));
    }

    fn pending_llm_turn_state(input_ref: String) -> SessionState {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());
        let run_id = RunId {
            session_id: state.session_id.clone(),
            run_seq: 1,
        };
        let config = RunConfig {
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            max_tokens: Some(512),
            ..RunConfig::default()
        };
        state.active_run_id = Some(run_id.clone());
        state.active_run_config = Some(config.clone());
        state.lifecycle = SessionLifecycle::Running;
        state.current_run = Some(RunState {
            run_id,
            lifecycle: RunLifecycle::Running,
            cause: RunCause::direct_input(input_ref.clone()),
            config,
            input_refs: vec![input_ref.clone()],
            pending_llm_turn_refs: Some(vec![input_ref.clone()]),
            ..RunState::default()
        });
        state.transcript_message_refs = vec![input_ref.clone()];
        state.pending_llm_turn_refs = Some(vec![input_ref]);
        state
    }

    #[test]
    fn steer_ref_is_injected_into_next_llm_turn() {
        let input_ref = fake_hash('a');
        let steer_ref = fake_hash('b');
        let mut state = pending_llm_turn_state(input_ref.clone());

        apply_session_workflow_event(
            &mut state,
            &session_input(
                1,
                SessionInputKind::RunSteerRequested {
                    instruction_ref: steer_ref.clone(),
                },
            ),
        )
        .expect("steer");
        let mut out = SessionWorkflowOutput::default();
        dispatch_pending_llm_turn(&mut state, &mut out).expect("dispatch");

        let message_refs = out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::LlmGenerate { params, .. } => Some(
                    params
                        .message_refs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .expect("llm params");
        assert_eq!(message_refs, vec![input_ref, steer_ref]);
        assert!(state.queued_steer_refs.is_empty());
        assert!(state.current_run.as_ref().is_some_and(|run| {
            run.trace
                .entries
                .iter()
                .any(|entry| matches!(entry.kind, RunTraceEntryKind::InterventionApplied))
        }));
    }

    #[test]
    fn follow_up_input_queues_and_starts_after_current_run() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(&mut state, &run_request_event(1)).expect("run");
        let follow_up = fake_hash('b');
        apply_session_workflow_event(
            &mut state,
            &session_input(
                2,
                SessionInputKind::FollowUpInputAppended {
                    input_ref: follow_up.clone(),
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
        .expect("queue follow-up");
        assert_eq!(state.queued_follow_up_runs.len(), 1);

        apply_session_workflow_event(
            &mut state,
            &session_input(3, SessionInputKind::RunCompleted),
        )
        .expect("complete and start next");

        assert_eq!(state.queued_follow_up_runs.len(), 0);
        assert_eq!(state.run_history.len(), 1);
        assert_eq!(
            state.current_run.as_ref().map(|run| run.run_id.run_seq),
            Some(2)
        );
        assert_eq!(
            state.transcript_message_refs,
            vec![fake_hash('a'), follow_up]
        );
    }

    #[test]
    fn interrupt_blocks_pending_llm_turn_and_finishes_when_quiescent() {
        let input_ref = fake_hash('a');
        let reason_ref = fake_hash('b');
        let mut state = pending_llm_turn_state(input_ref);

        let out = apply_session_workflow_event(
            &mut state,
            &session_input(
                1,
                SessionInputKind::RunInterruptRequested {
                    reason_ref: Some(reason_ref.clone()),
                },
            ),
        )
        .expect("interrupt");

        assert!(out.effects.is_empty());
        assert!(state.current_run.is_none());
        let record = state.run_history.first().expect("run record");
        assert_eq!(record.lifecycle, RunLifecycle::Interrupted);
        assert_eq!(
            record
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.interrupted_reason_ref.as_ref()),
            Some(&reason_ref)
        );
    }

    struct RepoBootstrapFirstPlanner;

    impl TurnPlanner for RepoBootstrapFirstPlanner {
        fn build_turn(&self, request: TurnRequest<'_>) -> Result<TurnPlan, TurnPlanError> {
            let mut selected_refs = Vec::new();
            let mut selected_count = 0_u64;
            let mut dropped_count = 0_u64;

            for input in &request.state.pinned_inputs {
                let is_repo_bootstrap = input.source_kind.as_deref() == Some("repo_bootstrap")
                    || matches!(
                        &input.lane,
                        TurnInputLane::Custom { kind } if kind == "repo_bootstrap"
                    );
                if is_repo_bootstrap {
                    selected_refs.push(input.content_ref.clone());
                    selected_count = selected_count.saturating_add(1);
                    break;
                }
            }

            for input in request.inputs {
                if matches!(input.lane, TurnInputLane::Conversation)
                    && matches!(input.priority, TurnPriority::Required)
                {
                    selected_refs.push(input.content_ref.clone());
                    selected_count = selected_count.saturating_add(1);
                } else if matches!(input.lane, TurnInputLane::Conversation) {
                    dropped_count = dropped_count.saturating_add(1);
                }
            }

            if selected_refs.is_empty() {
                return Err(TurnPlanError::EmptySelection);
            }

            Ok(TurnPlan {
                message_refs: selected_refs,
                selected_tool_ids: request
                    .tools
                    .iter()
                    .take(1)
                    .map(|tool| tool.tool_id.clone())
                    .collect(),
                tool_choice: Some(TurnToolChoice::Auto),
                response_format_ref: None,
                provider_options_ref: None,
                prerequisites: Vec::new(),
                state_updates: Vec::new(),
                report: TurnReport {
                    planner: "test/repo-bootstrap-first".into(),
                    selected_message_count: selected_count,
                    dropped_message_count: dropped_count,
                    selected_tool_count: request.tools.iter().take(1).count() as u64,
                    dropped_tool_count: request.tools.len().saturating_sub(1) as u64,
                    token_estimate: Default::default(),
                    budget: request.budget,
                    decision_codes: vec!["selected repo bootstrap before current turn".into()],
                    unresolved: Vec::new(),
                },
            })
        }
    }

    #[test]
    fn custom_turn_planner_can_reuse_llm_dispatch() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        let bootstrap_ref = fake_hash('b');
        let transcript_ref = fake_hash('c');
        let input_ref = fake_hash('a');
        state.turn_state.pinned_inputs.push(TurnInput {
            input_id: "repo-bootstrap".into(),
            kind: TurnInputKind::MessageRef,
            lane: TurnInputLane::Custom {
                kind: "repo_bootstrap".into(),
            },
            priority: TurnPriority::Required,
            content_ref: bootstrap_ref.clone(),
            estimated_tokens: None,
            source_kind: Some("repo_bootstrap".into()),
            source_id: Some("repo://main".into()),
            correlation_id: None,
            tags: Vec::new(),
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
        state.pending_llm_turn_refs = Some(vec![input_ref.clone()]);

        let mut out = SessionWorkflowOutput::default();
        dispatch_pending_llm_turn_with_planner(
            &mut state,
            &mut out,
            &RepoBootstrapFirstPlanner,
            TurnBudget {
                max_message_refs: Some(2),
                reserve_output_tokens: Some(128),
                max_input_tokens: None,
                max_tool_refs: Some(1),
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
            .and_then(|run| run.turn_plan.as_ref())
            .expect("turn plan");
        assert_eq!(plan.message_refs, vec![bootstrap_ref, input_ref.clone()]);
        assert_eq!(plan.report.planner, "test/repo-bootstrap-first");
        assert_eq!(plan.report.dropped_message_count, 1);
        assert_eq!(state.turn_state.last_report.as_ref(), Some(&plan.report));
        assert_eq!(
            state.transcript_message_refs,
            vec![transcript_ref, input_ref]
        );
    }

    #[test]
    fn session_can_exist_with_no_runs() {
        let mut state = SessionState::default();
        state.session_id = SessionId("s-1".into());

        apply_session_workflow_event(
            &mut state,
            &session_input(1, SessionInputKind::SessionOpened { config: None }),
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
        apply_session_workflow_event(
            &mut state,
            &session_input(2, SessionInputKind::RunCompleted),
        )
        .expect("complete first run");

        let second_input = fake_hash('b');
        apply_session_workflow_event(
            &mut state,
            &session_input(
                3,
                SessionInputKind::RunRequested {
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
            &session_input(
                2,
                SessionInputKind::RunFailed {
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

        apply_session_workflow_event(
            &mut state,
            &session_input(1, SessionInputKind::SessionPaused),
        )
        .expect("pause");
        assert_eq!(state.status, SessionStatus::Paused);
        assert!(state.current_run.is_none());
        assert!(apply_session_workflow_event(&mut state, &run_request_event(2)).is_err());

        apply_session_workflow_event(
            &mut state,
            &session_input(3, SessionInputKind::SessionResumed),
        )
        .expect("resume");
        apply_session_workflow_event(&mut state, &run_request_event(4)).expect("run");
        apply_session_workflow_event(
            &mut state,
            &session_input(5, SessionInputKind::RunCompleted),
        )
        .expect("complete");

        apply_session_workflow_event(
            &mut state,
            &session_input(6, SessionInputKind::SessionClosed),
        )
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
            &session_input(
                1,
                SessionInputKind::RunStartRequested {
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
            &session_input(
                1,
                SessionInputKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");

        apply_session_workflow_event(&mut state, &run_request_event(2)).expect("run");

        let tools = state
            .current_run
            .as_ref()
            .and_then(|run| run.turn_plan.as_ref())
            .map(|plan| plan.selected_tool_ids.clone())
            .expect("turn plan");
        assert!(tools.contains(&"host.exec".into()));
        assert!(tools.contains(&"host.fs.apply_patch".into()));
    }

    #[test]
    fn tool_calls_observed_builds_deterministic_plan_and_ignores_disabled() {
        let mut state = local_coding_state();
        state.session_config.default_tool_disable = Some(vec!["host.exec".into()]);
        apply_session_workflow_event(
            &mut state,
            &session_input(
                1,
                SessionInputKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )
        .expect("host session ready");
        apply_session_workflow_event(&mut state, &run_request_event(2)).expect("run");

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

        let mut out = SessionWorkflowOutput::default();
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
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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

        let mut last_out = SessionWorkflowOutput::default();
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
    fn deterministic_fixture_asserts_turn_plan_end_to_end() {
        let first = run_turn_plan_fixture().expect("first fixture run");
        let second = run_turn_plan_fixture().expect("second fixture run");

        assert_eq!(first, second);
        assert_eq!(
            first.initial_message_refs,
            vec![fake_hash('b'), fake_hash('a')]
        );
        assert!(
            first
                .initial_selected_tool_ids
                .contains(&"host.exec".into())
        );
        assert!(
            first
                .initial_selected_tool_ids
                .contains(&"host.fs.apply_patch".into())
        );
        assert!(first.initial_prerequisite_kinds.contains(&alloc::format!(
            "{:?}",
            TurnPrerequisiteKind::MaterializeToolDefinitions
        )));
        assert_eq!(first.final_message_refs, first.initial_message_refs);
        assert_eq!(
            first.final_selected_tool_ids,
            first.initial_selected_tool_ids
        );
        assert!(first.final_prerequisite_kinds.is_empty());
        assert_eq!(first.llm_message_refs, first.final_message_refs);
        assert_eq!(
            first.llm_tool_ref_count,
            first.final_selected_tool_ids.len()
        );
        assert_eq!(
            first.trace_kinds,
            vec!["RunStarted", "TurnPlanned", "TurnPlanned", "LlmRequested"]
        );
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TurnPlanFixtureObservation {
        initial_message_refs: Vec<String>,
        initial_selected_tool_ids: Vec<String>,
        initial_prerequisite_kinds: Vec<String>,
        final_message_refs: Vec<String>,
        final_selected_tool_ids: Vec<String>,
        final_prerequisite_kinds: Vec<String>,
        llm_message_refs: Vec<String>,
        llm_tool_ref_count: usize,
        trace_kinds: Vec<&'static str>,
    }

    fn run_turn_plan_fixture() -> Result<TurnPlanFixtureObservation, SessionWorkflowError> {
        let mut state = local_coding_state();
        state.session_id = SessionId("s-1".into());
        apply_session_workflow_event(
            &mut state,
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
                    host_session_id: Some("hs_1".into()),
                    host_session_status: Some(HostSessionStatus::Ready),
                },
            ),
        )?;

        let prompt_ref = fake_hash('b');
        let input_ref = fake_hash('a');
        let out = apply_session_workflow_event(
            &mut state,
            &session_input(
                1,
                SessionInputKind::RunRequested {
                    input_ref,
                    run_overrides: Some(SessionConfig {
                        provider: "openai".into(),
                        model: "gpt-5.2".into(),
                        reasoning_effort: None,
                        max_tokens: Some(512),
                        default_prompt_refs: Some(vec![prompt_ref]),
                        default_tool_profile: None,
                        default_tool_enable: None,
                        default_tool_disable: None,
                        default_tool_force: None,
                        default_host_session_open: None,
                    }),
                },
            ),
        )?;
        let initial_plan = state
            .current_run
            .as_ref()
            .and_then(|run| run.turn_plan.clone())
            .expect("initial turn plan");
        let blob_put_hashes = out
            .effects
            .iter()
            .filter(|effect| matches!(effect, SessionEffectCommand::BlobPut { .. }))
            .map(|effect| effect.params_hash().to_string())
            .collect::<Vec<_>>();
        assert!(!blob_put_hashes.is_empty(), "expected tool definition puts");

        let mut last_out = SessionWorkflowOutput::default();
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
            )?;
        }

        let final_plan = state
            .current_run
            .as_ref()
            .and_then(|run| run.turn_plan.clone())
            .expect("final turn plan");
        let (llm_message_refs, llm_tool_ref_count) = last_out
            .effects
            .iter()
            .find_map(|effect| match effect {
                SessionEffectCommand::LlmGenerate { params, .. } => Some((
                    params
                        .message_refs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    params.runtime.tool_refs.as_ref().map_or(0, Vec::len),
                )),
                _ => None,
            })
            .expect("llm.generate emitted after materialization");
        let trace_kinds = state
            .current_run
            .as_ref()
            .expect("current run")
            .trace
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    RunTraceEntryKind::RunStarted
                        | RunTraceEntryKind::TurnPlanned
                        | RunTraceEntryKind::LlmRequested
                )
            })
            .map(|entry| trace_kind_name(&entry.kind))
            .collect::<Vec<_>>();

        Ok(TurnPlanFixtureObservation {
            initial_message_refs: initial_plan.message_refs,
            initial_selected_tool_ids: initial_plan.selected_tool_ids,
            initial_prerequisite_kinds: initial_plan
                .prerequisites
                .iter()
                .map(|item| alloc::format!("{:?}", item.kind))
                .collect(),
            final_message_refs: final_plan.message_refs,
            final_selected_tool_ids: final_plan.selected_tool_ids,
            final_prerequisite_kinds: final_plan
                .prerequisites
                .iter()
                .map(|item| alloc::format!("{:?}", item.kind))
                .collect(),
            llm_message_refs,
            llm_tool_ref_count,
            trace_kinds,
        })
    }

    fn trace_kind_name(kind: &RunTraceEntryKind) -> &'static str {
        match kind {
            RunTraceEntryKind::RunStarted => "RunStarted",
            RunTraceEntryKind::TurnPlanned => "TurnPlanned",
            RunTraceEntryKind::LlmRequested => "LlmRequested",
            RunTraceEntryKind::LlmReceived => "LlmReceived",
            RunTraceEntryKind::ToolCallsObserved => "ToolCallsObserved",
            RunTraceEntryKind::ToolBatchPlanned => "ToolBatchPlanned",
            RunTraceEntryKind::EffectEmitted => "EffectEmitted",
            RunTraceEntryKind::DomainEventEmitted => "DomainEventEmitted",
            RunTraceEntryKind::StreamFrameObserved => "StreamFrameObserved",
            RunTraceEntryKind::ReceiptSettled => "ReceiptSettled",
            RunTraceEntryKind::InterventionRequested => "InterventionRequested",
            RunTraceEntryKind::InterventionApplied => "InterventionApplied",
            RunTraceEntryKind::RunFinished => "RunFinished",
            RunTraceEntryKind::Custom { .. } => "Custom",
        }
    }

    #[test]
    fn llm_receipt_settles_by_issuer_ref_when_params_hash_differs() {
        let mut state = local_coding_state();
        state.session_id = SessionId("s-1".into());
        apply_session_workflow_event(
            &mut state,
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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
        let mut out2 = SessionWorkflowOutput::default();
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
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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

        let mut out2 = SessionWorkflowOutput::default();
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
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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
        let mut out = SessionWorkflowOutput::default();
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
        let mut out = SessionWorkflowOutput::default();
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
            &session_input(
                0,
                SessionInputKind::HostSessionUpdated {
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
        let mut out = SessionWorkflowOutput::default();
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
