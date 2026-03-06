use super::*;

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

pub(super) fn on_tool_calls_observed(
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

pub(super) fn dispatch_next_ready_tool_group(
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
                let already_pending = state.pending_blob_gets.contains(&blob_get_hash)
                    || out.effects.iter().any(|effect| {
                        matches!(
                            effect,
                            SessionEffectCommand::BlobGet { params_hash, .. }
                                if params_hash == &blob_get_hash
                        )
                    });
                super::blob_effects::enqueue_blob_get(state, blob_get.blob_ref, pending_kind, out)?;
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

pub(super) fn settle_tool_call_from_receipt(
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
        super::blob_effects::enqueue_blob_get(
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
        super::blob_effects::enqueue_blob_put(
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
