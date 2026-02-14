use aos_effects::ReceiptStatus;
use aos_kernel::journal::{DomainEventRecord, JournalKind, JournalRecord, PlanEndStatus};
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
    let mut has_plan_error = false;
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
        if let JournalRecord::PlanEnded(ended) = &record {
            if matches!(ended.status, PlanEndStatus::Error) {
                has_plan_error = true;
            }
        }
        window.push(crate::control::JournalTailEntry {
            kind: journal_kind_name(entry.kind).to_string(),
            seq: entry.seq,
            record: serde_json::to_value(record)
                .map_err(|e| HostError::External(format!("encode journal record: {e}")))?,
        });
    }

    let pending_plan_receipts = kernel.pending_plan_receipts();
    let plan_wait_receipts = kernel.debug_plan_waits();
    let plan_wait_events = kernel.debug_plan_waiting_events();
    let pending_reducer_receipts = kernel.pending_reducer_receipts_snapshot();
    let queued_effects = kernel.queued_effects_snapshot();

    let waiting_receipt_count = pending_plan_receipts.len()
        + pending_reducer_receipts.len()
        + queued_effects.len()
        + plan_wait_receipts
            .iter()
            .map(|(_, waits)| waits.len())
            .sum::<usize>();
    let waiting_event_count = plan_wait_events.len();

    let terminal_state = if has_receipt_error || has_plan_error {
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
            "pending_plan_receipts": pending_plan_receipts.into_iter().map(|(plan_id, intent_hash)| {
                json!({
                    "plan_id": plan_id,
                    "plan_name": kernel.plan_name_for_instance(plan_id),
                    "intent_hash": hash_bytes_hex(&intent_hash),
                })
            }).collect::<Vec<_>>(),
            "plan_waiting_receipts": plan_wait_receipts.into_iter().map(|(plan_id, waits)| {
                json!({
                    "plan_id": plan_id,
                    "plan_name": kernel.plan_name_for_instance(plan_id),
                    "intent_hashes": waits.into_iter().map(|h| hash_bytes_hex(&h)).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "plan_waiting_events": plan_wait_events.into_iter().map(|(plan_id, schema)| {
                json!({
                    "plan_id": plan_id,
                    "plan_name": kernel.plan_name_for_instance(plan_id),
                    "event_schema": schema,
                })
            }).collect::<Vec<_>>(),
            "pending_reducer_receipts": pending_reducer_receipts.into_iter().map(|pending| {
                json!({
                    "intent_hash": hash_bytes_hex(&pending.intent_hash),
                    "reducer": pending.reducer,
                    "effect_kind": pending.effect_kind,
                })
            }).collect::<Vec<_>>(),
            "queued_effects": queued_effects.into_iter().map(|queued| {
                json!({
                    "intent_hash": hash_bytes_hex(&queued.intent_hash),
                    "kind": queued.kind,
                    "cap_name": queued.cap_name,
                })
            }).collect::<Vec<_>>(),
        },
        "terminal_state": terminal_state,
        "meta": {
            "journal_height": meta.journal_height,
            "manifest_hash": meta.manifest_hash.to_hex(),
            "snapshot_hash": meta.snapshot_hash.map(|h: aos_cbor::Hash| h.to_hex()),
        },
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

    let pending_plan_receipts = live_wait
        .get("pending_plan_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let plan_waiting_receipts = live_wait
        .get("plan_waiting_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let plan_waiting_events = live_wait
        .get("plan_waiting_events")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let pending_reducer_receipts = live_wait
        .get("pending_reducer_receipts")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);
    let queued_effects = live_wait
        .get("queued_effects")
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::len)
        .unwrap_or(0);

    let mut policy_denied = false;
    let mut capability_denied = false;
    let mut adapter_error = false;
    let mut adapter_timeout = false;
    let mut plan_error = false;
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
            "plan_ended" => {
                let status = find_str(&record, "status").unwrap_or_default();
                if status.eq_ignore_ascii_case("error") {
                    let error_code = find_str(&record, "error_code").unwrap_or_default();
                    if error_code.eq_ignore_ascii_case("invariant_violation") {
                        invariant_violation = true;
                    } else {
                        plan_error = true;
                    }
                }
            }
            _ => {}
        }
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
    } else if plan_error {
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
        "policy_denied" => "Adjust policy rules or plan origin/cap mapping.",
        "capability_denied" => "Inspect capability grant constraints and enforcer output.",
        "adapter_timeout" => "Check adapter timeout and upstream endpoint latency.",
        "adapter_error" => "Inspect adapter receipt payload for failure details.",
        "invariant_violation" => "Inspect plan invariants and local/step value assumptions.",
        "unknown_failure" => "Inspect plan/runtime records to identify the failure source.",
        "waiting_receipt" => "Flow is waiting for effect receipts or queued effect execution.",
        "waiting_event" => "Flow is waiting for a follow-up domain event.",
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
            "pending_plan_receipts": pending_plan_receipts,
            "plan_waiting_receipts": plan_waiting_receipts,
            "plan_waiting_events": plan_waiting_events,
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

    read(
        value
            .get("pending_plan_receipts")
            .and_then(|v| v.as_array()),
    )
    .or_else(|| {
        read(
            value
                .get("pending_reducer_receipts")
                .and_then(|v| v.as_array()),
        )
    })
    .or_else(|| {
        read(
            value
                .get("plan_waiting_receipts")
                .and_then(|v| v.as_array()),
        )
    })
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
        JournalKind::CapDecision => "cap_decision",
        JournalKind::Manifest => "manifest",
        JournalKind::Snapshot => "snapshot",
        JournalKind::PolicyDecision => "policy_decision",
        JournalKind::Governance => "governance",
        JournalKind::PlanResult => "plan_result",
        JournalKind::PlanEnded => "plan_ended",
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
