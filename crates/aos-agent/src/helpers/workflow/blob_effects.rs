use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use aos_air_types::HashRef;
use aos_effects::builtins::{
    BlobGetParams, BlobGetReceipt, BlobPutParams, LlmOutputEnvelope, LlmToolCallList,
};
use aos_wasm_sdk::PendingEffect;

use crate::contracts::{
    PendingBlobGet, PendingBlobGetKind, PendingBlobPut, PendingBlobPutKind, RunTraceEntryKind,
    SessionState, ToolCallObserved, ToolCallStatus,
};
use crate::helpers::{SessionEffectCommand, SessionReduceOutput};

use super::tool_batch::{fail_tool_call, set_tool_call_status};
use super::{
    RunToolBatch, SessionReduceError, TOOL_RESULT_BLOB_MAX_BYTES, continue_tool_batch,
    dispatch_queued_llm_turn, fail_run, push_run_trace, queue_llm_turn, run_tool_batch, trace_ref,
    transition_to_waiting_input_if_running,
};
use alloc::collections::BTreeMap;

pub(super) fn has_pending_tool_definition_puts(state: &SessionState) -> bool {
    state.pending_blob_puts.values().any(|shared| {
        shared
            .waiters
            .iter()
            .any(|pending| matches!(pending.kind, PendingBlobPutKind::ToolDefinition { .. }))
    })
}

fn has_pending_tool_result_blob_get(
    state: &SessionState,
    tool_batch_id: &crate::contracts::ToolBatchId,
    call_id: &str,
) -> bool {
    state.pending_blob_gets.values().any(|shared| {
        shared.waiters.iter().any(|pending| {
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
    let pending_entry = PendingBlobGet {
        kind,
        emitted_at_ns: state.updated_at,
    };
    let begin =
        match state
            .pending_blob_gets
            .begin(&params, state.updated_at, pending_entry.clone())
        {
            Ok(begin) => begin,
            Err(_) => state.pending_blob_gets.attach(
                PendingEffect::new("sys/blob.get@1", String::new(), state.updated_at),
                pending_entry,
            ),
        };
    let params_hash = begin.pending.params_hash.clone();
    if begin.should_emit {
        out.effects.push(SessionEffectCommand::BlobGet {
            params,
            pending: begin.pending.clone(),
        });
    }
    Ok(params_hash)
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
    let pending_entry = PendingBlobPut {
        kind,
        emitted_at_ns: state.updated_at,
    };
    let begin =
        match state
            .pending_blob_puts
            .begin(&params, state.updated_at, pending_entry.clone())
        {
            Ok(begin) => begin,
            Err(_) => state.pending_blob_puts.attach(
                PendingEffect::new("sys/blob.put@1", String::new(), state.updated_at),
                pending_entry,
            ),
        };
    let params_hash = begin.pending.params_hash.clone();
    if begin.should_emit {
        out.effects.push(SessionEffectCommand::BlobPut {
            params,
            pending: begin.pending.clone(),
        });
    }
    params_hash
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

pub(super) fn inject_blob_inline_text_into_output_json(
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
    state.last_output_ref = Some(receipt.blob_ref.as_str().into());
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
    let mut refs = Vec::new();
    let mut metadata = BTreeMap::new();
    metadata.insert("call_count".into(), observed.len().to_string());
    for call in &observed {
        refs.push(trace_ref("tool_call_id", call.call_id.clone()));
        if let Some(arguments_ref) = call.arguments_ref.as_ref() {
            refs.push(trace_ref("arguments_ref", arguments_ref.clone()));
        }
    }
    push_run_trace(
        state,
        RunTraceEntryKind::ToolCallsObserved,
        "llm tool calls observed",
        refs,
        metadata,
    );
    run_tool_batch(
        state,
        RunToolBatch {
            intent_id: envelope.intent_id.as_str(),
            params_hash: None,
            calls: &observed,
        },
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
            fail_tool_call(
                batch,
                &call_id,
                "tool_arguments_ref_decode_failed",
                "failed to decode blob.get receipt payload",
            );
        }
        continue_tool_batch(state, out)?;
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
                fail_tool_call(
                    batch,
                    &call_id,
                    "tool_arguments_not_json",
                    "tool arguments blob must contain JSON",
                );
            }
            continue_tool_batch(state, out)?;
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
    continue_tool_batch(state, out)?;
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
        set_tool_call_status(batch, &call_id, ToolCallStatus::Succeeded);
    }

    continue_tool_batch(state, out)?;
    Ok(true)
}

pub(super) fn handle_pending_blob_get_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(matched) = state.pending_blob_gets.settle(envelope) else {
        return Ok(false);
    };
    for pending in matched.waiters {
        match pending.kind {
            PendingBlobGetKind::LlmOutputEnvelope => {
                let Some(receipt) = matched.receipt.clone().ok() else {
                    fail_run(state)?;
                    return Ok(true);
                };
                on_llm_output_blob(state, receipt, out)?;
            }
            PendingBlobGetKind::LlmToolCalls => {
                let Some(receipt) = matched.receipt.clone().ok() else {
                    fail_run(state)?;
                    return Ok(true);
                };
                on_llm_tool_calls_blob(state, envelope, receipt, out)?;
            }
            PendingBlobGetKind::ToolCallArguments {
                tool_batch_id,
                call_id,
            } => {
                if matched.receipt.is_failed() {
                    if let Some(batch) = state.active_tool_batch.as_mut()
                        && batch.tool_batch_id == tool_batch_id
                    {
                        fail_tool_call(
                            batch,
                            &call_id,
                            "tool_arguments_ref_failed",
                            "blob.get for tool arguments failed",
                        );
                    }
                    continue_tool_batch(state, out)?;
                    continue;
                }
                on_tool_call_arguments_blob(
                    state,
                    tool_batch_id,
                    call_id,
                    matched.receipt.clone().ok(),
                    out,
                )?;
            }
            PendingBlobGetKind::ToolResultBlob {
                tool_batch_id,
                call_id,
                blob_ref,
            } => {
                on_tool_result_blob(
                    state,
                    tool_batch_id,
                    call_id,
                    blob_ref,
                    matched.receipt.clone().ok(),
                    out,
                )?;
            }
        }
    }
    Ok(true)
}

pub(super) fn handle_pending_blob_put_receipt(
    state: &mut SessionState,
    envelope: &aos_wasm_sdk::EffectReceiptEnvelope,
    out: &mut SessionReduceOutput,
) -> Result<bool, SessionReduceError> {
    let Some(matched) = state.pending_blob_puts.settle(envelope) else {
        return Ok(false);
    };
    for pending in matched.waiters {
        match pending.kind {
            PendingBlobPutKind::ToolDefinition { tool_id } => {
                let Some(receipt) = matched.receipt.clone().ok() else {
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
            }
            PendingBlobPutKind::FollowUpMessage { index } => {
                let Some(receipt) = matched.receipt.clone().ok() else {
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
                        state.transcript_message_refs = next_refs.clone();
                        state.pending_follow_up_turn = None;
                        queue_llm_turn(state, next_refs, out)?;
                    }
                }
            }
        }
    }
    Ok(true)
}
