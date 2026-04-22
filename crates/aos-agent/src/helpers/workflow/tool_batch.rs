use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use aos_air_types::HashRef;
use aos_effects::builtins::{BlobGetParams, BlobPutParams};
use aos_wasm_sdk::{PendingBatch, PendingEffect, PendingEffectSet};

use crate::contracts::{
    ActiveToolBatch, PendingBlobGetKind, PlannedToolCall, SessionState, ToolBatchPlan,
    ToolCallObserved, ToolCallStatus, ToolExecutionPlan, ToolExecutor,
};
use crate::helpers::{SessionEffectCommand, SessionReduceOutput, allocate_tool_batch_id};
use crate::tools::supported::workspace::{self, WorkspaceAction};
use crate::tools::{
    ToolEffectOp, map_tool_arguments_to_effect_params, map_tool_receipt_to_llm_result,
};

use super::blob_effects::enqueue_blob_get;
use super::{
    CompletedToolBatch, RunToolBatch, SessionReduceError, StartedToolBatch, ToolBatchReceiptMatch,
    hash_cbor, hash_tool_plan, pending_effect_lookup_err_to_session_err,
    recompute_in_flight_effects, refresh_effective_tools,
};

pub(super) fn build_tool_execution(
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

pub(super) fn set_tool_call_status(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    status: ToolCallStatus,
) {
    if status.is_terminal() {
        let _ = batch.execution.mark_terminal(call_id);
    }
    batch.call_status.insert(call_id.clone(), status);
}

pub(super) fn fail_tool_call(
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

pub(super) fn run_tool_batch(
    state: &mut SessionState,
    request: RunToolBatch<'_>,
    out: &mut SessionReduceOutput,
) -> Result<StartedToolBatch, SessionReduceError> {
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

    let (plan, call_status) = plan_tool_batch(state, request.calls);
    state.last_tool_plan_hash = Some(hash_tool_plan(&plan));
    let execution = build_tool_execution(plan.execution_plan.groups.clone(), &call_status);
    let started = StartedToolBatch {
        tool_batch_id: tool_batch_id.clone(),
        plan: plan.clone(),
    };

    state.active_tool_batch = Some(ActiveToolBatch {
        tool_batch_id,
        intent_id: request.intent_id.into(),
        params_hash: request.params_hash.cloned(),
        plan,
        call_status,
        pending_effects: PendingEffectSet::new(),
        execution,
        llm_results: BTreeMap::new(),
        results_ref: None,
    });

    advance_tool_batch(state, out)?;
    Ok(started)
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

pub(super) fn advance_tool_batch(
    state: &mut SessionState,
    out: &mut SessionReduceOutput,
) -> Result<Option<CompletedToolBatch>, SessionReduceError> {
    loop {
        let Some(mut batch) = state.active_tool_batch.take() else {
            return Ok(None);
        };
        batch.execution.advance_completed();
        if batch.execution.is_complete() {
            state.active_tool_batch = Some(batch);
            recompute_in_flight_effects(state);
            return Ok(take_completed_tool_batch(state));
        }

        let group = batch
            .execution
            .current_group_keys()
            .map(|group| group.to_vec())
            .unwrap_or_default();

        let runtime_ctx = state.tool_runtime_context.clone();
        let mut emitted_for_group = 0usize;
        for call_id in group {
            let Some(status) = batch.call_status.get(&call_id).cloned() else {
                continue;
            };
            let Some(planned) = batch
                .plan
                .planned_calls
                .iter()
                .find(|call| call.call_id == call_id)
                .cloned()
            else {
                continue;
            };

            let pending_composite = matches!(status, ToolCallStatus::Pending)
                && workspace::is_workspace_mapper(planned.mapper)
                && !batch.pending_effects.contains_key(&call_id)
                && batch.llm_results.contains_key(&call_id);
            if status != ToolCallStatus::Queued && !pending_composite {
                continue;
            }

            match &planned.executor {
                ToolExecutor::HostLoop { .. } => {
                    set_tool_call_status(&mut batch, &call_id, ToolCallStatus::Pending);
                    continue;
                }
                ToolExecutor::Effect { .. } | ToolExecutor::DomainEvent { .. } => {}
            };

            let arguments_json = if !planned.arguments_json.trim().is_empty() {
                planned.arguments_json.clone()
            } else if status == ToolCallStatus::Queued
                && let Some(arguments_ref) = planned.arguments_ref.clone()
            {
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
                let already_pending = state.pending_blob_gets.contains(&blob_get_hash)
                    || out.effects.iter().any(|effect| {
                        matches!(effect, SessionEffectCommand::BlobGet { .. })
                            && effect.params_hash() == blob_get_hash
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

            if workspace::is_workspace_mapper(planned.mapper) {
                match advance_workspace_tool_call(
                    &mut batch,
                    &call_id,
                    &planned,
                    arguments_json.as_str(),
                    status == ToolCallStatus::Queued,
                    state.updated_at,
                    out,
                ) {
                    Ok(emitted) => {
                        emitted_for_group = emitted_for_group.saturating_add(emitted as usize);
                    }
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
                    }
                }
                continue;
            }

            let (executor_effect_op, cap_slot) = match &planned.executor {
                ToolExecutor::Effect {
                    effect_op,
                    cap_slot,
                } => (effect_op.clone(), cap_slot.clone()),
                ToolExecutor::HostLoop { .. } => unreachable!(),
                ToolExecutor::DomainEvent { schema } => {
                    fail_tool_call(
                        &mut batch,
                        &call_id,
                        "executor_unsupported",
                        format!(
                            "domain event executor for {} requires a composite mapper: {schema}",
                            planned.tool_name
                        ),
                    );
                    continue;
                }
            };

            let mapped_args = match map_tool_arguments_to_effect_params(
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
            let kind = if let Some(kind) = mapped_args.effect_op {
                kind
            } else if let Some(mapper) =
                crate::tools::mapper_for_effect_op(executor_effect_op.as_str())
            {
                crate::tools::effect_op_for_mapper(mapper)
            } else {
                fail_tool_call(
                    &mut batch,
                    &call_id,
                    "executor_unsupported",
                    format!(
                        "unsupported effect op for wasm emit_raw: {}",
                        executor_effect_op
                    ),
                );
                continue;
            };

            let pending = batch
                .pending_effects
                .begin_with_issuer_ref(
                    call_id.clone(),
                    kind.as_str(),
                    &mapped_args.params_json,
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
            set_tool_call_status(&mut batch, &call_id, ToolCallStatus::Pending);
            emitted_for_group = emitted_for_group.saturating_add(1);

            out.effects.push(SessionEffectCommand::ToolEffect {
                kind,
                params_json: serde_json::to_string(&mapped_args.params_json)
                    .unwrap_or_else(|_| "{}".into()),
                pending,
            });
        }

        let current_group_complete = batch.execution.current_group_keys().is_some_and(|keys| {
            keys.iter().all(|call_id| {
                batch
                    .call_status
                    .get(call_id)
                    .is_some_and(ToolCallStatus::is_terminal)
            })
        });
        state.active_tool_batch = Some(batch);
        recompute_in_flight_effects(state);
        if emitted_for_group > 0 || !current_group_complete {
            return Ok(None);
        }
    }
}

fn insert_fallback_pending_tool_effect(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    kind: ToolEffectOp,
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

pub(super) fn settle_tool_batch_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<ToolBatchReceiptMatch, SessionReduceError> {
    let (call_id, planned, tool_batch_id) = {
        let Some(batch) = state.active_tool_batch.as_mut() else {
            return Ok(ToolBatchReceiptMatch::Unmatched);
        };
        let Some(matched) = batch
            .pending_effects
            .settle(envelope.into())
            .map_err(pending_effect_lookup_err_to_session_err)?
        else {
            return Ok(ToolBatchReceiptMatch::Unmatched);
        };
        let call_id = matched.key;
        let Some(planned) = batch
            .plan
            .planned_calls
            .iter()
            .find(|call| call.call_id == call_id)
            .cloned()
        else {
            return Ok(ToolBatchReceiptMatch::Unmatched);
        };
        (call_id, planned, batch.tool_batch_id.clone())
    };

    if workspace::is_workspace_mapper(planned.mapper) {
        let mapped = {
            let Some(batch) = state.active_tool_batch.as_ref() else {
                return Ok(ToolBatchReceiptMatch::Unmatched);
            };
            let state_json = batch
                .llm_results
                .get(&call_id)
                .map(|result| result.output_json.clone())
                .unwrap_or_default();
            match workspace::resume_tool(
                planned.tool_name.as_str(),
                state_json.as_str(),
                envelope.status.as_str(),
                envelope.receipt_payload.as_slice(),
            ) {
                WorkspaceAction::Emit {
                    effect_op,
                    cap_slot,
                    params_json,
                    state_json,
                } => {
                    emit_workspace_action(
                        state,
                        &tool_batch_id,
                        &call_id,
                        &planned,
                        effect_op,
                        cap_slot,
                        params_json,
                        state_json,
                        out,
                    )?;
                    None
                }
                WorkspaceAction::BlobPut { bytes, state_json } => {
                    emit_workspace_blob_put(
                        state,
                        &tool_batch_id,
                        &call_id,
                        &planned,
                        bytes,
                        state_json,
                        out,
                    )?;
                    None
                }
                WorkspaceAction::EmitEvent { receipt, .. } => Some(receipt),
                WorkspaceAction::Complete(mapped) => Some(mapped),
            }
        };

        if let Some(mapped) = mapped {
            apply_mapped_tool_receipt(
                state,
                tool_batch_id.clone(),
                &call_id,
                &planned,
                mapped,
                out,
            )?;
        }

        let completion = advance_tool_batch(state, out)?;
        return Ok(ToolBatchReceiptMatch::Matched { completion });
    }

    let mapped = map_tool_receipt_to_llm_result(
        planned.mapper,
        planned.tool_name.as_str(),
        envelope.status.as_str(),
        envelope.receipt_payload.as_slice(),
    );
    apply_mapped_tool_receipt(
        state,
        tool_batch_id.clone(),
        &call_id,
        &planned,
        mapped,
        out,
    )?;

    let completion = advance_tool_batch(state, out)?;
    Ok(ToolBatchReceiptMatch::Matched { completion })
}

fn advance_workspace_tool_call(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    planned: &PlannedToolCall,
    arguments_json: &str,
    is_initial: bool,
    emitted_at_ns: u64,
    out: &mut SessionReduceOutput,
) -> Result<bool, crate::tools::types::ToolMappingError> {
    let action = if is_initial {
        workspace::start_tool(
            planned.mapper,
            planned.tool_name.as_str(),
            arguments_json,
            emitted_at_ns,
        )?
    } else {
        let state_json = batch
            .llm_results
            .get(call_id)
            .map(|result| result.output_json.clone())
            .unwrap_or_default();
        workspace::continue_tool(planned.tool_name.as_str(), state_json.as_str())
    };

    match action {
        WorkspaceAction::Complete(mapped) => {
            batch.llm_results.insert(
                call_id.clone(),
                crate::contracts::ToolCallLlmResult {
                    call_id: call_id.clone(),
                    tool_id: planned.tool_id.clone(),
                    tool_name: planned.tool_name.clone(),
                    is_error: mapped.is_error,
                    output_json: mapped.llm_output_json,
                },
            );
            set_tool_call_status(batch, call_id, mapped.status);
            Ok(false)
        }
        WorkspaceAction::Emit {
            effect_op,
            cap_slot,
            params_json,
            state_json,
        } => emit_workspace_action_in_batch(
            batch,
            call_id,
            planned,
            effect_op,
            cap_slot,
            params_json,
            state_json,
            emitted_at_ns,
            out,
        ),
        WorkspaceAction::BlobPut { bytes, state_json } => emit_workspace_blob_put_in_batch(
            batch,
            call_id,
            planned,
            bytes,
            state_json,
            emitted_at_ns,
            out,
        ),
        WorkspaceAction::EmitEvent {
            schema,
            payload_json,
            receipt,
        } => {
            batch.llm_results.insert(
                call_id.clone(),
                crate::contracts::ToolCallLlmResult {
                    call_id: call_id.clone(),
                    tool_id: planned.tool_id.clone(),
                    tool_name: planned.tool_name.clone(),
                    is_error: receipt.is_error,
                    output_json: receipt.llm_output_json,
                },
            );
            set_tool_call_status(batch, call_id, receipt.status);
            out.domain_events
                .push(crate::helpers::SessionDomainEventCommand {
                    schema,
                    payload_json: serde_json::to_string(&payload_json)
                        .unwrap_or_else(|_| "{}".into()),
                });
            Ok(false)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_workspace_action(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &String,
    planned: &PlannedToolCall,
    effect_op: ToolEffectOp,
    cap_slot: &'static str,
    params_json: serde_json::Value,
    state_json: String,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if let Some(batch) = state.active_tool_batch.as_mut()
        && &batch.tool_batch_id == tool_batch_id
    {
        let _ = emit_workspace_action_in_batch(
            batch,
            call_id,
            planned,
            effect_op,
            cap_slot,
            params_json,
            state_json,
            state.updated_at,
            out,
        );
    }
    Ok(())
}

fn emit_workspace_blob_put(
    state: &mut SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &String,
    planned: &PlannedToolCall,
    bytes: Vec<u8>,
    state_json: String,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
    if let Some(batch) = state.active_tool_batch.as_mut()
        && &batch.tool_batch_id == tool_batch_id
    {
        let _ = emit_workspace_blob_put_in_batch(
            batch,
            call_id,
            planned,
            bytes,
            state_json,
            state.updated_at,
            out,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_workspace_action_in_batch(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    planned: &PlannedToolCall,
    effect_op: ToolEffectOp,
    cap_slot: &'static str,
    params_json: serde_json::Value,
    state_json: String,
    emitted_at_ns: u64,
    out: &mut SessionReduceOutput,
) -> Result<bool, crate::tools::types::ToolMappingError> {
    batch.llm_results.insert(
        call_id.clone(),
        crate::contracts::ToolCallLlmResult {
            call_id: call_id.clone(),
            tool_id: planned.tool_id.clone(),
            tool_name: planned.tool_name.clone(),
            is_error: false,
            output_json: state_json,
        },
    );
    let issuer_ref = workspace_substep_issuer_ref(call_id, effect_op.as_str(), &params_json);
    let pending = batch
        .pending_effects
        .begin_with_issuer_ref(
            call_id.clone(),
            effect_op.as_str(),
            &params_json,
            Some(cap_slot.into()),
            emitted_at_ns,
            Some(issuer_ref.clone()),
        )
        .unwrap_or_else(|_| {
            let fallback = PendingEffect::from_params_with_issuer_ref(
                effect_op.as_str(),
                &params_json,
                Some(cap_slot.into()),
                emitted_at_ns,
                Some(issuer_ref),
            )
            .unwrap_or_else(|_| {
                PendingEffect::new(
                    effect_op.as_str(),
                    String::new(),
                    Some(cap_slot.into()),
                    emitted_at_ns,
                )
            });
            batch
                .pending_effects
                .insert(call_id.clone(), fallback.clone());
            fallback
        });
    set_tool_call_status(batch, call_id, ToolCallStatus::Pending);
    out.effects.push(SessionEffectCommand::ToolEffect {
        kind: effect_op,
        params_json: serde_json::to_string(&params_json).unwrap_or_else(|_| "{}".into()),
        pending,
    });
    Ok(true)
}

fn emit_workspace_blob_put_in_batch(
    batch: &mut ActiveToolBatch,
    call_id: &String,
    planned: &PlannedToolCall,
    bytes: Vec<u8>,
    state_json: String,
    emitted_at_ns: u64,
    out: &mut SessionReduceOutput,
) -> Result<bool, crate::tools::types::ToolMappingError> {
    batch.llm_results.insert(
        call_id.clone(),
        crate::contracts::ToolCallLlmResult {
            call_id: call_id.clone(),
            tool_id: planned.tool_id.clone(),
            tool_name: planned.tool_name.clone(),
            is_error: false,
            output_json: state_json,
        },
    );
    let params = BlobPutParams {
        bytes,
        blob_ref: None,
        refs: Some(Vec::new()),
    };
    let pending = batch
        .pending_effects
        .begin(
            call_id.clone(),
            "sys/blob.put@1",
            &params,
            Some("blob".into()),
            emitted_at_ns,
        )
        .unwrap_or_else(|_| {
            let fallback = PendingEffect::from_params(
                "sys/blob.put@1",
                &params,
                Some("blob".into()),
                emitted_at_ns,
            )
            .unwrap_or_else(|_| {
                PendingEffect::new(
                    "sys/blob.put@1",
                    String::new(),
                    Some("blob".into()),
                    emitted_at_ns,
                )
            });
            batch
                .pending_effects
                .insert(call_id.clone(), fallback.clone());
            fallback
        });
    set_tool_call_status(batch, call_id, ToolCallStatus::Pending);
    out.effects
        .push(SessionEffectCommand::BlobPut { params, pending });
    Ok(true)
}

fn workspace_substep_issuer_ref<T: serde::Serialize>(
    call_id: &str,
    effect_op: &str,
    params: &T,
) -> String {
    format!("{call_id}:{effect_op}:{}", hash_cbor(params))
}

fn apply_mapped_tool_receipt(
    state: &mut SessionState,
    tool_batch_id: crate::contracts::ToolBatchId,
    call_id: &String,
    planned: &PlannedToolCall,
    mapped: crate::tools::ToolMappedReceipt,
    out: &mut SessionReduceOutput,
) -> Result<(), SessionReduceError> {
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
                tool_name: planned.tool_name.clone(),
                is_error: mapped.is_error,
                output_json: mapped.llm_output_json.clone(),
            },
        );
        let initial_status = if !queued_blob_refs.is_empty() {
            ToolCallStatus::Pending
        } else {
            mapped.status.clone()
        };
        set_tool_call_status(batch, call_id, initial_status);
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
    Ok(())
}

pub(super) fn collect_blob_refs_from_output_json(output_json: &str) -> Vec<String> {
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

pub(super) fn take_completed_tool_batch(state: &mut SessionState) -> Option<CompletedToolBatch> {
    let batch = state.active_tool_batch.as_mut()?;
    batch.execution.advance_completed();
    if !batch.execution.is_complete() || !batch.is_settled() || batch.results_ref.is_some() {
        return None;
    }

    let mut ordered_results = Vec::new();
    for observed in &batch.plan.observed_calls {
        if let Some(result) = batch.llm_results.get(&observed.call_id) {
            ordered_results.push(result.clone());
        }
    }
    let accepted_calls = batch
        .plan
        .planned_calls
        .iter()
        .filter(|planned| planned.accepted)
        .cloned()
        .collect::<Vec<_>>();
    let results_ref = hash_cbor(&ordered_results);
    batch.results_ref = Some(results_ref.clone());
    Some(CompletedToolBatch {
        tool_batch_id: batch.tool_batch_id.clone(),
        accepted_calls,
        ordered_results,
        results_ref,
    })
}
