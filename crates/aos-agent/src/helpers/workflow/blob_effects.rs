use super::*;

fn enqueue_pending_blob_effect<P>(
    pending_entries: &mut BTreeMap<String, Vec<P>>,
    pending_effects: &mut aos_wasm_sdk::PendingEffects,
    pending: PendingEffect,
    pending_entry: P,
    make_effect: impl FnOnce(String) -> SessionEffectCommand,
    out: &mut SessionReduceOutput,
) -> String {
    let params_hash = pending.params_hash.clone();
    let already_pending = pending_entries.contains_key(&params_hash);
    pending_entries
        .entry(params_hash.clone())
        .or_default()
        .push(pending_entry);
    if !already_pending {
        pending_effects.insert(pending);
        out.effects.push(make_effect(params_hash.clone()));
    }
    params_hash
}

pub(super) fn has_pending_tool_definition_puts(state: &SessionState) -> bool {
    state.pending_blob_puts.values().any(|items| {
        items
            .iter()
            .any(|pending| matches!(pending.kind, PendingBlobPutKind::ToolDefinition { .. }))
    })
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

pub(super) fn enqueue_blob_get(
    state: &mut SessionState,
    blob_ref: HashRef,
    kind: PendingBlobGetKind,
    out: &mut SessionReduceOutput,
) -> Result<String, SessionReduceError> {
    let params = BlobGetParams { blob_ref };
    let pending = pending_effect_from_params(state, "blob.get", &params, Some("blob"));
    Ok(enqueue_pending_blob_effect(
        &mut state.pending_blob_gets,
        &mut state.pending_effects,
        pending,
        PendingBlobGet {
            kind,
            emitted_at_ns: state.updated_at,
        },
        |params_hash| SessionEffectCommand::BlobGet {
            params,
            cap_slot: Some("blob".into()),
            params_hash,
        },
        out,
    ))
}

pub(super) fn enqueue_blob_put(
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
    enqueue_pending_blob_effect(
        &mut state.pending_blob_puts,
        &mut state.pending_effects,
        pending,
        PendingBlobPut {
            kind,
            emitted_at_ns: state.updated_at,
        },
        |params_hash| SessionEffectCommand::BlobPut {
            params,
            cap_slot: Some("blob".into()),
            params_hash,
        },
        out,
    )
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

fn on_llm_output_blob(
    state: &mut SessionState,
    receipt: BlobGetReceipt,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
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

fn on_llm_tool_calls_blob(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    receipt: BlobGetReceipt,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
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
    super::tool_batch::on_tool_calls_observed(
        state,
        envelope.intent_id.as_str(),
        None,
        &observed,
        out,
    )?;
    Ok(true)
}

fn on_tool_call_arguments_blob(
    state: &mut SessionState,
    tool_batch_id: crate::contracts::ToolBatchId,
    call_id: String,
    receipt: Option<BlobGetReceipt>,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(receipt) = receipt else {
        if let Some(batch) = state.active_tool_batch.as_mut()
            && batch.tool_batch_id == tool_batch_id
        {
            super::tool_batch::fail_tool_call(
                batch,
                &call_id,
                "tool_arguments_ref_decode_failed",
                "failed to decode blob.get receipt payload",
            );
        }
        super::tool_batch::dispatch_next_ready_tool_group(state, out)?;
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
                super::tool_batch::fail_tool_call(
                    batch,
                    &call_id,
                    "tool_arguments_not_json",
                    "tool arguments blob must contain JSON",
                );
            }
            super::tool_batch::dispatch_next_ready_tool_group(state, out)?;
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
        super::tool_batch::set_tool_call_status(batch, &call_id, ToolCallStatus::Queued);
        let _ = batch.execution.rewind_to_group_containing(&call_id);
    }
    super::tool_batch::dispatch_next_ready_tool_group(state, out)?;
    Ok(true)
}

fn on_tool_result_blob(
    state: &mut SessionState,
    tool_batch_id: crate::contracts::ToolBatchId,
    call_id: String,
    blob_ref: String,
    receipt: Option<BlobGetReceipt>,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
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
        super::tool_batch::set_tool_call_status(batch, &call_id, ToolCallStatus::Succeeded);
    }

    super::tool_batch::dispatch_next_ready_tool_group(state, out)?;
    Ok(true)
}

pub(super) fn handle_pending_blob_get_receipt(
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
            on_llm_output_blob(state, receipt, out)
        }
        PendingBlobGetKind::LlmToolCalls => {
            let Some(receipt) = receipt else {
                fail_run(state)?;
                return Ok(true);
            };
            on_llm_tool_calls_blob(state, envelope, receipt, out)
        }
        PendingBlobGetKind::ToolCallArguments {
            tool_batch_id,
            call_id,
        } => {
            if failed {
                if let Some(batch) = state.active_tool_batch.as_mut()
                    && batch.tool_batch_id == tool_batch_id
                {
                    super::tool_batch::fail_tool_call(
                        batch,
                        &call_id,
                        "tool_arguments_ref_failed",
                        "blob.get for tool arguments failed",
                    );
                }
                super::tool_batch::dispatch_next_ready_tool_group(state, out)?;
                return Ok(true);
            }
            on_tool_call_arguments_blob(state, tool_batch_id, call_id, receipt, out)
        }
        PendingBlobGetKind::ToolResultBlob {
            tool_batch_id,
            call_id,
            blob_ref,
        } => on_tool_result_blob(state, tool_batch_id, call_id, blob_ref, receipt, out),
    }
}

pub(super) fn handle_pending_blob_put_receipt(
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
