use aos_effects::ReceiptStatus;
use aos_kernel::journal::{
    CapDecisionOutcome, DomainEventRecord, IntentOriginRecord, JournalKind, JournalRecord,
    PlanEndStatus, PolicyDecisionOutcome,
};
use aos_kernel::{Kernel, StateReader};
use aos_store::Store;
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

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

pub fn plan_run_summary<S: Store + 'static>(kernel: &Kernel<S>) -> Result<Value, HostError> {
    let entries = kernel.dump_journal()?;
    let mut records = Vec::with_capacity(entries.len());
    for entry in entries {
        let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
            .map_err(|e| HostError::External(format!("decode journal record: {e}")))?;
        records.push((entry.seq, record));
    }
    Ok(summarize_plan_runs_from_records(records))
}

#[derive(Default)]
struct PlanRunSummary {
    plan_name: String,
    started: u64,
    started_as_child: u64,
    ended_ok: u64,
    ended_error: u64,
    invariant_violation: u64,
    timeout_path: u64,
    effect_intents: u64,
    receipt_ok: u64,
    receipt_error: u64,
    receipt_timeout: u64,
    policy_allow: u64,
    policy_deny: u64,
    cap_allow: u64,
    cap_deny: u64,
}

fn summarize_plan_runs_from_records<I>(records: I) -> Value
where
    I: IntoIterator<Item = (u64, JournalRecord)>,
{
    let mut plans: BTreeMap<u64, PlanRunSummary> = BTreeMap::new();
    let mut intent_plan: HashMap<[u8; 32], u64> = HashMap::new();
    let mut correlation_events = Vec::new();

    for (seq, record) in records {
        match record {
            JournalRecord::DomainEvent(domain) => {
                if let Some(key) = domain.key {
                    correlation_events.push(json!({
                        "seq": seq,
                        "schema": domain.schema,
                        "event_hash": domain.event_hash,
                        "key_b64": base64::prelude::BASE64_STANDARD.encode(key),
                    }));
                }
            }
            JournalRecord::PlanStarted(started) => {
                let summary = plans.entry(started.plan_id).or_default();
                summary.plan_name = started.plan_name;
                summary.started += 1;
                if started.parent_instance_id.is_some() {
                    summary.started_as_child += 1;
                }
            }
            JournalRecord::PlanEnded(ended) => {
                let summary = plans.entry(ended.plan_id).or_default();
                if summary.plan_name.is_empty() {
                    summary.plan_name = ended.plan_name;
                }
                match ended.status {
                    PlanEndStatus::Ok => summary.ended_ok += 1,
                    PlanEndStatus::Error => {
                        summary.ended_error += 1;
                        if let Some(code) = ended.error_code {
                            if code == "invariant_violation" {
                                summary.invariant_violation += 1;
                            }
                            if code.contains("timeout") {
                                summary.timeout_path += 1;
                            }
                        }
                    }
                }
            }
            JournalRecord::EffectIntent(intent) => {
                if let IntentOriginRecord::Plan { plan_id, name } = intent.origin {
                    intent_plan.insert(intent.intent_hash, plan_id);
                    let summary = plans.entry(plan_id).or_default();
                    if summary.plan_name.is_empty() {
                        summary.plan_name = name;
                    }
                    summary.effect_intents += 1;
                }
            }
            JournalRecord::EffectReceipt(receipt) => {
                let Some(plan_id) = intent_plan.get(&receipt.intent_hash).copied() else {
                    continue;
                };
                let summary = plans.entry(plan_id).or_default();
                match receipt.status {
                    ReceiptStatus::Ok => summary.receipt_ok += 1,
                    ReceiptStatus::Error => summary.receipt_error += 1,
                    ReceiptStatus::Timeout => {
                        summary.receipt_timeout += 1;
                        summary.timeout_path += 1;
                    }
                }
            }
            JournalRecord::PolicyDecision(decision) => {
                let Some(plan_id) = intent_plan.get(&decision.intent_hash).copied() else {
                    continue;
                };
                let summary = plans.entry(plan_id).or_default();
                match decision.decision {
                    PolicyDecisionOutcome::Allow => summary.policy_allow += 1,
                    PolicyDecisionOutcome::Deny => summary.policy_deny += 1,
                }
            }
            JournalRecord::CapDecision(decision) => {
                let Some(plan_id) = intent_plan.get(&decision.intent_hash).copied() else {
                    continue;
                };
                let summary = plans.entry(plan_id).or_default();
                match decision.decision {
                    CapDecisionOutcome::Allow => summary.cap_allow += 1,
                    CapDecisionOutcome::Deny => summary.cap_deny += 1,
                }
            }
            _ => {}
        }
    }

    let plans_json: Vec<Value> = plans
        .iter()
        .map(|(plan_id, s)| {
            json!({
                "plan_id": plan_id,
                "plan_name": s.plan_name,
                "runs": {
                    "started": s.started,
                    "started_as_child": s.started_as_child,
                    "ok": s.ended_ok,
                    "error": s.ended_error
                },
                "failure_signals": {
                    "policy_deny": s.policy_deny,
                    "invariant_violation": s.invariant_violation,
                    "timeout_path": s.timeout_path,
                    "adapter_error": s.receipt_error
                },
                "effects": {
                    "intents": s.effect_intents,
                    "receipts": {
                        "ok": s.receipt_ok,
                        "error": s.receipt_error,
                        "timeout": s.receipt_timeout
                    },
                    "policy_decisions": {
                        "allow": s.policy_allow,
                        "deny": s.policy_deny
                    },
                    "cap_decisions": {
                        "allow": s.cap_allow,
                        "deny": s.cap_deny
                    }
                }
            })
        })
        .collect();

    let mut totals = PlanRunSummary::default();
    for s in plans.values() {
        totals.started += s.started;
        totals.started_as_child += s.started_as_child;
        totals.ended_ok += s.ended_ok;
        totals.ended_error += s.ended_error;
        totals.invariant_violation += s.invariant_violation;
        totals.timeout_path += s.timeout_path;
        totals.effect_intents += s.effect_intents;
        totals.receipt_ok += s.receipt_ok;
        totals.receipt_error += s.receipt_error;
        totals.receipt_timeout += s.receipt_timeout;
        totals.policy_allow += s.policy_allow;
        totals.policy_deny += s.policy_deny;
        totals.cap_allow += s.cap_allow;
        totals.cap_deny += s.cap_deny;
    }

    json!({
        "plans": plans_json,
        "totals": {
            "runs": {
                "started": totals.started,
                "started_as_child": totals.started_as_child,
                "ok": totals.ended_ok,
                "error": totals.ended_error
            },
            "failure_signals": {
                "policy_deny": totals.policy_deny,
                "invariant_violation": totals.invariant_violation,
                "timeout_path": totals.timeout_path,
                "adapter_error": totals.receipt_error
            },
            "effects": {
                "intents": totals.effect_intents,
                "receipts": {
                    "ok": totals.receipt_ok,
                    "error": totals.receipt_error,
                    "timeout": totals.receipt_timeout
                },
                "policy_decisions": {
                    "allow": totals.policy_allow,
                    "deny": totals.policy_deny
                },
                "cap_decisions": {
                    "allow": totals.cap_allow,
                    "deny": totals.cap_deny
                }
            }
        },
        "correlation_events": correlation_events,
    })
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
        JournalKind::PlanStarted => "plan_started",
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

#[cfg(test)]
mod tests {
    use super::summarize_plan_runs_from_records;
    use aos_effects::ReceiptStatus;
    use aos_kernel::journal::{
        CapDecisionOutcome, CapDecisionRecord, CapDecisionStage, DomainEventRecord,
        EffectIntentRecord, EffectReceiptRecord, IntentOriginRecord, JournalRecord, PlanEndStatus,
        PlanEndedRecord, PlanStartedRecord, PolicyDecisionOutcome, PolicyDecisionRecord,
    };

    fn find_plan<'a>(summary: &'a serde_json::Value, plan_id: u64) -> &'a serde_json::Value {
        summary
            .get("plans")
            .and_then(|v| v.as_array())
            .and_then(|plans| {
                plans
                    .iter()
                    .find(|p| p.get("plan_id").and_then(|v| v.as_u64()) == Some(plan_id))
            })
            .expect("plan summary present")
    }

    #[test]
    fn plan_summary_aggregates_lifecycle_and_effect_signals() {
        let h1 = [1u8; 32];
        let records = vec![
            (
                1,
                JournalRecord::DomainEvent(DomainEventRecord {
                    schema: "com.acme/Start@1".into(),
                    value: vec![],
                    key: Some(b"corr-1".to_vec()),
                    now_ns: 0,
                    logical_now_ns: 0,
                    journal_height: 0,
                    entropy: vec![],
                    event_hash: "sha256:abc".into(),
                    manifest_hash: String::new(),
                }),
            ),
            (
                2,
                JournalRecord::PlanStarted(PlanStartedRecord {
                    plan_name: "com.acme/Parent@1".into(),
                    plan_id: 10,
                    input_hash: "sha256:in".into(),
                    parent_instance_id: None,
                }),
            ),
            (
                3,
                JournalRecord::PlanStarted(PlanStartedRecord {
                    plan_name: "com.acme/Child@1".into(),
                    plan_id: 11,
                    input_hash: "sha256:in-child".into(),
                    parent_instance_id: Some(10),
                }),
            ),
            (
                4,
                JournalRecord::EffectIntent(EffectIntentRecord {
                    intent_hash: h1,
                    kind: "http.request".into(),
                    cap_name: "cap_http".into(),
                    params_cbor: vec![],
                    idempotency_key: [0u8; 32],
                    origin: IntentOriginRecord::Plan {
                        name: "com.acme/Parent@1".into(),
                        plan_id: 10,
                    },
                }),
            ),
            (
                5,
                JournalRecord::PolicyDecision(PolicyDecisionRecord {
                    intent_hash: h1,
                    policy_name: "default".into(),
                    rule_index: Some(0),
                    decision: PolicyDecisionOutcome::Deny,
                }),
            ),
            (
                6,
                JournalRecord::CapDecision(CapDecisionRecord {
                    intent_hash: h1,
                    stage: CapDecisionStage::Enqueue,
                    effect_kind: "http.request".into(),
                    cap_name: "cap_http".into(),
                    cap_type: "sys/http.out@1".into(),
                    grant_hash: [2u8; 32],
                    enforcer_module: "sys/CapAllowAll@1".into(),
                    decision: CapDecisionOutcome::Allow,
                    deny: None,
                    expiry_ns: None,
                    logical_now_ns: 0,
                }),
            ),
            (
                7,
                JournalRecord::EffectReceipt(EffectReceiptRecord {
                    intent_hash: h1,
                    adapter_id: "adapter.http".into(),
                    status: ReceiptStatus::Timeout,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                    now_ns: 0,
                    logical_now_ns: 0,
                    journal_height: 0,
                    entropy: vec![],
                    manifest_hash: String::new(),
                }),
            ),
            (
                8,
                JournalRecord::PlanEnded(PlanEndedRecord {
                    plan_name: "com.acme/Parent@1".into(),
                    plan_id: 10,
                    status: PlanEndStatus::Error,
                    error_code: Some("invariant_violation".into()),
                }),
            ),
            (
                9,
                JournalRecord::PlanEnded(PlanEndedRecord {
                    plan_name: "com.acme/Child@1".into(),
                    plan_id: 11,
                    status: PlanEndStatus::Ok,
                    error_code: None,
                }),
            ),
        ];

        let summary = summarize_plan_runs_from_records(records);
        let parent = find_plan(&summary, 10);
        assert_eq!(parent["runs"]["started"], 1);
        assert_eq!(parent["runs"]["error"], 1);
        assert_eq!(parent["effects"]["intents"], 1);
        assert_eq!(parent["effects"]["receipts"]["timeout"], 1);
        assert_eq!(parent["effects"]["policy_decisions"]["deny"], 1);
        assert_eq!(parent["effects"]["cap_decisions"]["allow"], 1);
        assert_eq!(parent["failure_signals"]["invariant_violation"], 1);
        assert_eq!(parent["failure_signals"]["timeout_path"], 1);

        let child = find_plan(&summary, 11);
        assert_eq!(child["runs"]["started_as_child"], 1);
        assert_eq!(child["runs"]["ok"], 1);

        assert_eq!(summary["totals"]["runs"]["started"], 2);
        assert_eq!(summary["totals"]["runs"]["ok"], 1);
        assert_eq!(summary["totals"]["runs"]["error"], 1);
        assert_eq!(summary["totals"]["failure_signals"]["policy_deny"], 1);

        let correlation_events = summary
            .get("correlation_events")
            .and_then(|v| v.as_array())
            .expect("correlation events");
        assert_eq!(correlation_events.len(), 1);
        assert_eq!(correlation_events[0]["schema"], "com.acme/Start@1");
    }

    #[test]
    fn plan_summary_ignores_non_plan_effect_origins() {
        let records = vec![
            (
                1,
                JournalRecord::EffectIntent(EffectIntentRecord {
                    intent_hash: [9u8; 32],
                    kind: "timer.set".into(),
                    cap_name: "timer_cap".into(),
                    params_cbor: vec![],
                    idempotency_key: [0u8; 32],
                    origin: IntentOriginRecord::Reducer {
                        name: "com.acme/Reducer@1".into(),
                    },
                }),
            ),
            (
                2,
                JournalRecord::EffectReceipt(EffectReceiptRecord {
                    intent_hash: [9u8; 32],
                    adapter_id: "adapter.timer".into(),
                    status: ReceiptStatus::Ok,
                    payload_cbor: vec![],
                    cost_cents: None,
                    signature: vec![],
                    now_ns: 0,
                    logical_now_ns: 0,
                    journal_height: 0,
                    entropy: vec![],
                    manifest_hash: String::new(),
                }),
            ),
        ];

        let summary = summarize_plan_runs_from_records(records);
        assert_eq!(summary["plans"], serde_json::json!([]));
        assert_eq!(summary["totals"]["effects"]["intents"], 0);
        assert_eq!(summary["totals"]["effects"]["receipts"]["ok"], 0);
    }
}
