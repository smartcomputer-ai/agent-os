use aos_effects::ReceiptStatus;
use aos_kernel::journal::{
    CapDecisionOutcome, DomainEventRecord, JournalKind, JournalRecord, PolicyDecisionOutcome,
};
use aos_kernel::{Kernel, StateReader};
use aos_store::Store;
use base64::Engine as _;
use serde_json::{Value, json};

use crate::error::HostError;

#[derive(Debug, Clone, Default)]
pub struct TraceQuery {
    pub event_hash: Option<String>,
    pub schema: Option<String>,
    pub correlate_by: Option<String>,
    pub correlate_value: Option<Value>,
    pub window_limit: Option<u64>,
}

pub fn trace_get<S: Store + 'static>(
    kernel: &Kernel<S>,
    query: TraceQuery,
) -> Result<Value, HostError> {
    let TraceQuery {
        event_hash,
        schema,
        correlate_by,
        correlate_value,
        window_limit,
    } = query;

    let entries = kernel.dump_journal()?;
    let mut root_seq: Option<u64> = None;
    let mut root_domain: Option<DomainEventRecord> = None;

    if let Some(hash) = event_hash.clone() {
        for entry in &entries {
            if entry.kind != JournalKind::DomainEvent {
                continue;
            }
            let Ok(record) = serde_cbor::from_slice::<JournalRecord>(&entry.payload) else {
                continue;
            };
            let JournalRecord::DomainEvent(domain) = record else {
                continue;
            };
            if domain.event_hash == hash {
                root_seq = Some(entry.seq);
                root_domain = Some(domain);
                break;
            }
        }
    } else if let (Some(schema), Some(correlate_by), Some(correlate_value)) = (
        schema.clone(),
        correlate_by.clone(),
        correlate_value.clone(),
    ) {
        for entry in entries.iter().rev() {
            if entry.kind != JournalKind::DomainEvent {
                continue;
            }
            let Ok(record) = serde_cbor::from_slice::<JournalRecord>(&entry.payload) else {
                continue;
            };
            let JournalRecord::DomainEvent(domain) = record else {
                continue;
            };
            if domain.schema != schema {
                continue;
            }
            let Ok(value_json) = serde_cbor::from_slice::<Value>(&domain.value) else {
                continue;
            };
            let Some(found) = json_path_get(&value_json, &correlate_by) else {
                continue;
            };
            if found == &correlate_value {
                root_seq = Some(entry.seq);
                root_domain = Some(domain);
                break;
            }
        }
    }

    let root_domain = root_domain.ok_or_else(|| {
        if let Some(hash) = event_hash.clone() {
            HostError::External(format!("trace root event_hash '{}' not found", hash))
        } else {
            HostError::External("trace root event not found for correlation query".into())
        }
    })?;
    let root_seq =
        root_seq.ok_or_else(|| HostError::External("trace root sequence missing".into()))?;
    let root_record_json = serde_json::to_value(&root_domain)
        .map_err(|e| HostError::External(format!("encode root event record: {e}")))?;

    let limit = window_limit.unwrap_or(400) as usize;
    let mut window = Vec::new();
    let mut has_receipt_error = false;
    let mut has_workflow_error = false;
    for entry in entries
        .into_iter()
        .filter(|entry| entry.seq >= root_seq)
        .take(limit)
    {
        let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
            .map_err(|e| HostError::External(format!("decode journal record: {e}")))?;
        if let JournalRecord::EffectReceipt(receipt) = &record {
            if !matches!(receipt.status, ReceiptStatus::Ok) {
                has_receipt_error = true;
            }
        }
        if let JournalRecord::Custom(custom) = &record {
            if custom.tag == "workflow_error" {
                has_workflow_error = true;
            }
        }
        window.push(crate::control::JournalTailEntry {
            kind: journal_kind_name(entry.kind).to_string(),
            seq: entry.seq,
            record: serde_json::to_value(record)
                .map_err(|e| HostError::External(format!("encode journal record: {e}")))?,
        });
    }

    let workflow_instances = kernel.workflow_instances_snapshot();
    let pending_reducer_receipts = kernel.pending_reducer_receipts_snapshot();
    let queued_effects = kernel.queued_effects_snapshot();
    let journal_head = kernel.journal_head();

    let inflight_workflow_intents = workflow_instances
        .iter()
        .map(|instance| instance.inflight_intents.len())
        .sum::<usize>();
    let non_terminal_workflow_instances = workflow_instances
        .iter()
        .filter(|instance| {
            !matches!(
                instance.status,
                aos_kernel::snapshot::WorkflowStatusSnapshot::Completed
                    | aos_kernel::snapshot::WorkflowStatusSnapshot::Failed
            )
        })
        .count();
    let pending_reducer_receipts_count = pending_reducer_receipts.len();
    let queued_effects_count = queued_effects.len();
    let waiting_receipt_count =
        inflight_workflow_intents + pending_reducer_receipts_count + queued_effects_count;
    let waiting_event_count = non_terminal_workflow_instances;

    let terminal_state = if has_receipt_error || has_workflow_error {
        "failed"
    } else if waiting_receipt_count > 0 {
        "waiting_receipt"
    } else if waiting_event_count > 0 {
        "waiting_event"
    } else if window.is_empty() {
        "unknown"
    } else {
        "completed"
    };

    let meta = kernel.get_journal_head();
    let root_value_json = serde_cbor::from_slice::<Value>(&root_domain.value).ok();

    Ok(json!({
        "query": {
            "event_hash": event_hash,
            "schema": schema,
            "correlate_by": correlate_by,
            "value": correlate_value,
            "window_limit": window_limit.unwrap_or(400),
        },
        "root": {
            "schema": root_domain.schema,
            "event_hash": root_domain.event_hash,
            "seq": root_seq,
            "key_b64": root_domain.key.as_ref().map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
            "value": root_value_json,
        },
        "root_event": {
            "seq": root_seq,
            "record": root_record_json,
        },
        "journal_window": {
            "from_seq": root_seq,
            "to_seq": window.last().map(|e| e.seq).unwrap_or(root_seq),
            "entries": window,
        },
        "live_wait": {
            "workflow_instances": workflow_instances.into_iter().map(|instance| {
                json!({
                    "instance_id": instance.instance_id,
                    "status": match instance.status {
                        aos_kernel::snapshot::WorkflowStatusSnapshot::Running => "running",
                        aos_kernel::snapshot::WorkflowStatusSnapshot::Waiting => "waiting",
                        aos_kernel::snapshot::WorkflowStatusSnapshot::Completed => "completed",
                        aos_kernel::snapshot::WorkflowStatusSnapshot::Failed => "failed",
                    },
                    "last_processed_event_seq": instance.last_processed_event_seq,
                    "module_version": instance.module_version,
                    "inflight_intents": instance.inflight_intents.into_iter().map(|intent| {
                        json!({
                            "intent_hash": hash_bytes_hex(&intent.intent_id),
                            "effect_kind": intent.effect_kind,
                            "origin_module_id": intent.origin_module_id,
                            "origin_instance_key_b64": intent.origin_instance_key.as_ref().map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
                            "emitted_at_seq": intent.emitted_at_seq,
                            "last_stream_seq": intent.last_stream_seq,
                            "age_events": journal_head.saturating_sub(intent.emitted_at_seq),
                        })
                    }).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "pending_workflow_receipts": kernel.workflow_instances_snapshot().into_iter().flat_map(|instance| {
                let instance_id = instance.instance_id.clone();
                instance.inflight_intents.into_iter().map(move |intent| {
                    json!({
                        "instance_id": instance_id,
                        "intent_hash": hash_bytes_hex(&intent.intent_id),
                        "origin_module_id": intent.origin_module_id,
                        "origin_instance_key_b64": intent.origin_instance_key.as_ref().map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
                        "effect_kind": intent.effect_kind,
                        "emitted_at_seq": intent.emitted_at_seq,
                        "last_stream_seq": intent.last_stream_seq,
                        "age_events": journal_head.saturating_sub(intent.emitted_at_seq),
                    })
                })
            }).collect::<Vec<_>>(),
            "pending_reducer_receipts": pending_reducer_receipts.into_iter().map(|pending| {
                json!({
                    "intent_hash": hash_bytes_hex(&pending.intent_hash),
                    "origin_module_id": pending.origin_module_id,
                    "effect_kind": pending.effect_kind,
                    "origin_instance_key_b64": pending.origin_instance_key.as_ref().map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
                    "emitted_at_seq": pending.emitted_at_seq,
                })
            }).collect::<Vec<_>>(),
            "queued_effects": queued_effects.into_iter().map(|queued| {
                json!({
                    "intent_hash": hash_bytes_hex(&queued.intent_hash),
                    "kind": queued.kind,
                    "cap_name": queued.cap_name,
                })
            }).collect::<Vec<_>>(),
            "strict_quiescence": {
                "non_terminal_workflow_instances": non_terminal_workflow_instances,
                "inflight_workflow_intents": inflight_workflow_intents,
                "pending_reducer_receipts": pending_reducer_receipts_count,
                "queued_effects": queued_effects_count,
                "open_streaming_intents": kernel.workflow_instances_snapshot().into_iter().flat_map(|instance| {
                    let instance_id = instance.instance_id.clone();
                    instance.inflight_intents.into_iter().map(move |intent| {
                        json!({
                            "instance_id": instance_id,
                            "intent_hash": hash_bytes_hex(&intent.intent_id),
                            "effect_kind": intent.effect_kind,
                            "emitted_at_seq": intent.emitted_at_seq,
                            "last_stream_seq": intent.last_stream_seq,
                            "age_events": journal_head.saturating_sub(intent.emitted_at_seq),
                        })
                    })
                }).collect::<Vec<_>>(),
            },
        },
        "terminal_state": terminal_state,
        "meta": {
            "journal_height": meta.journal_height,
            "manifest_hash": meta.manifest_hash.to_hex(),
            "snapshot_hash": meta.snapshot_hash.map(|h: aos_cbor::Hash| h.to_hex()),
        },
    }))
}

pub fn workflow_trace_summary<S: Store + 'static>(kernel: &Kernel<S>) -> Result<Value, HostError> {
    let mut effect_intents = 0u64;
    let mut receipt_ok = 0u64;
    let mut receipt_error = 0u64;
    let mut receipt_timeout = 0u64;
    let mut stream_frames = 0u64;
    let mut policy_allow = 0u64;
    let mut policy_deny = 0u64;
    let mut cap_allow = 0u64;
    let mut cap_deny = 0u64;
    let mut proposed = 0u64;
    let mut shadowed = 0u64;
    let mut approved = 0u64;
    let mut applied = 0u64;

    for entry in kernel.dump_journal()? {
        let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
            .map_err(|e| HostError::External(format!("decode journal record: {e}")))?;
        match record {
            JournalRecord::EffectIntent(_) => effect_intents += 1,
            JournalRecord::EffectReceipt(receipt) => match receipt.status {
                ReceiptStatus::Ok => receipt_ok += 1,
                ReceiptStatus::Error => receipt_error += 1,
                ReceiptStatus::Timeout => receipt_timeout += 1,
            },
            JournalRecord::StreamFrame(_) => stream_frames += 1,
            JournalRecord::PolicyDecision(decision) => match decision.decision {
                PolicyDecisionOutcome::Allow => policy_allow += 1,
                PolicyDecisionOutcome::Deny => policy_deny += 1,
            },
            JournalRecord::CapDecision(decision) => match decision.decision {
                CapDecisionOutcome::Allow => cap_allow += 1,
                CapDecisionOutcome::Deny => cap_deny += 1,
            },
            JournalRecord::Governance(governance) => match governance {
                aos_kernel::journal::GovernanceRecord::Proposed(_) => proposed += 1,
                aos_kernel::journal::GovernanceRecord::ShadowReport(_) => shadowed += 1,
                aos_kernel::journal::GovernanceRecord::Approved(_) => approved += 1,
                aos_kernel::journal::GovernanceRecord::Applied(_) => applied += 1,
            },
            _ => {}
        }
    }

    let workflow_instances = kernel.workflow_instances_snapshot();
    let pending_reducer_receipts = kernel.pending_reducer_receipts_snapshot();
    let queued_effects = kernel.queued_effects_snapshot();

    let mut running = 0u64;
    let mut waiting = 0u64;
    let mut completed = 0u64;
    let mut failed = 0u64;
    let mut inflight_total = 0u64;
    let mut continuations = Vec::new();

    for instance in &workflow_instances {
        match instance.status {
            aos_kernel::snapshot::WorkflowStatusSnapshot::Running => running += 1,
            aos_kernel::snapshot::WorkflowStatusSnapshot::Waiting => waiting += 1,
            aos_kernel::snapshot::WorkflowStatusSnapshot::Completed => completed += 1,
            aos_kernel::snapshot::WorkflowStatusSnapshot::Failed => failed += 1,
        }
        inflight_total += instance.inflight_intents.len() as u64;
        for intent in &instance.inflight_intents {
            continuations.push(json!({
                "instance_id": instance.instance_id,
                "intent_hash": hash_bytes_hex(&intent.intent_id),
                "origin_module_id": intent.origin_module_id,
                "origin_instance_key_b64": intent.origin_instance_key.as_ref().map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
                "effect_kind": intent.effect_kind,
                "emitted_at_seq": intent.emitted_at_seq,
                "last_stream_seq": intent.last_stream_seq,
            }));
        }
    }

    Ok(json!({
        "totals": {
            "effects": {
                "intents": effect_intents,
                "receipts": {
                    "ok": receipt_ok,
                    "error": receipt_error,
                    "timeout": receipt_timeout,
                },
                "stream_frames": stream_frames,
            },
            "policy_decisions": {
                "allow": policy_allow,
                "deny": policy_deny,
            },
            "cap_decisions": {
                "allow": cap_allow,
                "deny": cap_deny,
            },
            "workflows": {
                "total": workflow_instances.len(),
                "running": running,
                "waiting": waiting,
                "completed": completed,
                "failed": failed,
                "inflight_intents": inflight_total,
            },
            "governance": {
                "proposed": proposed,
                "shadowed": shadowed,
                "approved": approved,
                "applied": applied,
            }
        },
        "runtime_wait": {
            "pending_reducer_receipts": pending_reducer_receipts.len(),
            "queued_effects": queued_effects.len(),
        },
        "strict_quiescence": {
            "non_terminal_workflow_instances": (running + waiting),
            "inflight_workflow_intents": inflight_total,
            "pending_reducer_receipts": pending_reducer_receipts.len(),
            "queued_effects": queued_effects.len(),
        },
        "continuations": continuations,
    }))
}

pub fn diagnose_trace(trace: &Value) -> Value {
    let terminal = trace
        .get("terminal_state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let entries = trace
        .get("journal_window")
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let live_wait = trace
        .get("live_wait")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let pending_workflow_receipts = live_wait
        .get("pending_workflow_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let pending_reducer_receipts = live_wait
        .get("pending_reducer_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let workflow_instances = live_wait
        .get("workflow_instances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let waiting_workflow_instances = workflow_instances
        .iter()
        .filter(|instance| {
            instance
                .get("status")
                .and_then(|v| v.as_str())
                .map(|status| matches!(status, "running" | "waiting"))
                .unwrap_or(false)
        })
        .count();
    let pending_workflow_intents = workflow_instances
        .iter()
        .map(|instance| {
            instance
                .get("inflight_intents")
                .and_then(|v| v.as_array())
                .map(std::vec::Vec::len)
                .unwrap_or(0)
        })
        .sum::<usize>();
    let queued_effects = live_wait
        .get("queued_effects")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);

    let mut policy_denied = false;
    let mut capability_denied = false;
    let mut adapter_error = false;
    let mut adapter_timeout = false;
    let mut workflow_failed = false;
    let mut invariant_violation = false;

    for entry in entries {
        let kind = entry
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let record = entry.get("record").cloned().unwrap_or(Value::Null);

        match kind {
            "policy_decision" => {
                let decision = find_str(&record, "decision").unwrap_or_default();
                if decision.eq_ignore_ascii_case("deny") {
                    policy_denied = true;
                }
            }
            "cap_decision" => {
                let decision = find_str(&record, "decision").unwrap_or_default();
                if decision.eq_ignore_ascii_case("deny") {
                    capability_denied = true;
                }
            }
            "effect_receipt" => {
                let status = find_str(&record, "status").unwrap_or_default();
                if status.eq_ignore_ascii_case("error") {
                    adapter_error = true;
                }
                if status.eq_ignore_ascii_case("timeout") {
                    adapter_timeout = true;
                }
            }
            "legacy_plan_ended" => {
                let status = find_str(&record, "status").unwrap_or_default();
                if status.eq_ignore_ascii_case("error") {
                    let error_code = find_str(&record, "error_code").unwrap_or_default();
                    if error_code.eq_ignore_ascii_case("invariant_violation") {
                        invariant_violation = true;
                    } else {
                        workflow_failed = true;
                    }
                }
            }
            _ => {}
        }
    }

    if workflow_instances.iter().any(|instance| {
        instance
            .get("status")
            .and_then(|v| v.as_str())
            .map(|status| status == "failed")
            .unwrap_or(false)
    }) {
        workflow_failed = true;
    }

    let cause = if policy_denied {
        "policy_denied"
    } else if capability_denied {
        "capability_denied"
    } else if adapter_timeout {
        "adapter_timeout"
    } else if adapter_error {
        "adapter_error"
    } else if invariant_violation {
        "invariant_violation"
    } else if workflow_failed {
        "unknown_failure"
    } else if terminal == "waiting_receipt" {
        "waiting_receipt"
    } else if terminal == "waiting_event" {
        "waiting_event"
    } else if terminal == "completed" {
        "completed"
    } else {
        "unknown"
    };

    let hint = match cause {
        "policy_denied" => "Adjust policy rules or module-origin/cap mapping.",
        "capability_denied" => "Inspect capability grant constraints and enforcer output.",
        "adapter_timeout" => "Check adapter timeout and upstream endpoint latency.",
        "adapter_error" => "Inspect adapter receipt payload for failure details.",
        "invariant_violation" => "Inspect module state invariants and step transitions.",
        "unknown_failure" => "Inspect runtime records to identify the failure source.",
        "waiting_receipt" => {
            "Flow is waiting for workflow in-flight receipts or queued effect execution."
        }
        "waiting_event" => "Flow has non-terminal workflow instances pending follow-up events.",
        "completed" => "Flow completed successfully.",
        _ => "Insufficient signal; inspect full trace timeline.",
    };

    let event_hash = trace
        .get("root")
        .and_then(|v| v.get("event_hash"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            trace
                .get("query")
                .and_then(|v| v.get("event_hash"))
                .and_then(|v| v.as_str())
        });

    let intent_hash = first_intent_hash(trace.get("live_wait").unwrap_or(&Value::Null));

    json!({
        "terminal_state": terminal,
        "cause": cause,
        "hint": hint,
        "hashes": {
            "event_hash": event_hash,
            "intent_hash": intent_hash,
        },
        "waits": {
            "pending_workflow_receipts": pending_workflow_receipts,
            "waiting_workflow_instances": waiting_workflow_instances,
            "pending_workflow_intents": pending_workflow_intents,
            "pending_reducer_receipts": pending_reducer_receipts,
            "queued_effects": queued_effects,
        }
    })
}

fn first_intent_hash(value: &Value) -> Option<String> {
    let read = |arr: Option<&Vec<Value>>| -> Option<String> {
        let arr = arr?;
        for item in arr {
            if let Some(hash) = item.get("intent_hash").and_then(|v| v.as_str()) {
                return Some(hash.to_string());
            }
            if let Some(hashes) = item.get("intent_hashes").and_then(|v| v.as_array()) {
                if let Some(hash) = hashes.first().and_then(|v| v.as_str()) {
                    return Some(hash.to_string());
                }
            }
        }
        None
    };

    let workflow_hash = {
        value
            .get("workflow_instances")
            .and_then(|v| v.as_array())
            .and_then(|instances| {
                for instance in instances {
                    let Some(inflight) =
                        instance.get("inflight_intents").and_then(|v| v.as_array())
                    else {
                        continue;
                    };
                    if let Some(hash) = inflight
                        .iter()
                        .find_map(|intent| intent.get("intent_hash").and_then(|v| v.as_str()))
                    {
                        return Some(hash.to_string());
                    }
                }
                None
            })
    };

    workflow_hash
        .or_else(|| {
            read(
                value
                    .get("pending_workflow_receipts")
                    .and_then(|v| v.as_array()),
            )
        })
        .or_else(|| {
            read(
                value
                    .get("pending_reducer_receipts")
                    .and_then(|v| v.as_array()),
            )
        })
        .or_else(|| read(value.get("queued_effects").and_then(|v| v.as_array())))
}

fn find_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(key).and_then(Value::as_str) {
                return Some(v);
            }
            for v in map.values() {
                if let Some(found) = find_str(v, key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_str(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn journal_kind_name(kind: JournalKind) -> &'static str {
    match kind {
        JournalKind::DomainEvent => "domain_event",
        JournalKind::EffectIntent => "effect_intent",
        JournalKind::EffectReceipt => "effect_receipt",
        JournalKind::StreamFrame => "stream_frame",
        JournalKind::CapDecision => "cap_decision",
        JournalKind::Manifest => "manifest",
        JournalKind::Snapshot => "snapshot",
        JournalKind::PolicyDecision => "policy_decision",
        JournalKind::Governance => "governance",
        JournalKind::PlanStarted => "legacy_plan_started",
        JournalKind::PlanResult => "legacy_plan_result",
        JournalKind::PlanEnded => "legacy_plan_ended",
        JournalKind::Custom => "custom",
    }
}

fn hash_bytes_hex(hash: &[u8; 32]) -> String {
    aos_cbor::Hash::from_bytes(hash)
        .map(|h| h.to_hex())
        .unwrap_or_else(|_| hex::encode(hash))
}

fn json_path_get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let normalized = path.trim();
    let normalized = if let Some(rest) = normalized.strip_prefix("$.") {
        rest
    } else {
        normalized
    };
    if normalized.is_empty() {
        return Some(value);
    }

    let mut current = value;
    for segment in normalized.split('.') {
        if segment.is_empty() {
            continue;
        }
        let obj = current.as_object()?;
        current = obj.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::diagnose_trace;

    #[test]
    fn diagnose_trace_prefers_policy_denied() {
        let trace = serde_json::json!({
            "terminal_state": "failed",
            "journal_window": {
                "entries": [
                    { "kind": "policy_decision", "record": { "decision": "deny" } }
                ]
            },
            "live_wait": {}
        });
        let diagnosis = diagnose_trace(&trace);
        assert_eq!(diagnosis["cause"], "policy_denied");
    }
}
