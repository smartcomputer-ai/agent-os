use std::sync::Arc;

use aos_effects::ReceiptStatus;
use aos_fdb::{HostedRuntimeStore, UniverseId, WorldId, WorldRuntimeInfo};
use aos_kernel::journal::{
    CapDecisionOutcome, DomainEventRecord, GovernanceRecord, JournalKind, JournalRecord,
    OwnedJournalEntry, PlanEndStatus, PolicyDecisionOutcome,
};
use aos_node::control::ControlError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default)]
pub struct TraceQuery {
    pub event_hash: Option<String>,
    pub schema: Option<String>,
    pub correlate_by: Option<String>,
    pub correlate_value: Option<Value>,
    pub window_limit: Option<u64>,
}

pub fn build_trace<P>(
    persistence: &Arc<P>,
    universe: UniverseId,
    world: WorldId,
    query: TraceQuery,
) -> Result<Value, ControlError>
where
    P: HostedRuntimeStore + 'static,
{
    match (
        query.event_hash.is_some(),
        query.schema.is_some(),
        query.correlate_by.is_some(),
        query.correlate_value.is_some(),
    ) {
        (true, false, false, false) => {}
        (false, true, true, true) => {}
        (false, false, false, false) => {
            return Err(ControlError::invalid(
                "trace requires either event_hash or schema+correlate_by+value",
            ));
        }
        _ => {
            return Err(ControlError::invalid(
                "trace requires exactly one mode: event_hash or schema+correlate_by+value",
            ));
        }
    }

    let entries = load_all_journal_entries(persistence, universe, world)?;
    let mut root_seq = None;
    let mut root_domain = None;

    if let Some(event_hash) = query.event_hash.as_deref() {
        for entry in &entries {
            let Some(domain) = decode_domain_event(entry)? else {
                continue;
            };
            if domain.event_hash == event_hash {
                root_seq = Some(entry.seq);
                root_domain = Some(domain);
                break;
            }
        }
    } else if let (Some(schema), Some(correlate_by), Some(correlate_value)) = (
        query.schema.as_deref(),
        query.correlate_by.as_deref(),
        query.correlate_value.as_ref(),
    ) {
        for entry in entries.iter().rev() {
            let Some(domain) = decode_domain_event(entry)? else {
                continue;
            };
            if domain.schema != schema {
                continue;
            }
            let Ok(value_json) = serde_cbor::from_slice::<Value>(&domain.value) else {
                continue;
            };
            let Some(found) = json_path_get(&value_json, correlate_by) else {
                continue;
            };
            if found == correlate_value {
                root_seq = Some(entry.seq);
                root_domain = Some(domain);
                break;
            }
        }
    }

    let root_domain = root_domain.ok_or_else(|| match query.event_hash.as_deref() {
        Some(hash) => ControlError::not_found(format!("trace root event_hash '{hash}'")),
        None => ControlError::not_found("trace root event for correlation query"),
    })?;
    let root_seq =
        root_seq.ok_or_else(|| ControlError::not_found("trace root sequence missing"))?;

    let limit = query.window_limit.unwrap_or(400) as usize;
    let mut window = Vec::new();
    let mut has_receipt_error = false;
    let mut has_workflow_error = false;
    for entry in entries
        .into_iter()
        .filter(|entry| entry.seq >= root_seq)
        .take(limit)
    {
        let (record_json, decoded) = journal_record_value(&entry)?;
        if let Some(record) = decoded {
            match record {
                JournalRecord::EffectReceipt(receipt)
                    if !matches!(receipt.status, ReceiptStatus::Ok) =>
                {
                    has_receipt_error = true;
                }
                JournalRecord::PlanEnded(ended) if matches!(ended.status, PlanEndStatus::Error) => {
                    has_workflow_error = true;
                }
                JournalRecord::Custom(custom) if custom.tag == "workflow_error" => {
                    has_workflow_error = true;
                }
                _ => {}
            }
        }
        window.push(json!({
            "kind": journal_kind_name(entry.kind),
            "seq": entry.seq,
            "record": record_json,
        }));
    }

    let runtime =
        persistence.world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())?;
    let active_baseline = persistence.snapshot_active_baseline(universe, world)?;
    let terminal_state =
        terminal_state_from_runtime(&runtime, has_receipt_error || has_workflow_error);
    let root_record_json = serde_json::to_value(&root_domain)?;
    let root_value_json = serde_cbor::from_slice::<Value>(&root_domain.value).ok();

    Ok(json!({
        "query": {
            "event_hash": query.event_hash,
            "schema": query.schema,
            "correlate_by": query.correlate_by,
            "value": query.correlate_value,
            "window_limit": query.window_limit.unwrap_or(400),
        },
        "root": {
            "schema": root_domain.schema,
            "event_hash": root_domain.event_hash,
            "seq": root_seq,
            "key_b64": root_domain.key.as_ref().map(|key| BASE64_STANDARD.encode(key)),
            "value": root_value_json,
        },
        "root_event": {
            "seq": root_seq,
            "record": root_record_json,
        },
        "journal_window": {
            "from_seq": root_seq,
            "to_seq": window
                .last()
                .and_then(|entry| entry.get("seq"))
                .and_then(Value::as_u64)
                .unwrap_or(root_seq),
            "entries": window,
        },
        "runtime": runtime,
        "active_baseline": active_baseline,
        "runtime_wait": {
            "has_pending_inbox": runtime.has_pending_inbox,
            "has_pending_effects": runtime.has_pending_effects,
            "next_timer_due_at_ns": runtime.next_timer_due_at_ns,
            "has_pending_maintenance": runtime.has_pending_maintenance,
            "lease": runtime.lease,
        },
        "terminal_state": terminal_state,
        "coarse": true,
    }))
}

pub fn build_trace_summary<P>(
    persistence: &Arc<P>,
    universe: UniverseId,
    world: WorldId,
    recent_limit: u32,
) -> Result<Value, ControlError>
where
    P: HostedRuntimeStore + 'static,
{
    let entries = load_all_journal_entries(persistence, universe, world)?;
    let runtime =
        persistence.world_runtime_info(universe, world, aos_runtime::now_wallclock_ns())?;
    let active_baseline = persistence.snapshot_active_baseline(universe, world)?;
    let head = persistence.head_projection(universe, world)?;
    let journal_head = persistence.journal_head(universe, world)?;

    let mut domain_events = 0u64;
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

    for entry in &entries {
        let Some(record) = decode_journal_record(entry)? else {
            continue;
        };
        match record {
            JournalRecord::DomainEvent(_) => domain_events += 1,
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
                GovernanceRecord::Proposed(_) => proposed += 1,
                GovernanceRecord::ShadowReport(_) => shadowed += 1,
                GovernanceRecord::Approved(_) => approved += 1,
                GovernanceRecord::Applied(_) => applied += 1,
            },
            _ => {}
        }
    }

    let recent_limit = recent_limit.max(1) as usize;
    let mut recent = entries
        .iter()
        .rev()
        .take(recent_limit)
        .map(journal_entry_summary)
        .collect::<Result<Vec<_>, _>>()?;
    recent.reverse();
    let has_runtime_wait = runtime.has_pending_inbox
        || runtime.has_pending_effects
        || runtime.next_timer_due_at_ns.is_some();

    Ok(json!({
        "world_id": world,
        "journal_head": journal_head,
        "manifest_hash": head.as_ref().map(|record| record.manifest_hash.clone()),
        "active_baseline": active_baseline,
        "runtime": runtime,
        "totals": {
            "journal": {
                "entries": entries.len(),
                "domain_events": domain_events,
            },
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
            "governance": {
                "proposed": proposed,
                "shadowed": shadowed,
                "approved": approved,
                "applied": applied,
            },
            "workflows": {
                "waiting": u64::from(has_runtime_wait),
            },
        },
        "runtime_wait": {
            "has_pending_inbox": runtime.has_pending_inbox,
            "has_pending_effects": runtime.has_pending_effects,
            "next_timer_due_at_ns": runtime.next_timer_due_at_ns,
            "has_pending_maintenance": runtime.has_pending_maintenance,
        },
        "strict_quiescence": {
            "has_runtime_wait": has_runtime_wait,
            "has_pending_inbox": runtime.has_pending_inbox,
            "has_pending_effects": runtime.has_pending_effects,
            "next_timer_due_at_ns": runtime.next_timer_due_at_ns,
        },
        "recent_journal": recent,
        "coarse": true,
    }))
}

fn load_all_journal_entries<P>(
    persistence: &Arc<P>,
    universe: UniverseId,
    world: WorldId,
) -> Result<Vec<OwnedJournalEntry>, ControlError>
where
    P: HostedRuntimeStore + 'static,
{
    const PAGE: u32 = 512;

    let mut from = persistence
        .snapshot_active_baseline(universe, world)?
        .height
        .saturating_add(1);
    let mut entries = Vec::new();
    loop {
        let rows = persistence.journal_read_range(universe, world, from, PAGE)?;
        if rows.is_empty() {
            break;
        }
        let mut last_seq = from;
        for (_seq, raw) in rows {
            let entry: OwnedJournalEntry = serde_cbor::from_slice(&raw)?;
            last_seq = entry.seq;
            entries.push(entry);
        }
        from = last_seq.saturating_add(1);
    }
    Ok(entries)
}

fn decode_journal_record(entry: &OwnedJournalEntry) -> Result<Option<JournalRecord>, ControlError> {
    Ok(match entry.kind {
        JournalKind::DomainEvent
        | JournalKind::EffectIntent
        | JournalKind::EffectReceipt
        | JournalKind::StreamFrame
        | JournalKind::CapDecision
        | JournalKind::Manifest
        | JournalKind::Snapshot
        | JournalKind::PolicyDecision
        | JournalKind::Governance
        | JournalKind::PlanStarted
        | JournalKind::PlanResult
        | JournalKind::PlanEnded
        | JournalKind::Custom => Some(serde_cbor::from_slice(&entry.payload)?),
    })
}

fn decode_domain_event(
    entry: &OwnedJournalEntry,
) -> Result<Option<DomainEventRecord>, ControlError> {
    let Some(record) = decode_journal_record(entry)? else {
        return Ok(None);
    };
    match record {
        JournalRecord::DomainEvent(domain) => Ok(Some(domain)),
        _ => Ok(None),
    }
}

fn journal_record_value(
    entry: &OwnedJournalEntry,
) -> Result<(Value, Option<JournalRecord>), ControlError> {
    let decoded = decode_journal_record(entry)?;
    let value = if let Some(record) = decoded.as_ref() {
        serde_json::to_value(record)?
    } else {
        json!({ "payload_b64": BASE64_STANDARD.encode(&entry.payload) })
    };
    Ok((value, decoded))
}

fn journal_entry_summary(entry: &OwnedJournalEntry) -> Result<Value, ControlError> {
    let (record, _) = journal_record_value(entry)?;
    Ok(json!({
        "seq": entry.seq,
        "kind": journal_kind_name(entry.kind),
        "record": record,
    }))
}

fn terminal_state_from_runtime(runtime: &WorldRuntimeInfo, failed: bool) -> &'static str {
    if failed {
        "failed"
    } else if runtime.has_pending_effects || runtime.next_timer_due_at_ns.is_some() {
        "waiting_receipt"
    } else if runtime.has_pending_inbox {
        "waiting_event"
    } else {
        "completed"
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
        let object = current.as_object()?;
        current = object.get(segment)?;
    }
    Some(current)
}
