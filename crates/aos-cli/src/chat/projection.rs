use std::collections::BTreeMap;

use aos_agent::{
    ActiveToolBatch, RunRecord, RunState, RunTraceEntryKind, SessionState, ToolCallStatus,
};

use crate::chat::blob_cache::BlobCache;
use crate::chat::client::run_id_label;
use crate::chat::protocol::{
    ChatCompactionView, ChatDelta, ChatEvent, ChatMessageView, ChatProgressStatus, ChatRunView,
    ChatToolCallView, ChatToolChainView, ChatTurn, run_status,
};

#[derive(Debug, Clone)]
pub(crate) struct ChatProjection {
    #[allow(dead_code)]
    pub world_id: String,
    pub session_id: String,
    pub journal_next_from: u64,
    pub session_state: Option<SessionState>,
    pub turns: Vec<ChatTurn>,
    pub active_run: Option<ChatRunView>,
    pub tool_chains: Vec<ChatToolChainView>,
    pub compactions: Vec<ChatCompactionView>,
}

impl ChatProjection {
    pub(crate) fn new(world_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            world_id: world_id.into(),
            session_id: session_id.into(),
            journal_next_from: 0,
            session_state: None,
            turns: Vec::new(),
            active_run: None,
            tool_chains: Vec::new(),
            compactions: Vec::new(),
        }
    }

    pub(crate) fn reset(&mut self, session_id: impl Into<String>) -> ChatEvent {
        self.session_id = session_id.into();
        self.session_state = None;
        self.turns.clear();
        self.active_run = None;
        self.tool_chains.clear();
        self.compactions.clear();
        ChatEvent::HistoryReset {
            session_id: self.session_id.clone(),
        }
    }

    pub(crate) fn apply_state(
        &mut self,
        journal_head: u64,
        state: Option<SessionState>,
        blob_cache: &BlobCache,
    ) -> Vec<ChatEvent> {
        self.journal_next_from = journal_head.saturating_add(1);
        self.session_state = state.clone();

        let old_turns = self.turns.clone();
        let old_run = self.active_run.clone();
        let old_tools = self.tool_chains.clone();
        let old_compactions = self.compactions.clone();

        if let Some(state) = state.as_ref() {
            self.turns = project_turns(state, blob_cache);
            self.active_run = project_active_run(state);
            self.tool_chains = project_tool_chains(state, blob_cache);
            self.compactions = project_compactions(state);
        } else {
            self.turns.clear();
            self.active_run = None;
            self.tool_chains.clear();
            self.compactions.clear();
        }

        let mut events = Vec::new();
        if old_turns != self.turns {
            events.push(ChatEvent::TranscriptDelta(ChatDelta::ReplaceTurns {
                session_id: self.session_id.clone(),
                turns: self.turns.clone(),
            }));
        }
        if old_run != self.active_run
            && let Some(run) = self.active_run.clone()
        {
            events.push(ChatEvent::RunChanged(run));
        }
        if old_tools != self.tool_chains {
            events.push(ChatEvent::ToolChainsChanged {
                session_id: self.session_id.clone(),
                chains: self.tool_chains.clone(),
            });
        }
        if old_compactions != self.compactions {
            events.push(ChatEvent::CompactionsChanged {
                session_id: self.session_id.clone(),
                compactions: self.compactions.clone(),
            });
        }
        events
    }
}

fn project_turns(state: &SessionState, blob_cache: &BlobCache) -> Vec<ChatTurn> {
    let mut turns = Vec::new();
    for record in &state.run_history {
        let run = project_run_record(record);
        turns.push(ChatTurn {
            turn_id: run.id.clone(),
            user: first_user_message(&record.input_refs, blob_cache),
            assistant: record
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.output_ref.as_deref())
                .and_then(|ref_| assistant_message(ref_, blob_cache)),
            run: Some(run),
            tool_chains: Vec::new(),
        });
    }
    if let Some(current) = &state.current_run {
        let run = project_run_state(current);
        turns.push(ChatTurn {
            turn_id: run.id.clone(),
            user: first_user_message(&current.input_refs, blob_cache),
            assistant: current
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.output_ref.as_deref())
                .or(current.last_output_ref.as_deref())
                .and_then(|ref_| assistant_message(ref_, blob_cache)),
            run: Some(run),
            // Active tool chains are live viewport UI, not committed transcript history.
            // They are projected separately via ToolChainsChanged and finalized by the TUI.
            tool_chains: Vec::new(),
        });
    }
    turns
}

fn first_user_message(input_refs: &[String], blob_cache: &BlobCache) -> Option<ChatMessageView> {
    let ref_ = input_refs.first()?;
    let message = blob_cache.user_message(ref_)?;
    Some(ChatMessageView {
        id: ref_.clone(),
        role: message.role,
        content: message.content,
        ref_: Some(ref_.clone()),
    })
}

fn assistant_message(ref_: &str, blob_cache: &BlobCache) -> Option<ChatMessageView> {
    let text = blob_cache.assistant_text(ref_)?;
    Some(ChatMessageView {
        id: ref_.to_string(),
        role: "assistant".into(),
        content: text,
        ref_: Some(ref_.to_string()),
    })
}

fn project_active_run(state: &SessionState) -> Option<ChatRunView> {
    state.current_run.as_ref().map(project_run_state)
}

fn project_run_record(record: &RunRecord) -> ChatRunView {
    ChatRunView {
        id: run_id_label(&record.run_id),
        run_seq: record.run_id.run_seq,
        lifecycle: record.lifecycle,
        status: run_status(record.lifecycle),
        provider: String::new(),
        model: String::new(),
        reasoning_effort: None,
        input_refs: record.input_refs.clone(),
        output_ref: record
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.output_ref.clone()),
        started_at_ns: record.started_at,
        updated_at_ns: record.ended_at,
    }
}

fn project_run_state(run: &RunState) -> ChatRunView {
    ChatRunView {
        id: run_id_label(&run.run_id),
        run_seq: run.run_id.run_seq,
        lifecycle: run.lifecycle,
        status: run_status(run.lifecycle),
        provider: run.config.provider.clone(),
        model: run.config.model.clone(),
        reasoning_effort: run.config.reasoning_effort,
        input_refs: run.input_refs.clone(),
        output_ref: run
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.output_ref.clone())
            .or_else(|| run.last_output_ref.clone()),
        started_at_ns: run.started_at,
        updated_at_ns: run.updated_at,
    }
}

fn project_tool_chains(state: &SessionState, blob_cache: &BlobCache) -> Vec<ChatToolChainView> {
    state
        .current_run
        .as_ref()
        .and_then(|run| run.active_tool_batch.as_ref())
        .or(state.active_tool_batch.as_ref())
        .map(|batch| vec![project_tool_batch(batch, blob_cache)])
        .unwrap_or_default()
}

fn project_tool_batch(batch: &ActiveToolBatch, blob_cache: &BlobCache) -> ChatToolChainView {
    let group_by_call = batch
        .plan
        .execution_plan
        .groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| {
            group
                .iter()
                .map(move |call_id| (call_id.clone(), index as u64 + 1))
        })
        .collect::<BTreeMap<_, _>>();
    let planned_by_call = batch
        .plan
        .planned_calls
        .iter()
        .map(|call| (call.call_id.clone(), call))
        .collect::<BTreeMap<_, _>>();

    let mut calls = Vec::new();
    for observed in &batch.plan.observed_calls {
        let planned = planned_by_call.get(&observed.call_id).copied();
        let status = batch
            .call_status
            .get(&observed.call_id)
            .map(tool_call_status)
            .unwrap_or(ChatProgressStatus::Queued);
        let error = batch
            .call_status
            .get(&observed.call_id)
            .and_then(tool_call_error);
        let arguments_preview = observed
            .arguments_ref
            .as_deref()
            .and_then(|ref_| {
                blob_cache
                    .preview_json_or_text(ref_)
                    .or_else(|| blob_cache.error(ref_).map(str::to_owned))
            })
            .or_else(|| {
                (!observed.arguments_json.is_empty()).then(|| preview(&observed.arguments_json))
            });
        let result_preview = batch
            .llm_results
            .get(&observed.call_id)
            .map(|result| preview(&result.output_json));
        calls.push(ChatToolCallView {
            id: observed.call_id.clone(),
            tool_id: planned.map(|call| call.tool_id.clone()),
            tool_name: planned
                .map(|call| call.tool_name.clone())
                .unwrap_or_else(|| observed.tool_name.clone()),
            status,
            group_index: group_by_call.get(&observed.call_id).copied(),
            parallel_safe: planned.map(|call| call.parallel_safe),
            resource_key: planned.and_then(|call| call.resource_key.clone()),
            arguments_preview,
            result_preview,
            error,
        });
    }

    let status = aggregate_tool_status(&calls);
    ChatToolChainView {
        id: format!(
            "{}:{}",
            run_id_label(&batch.tool_batch_id.run_id),
            batch.tool_batch_id.batch_seq
        ),
        title: format!("tools {} calls", calls.len()),
        status,
        calls,
        summary: Some(format!(
            "{} execution groups",
            batch.plan.execution_plan.groups.len()
        )),
    }
}

fn tool_call_status(status: &ToolCallStatus) -> ChatProgressStatus {
    match status {
        ToolCallStatus::Queued => ChatProgressStatus::Queued,
        ToolCallStatus::Pending => ChatProgressStatus::Running,
        ToolCallStatus::Succeeded => ChatProgressStatus::Succeeded,
        ToolCallStatus::Failed { .. } => ChatProgressStatus::Failed,
        ToolCallStatus::Ignored => ChatProgressStatus::Stale,
        ToolCallStatus::Cancelled => ChatProgressStatus::Cancelled,
    }
}

fn tool_call_error(status: &ToolCallStatus) -> Option<String> {
    match status {
        ToolCallStatus::Failed { code, detail } => Some(format!("{code}: {detail}")),
        _ => None,
    }
}

fn aggregate_tool_status(calls: &[ChatToolCallView]) -> ChatProgressStatus {
    if calls.is_empty() {
        return ChatProgressStatus::Unknown;
    }
    if calls
        .iter()
        .any(|call| matches!(call.status, ChatProgressStatus::Failed))
    {
        return ChatProgressStatus::Failed;
    }
    if calls
        .iter()
        .any(|call| matches!(call.status, ChatProgressStatus::Running))
    {
        return ChatProgressStatus::Running;
    }
    if calls.iter().any(|call| {
        matches!(
            call.status,
            ChatProgressStatus::Queued | ChatProgressStatus::Waiting
        )
    }) {
        return ChatProgressStatus::Queued;
    }
    if calls.iter().all(|call| {
        matches!(
            call.status,
            ChatProgressStatus::Succeeded | ChatProgressStatus::Stale
        )
    }) {
        return ChatProgressStatus::Succeeded;
    }
    ChatProgressStatus::Unknown
}

fn project_compactions(state: &SessionState) -> Vec<ChatCompactionView> {
    let Some(run) = &state.current_run else {
        return Vec::new();
    };
    let mut views = Vec::new();
    for entry in &run.trace.entries {
        let status = match entry.kind {
            RunTraceEntryKind::ContextPressureObserved => ChatProgressStatus::Waiting,
            RunTraceEntryKind::CompactionRequested => ChatProgressStatus::Running,
            RunTraceEntryKind::CompactionReceived | RunTraceEntryKind::ActiveWindowUpdated => {
                ChatProgressStatus::Succeeded
            }
            RunTraceEntryKind::TokenCountRequested => ChatProgressStatus::Running,
            RunTraceEntryKind::TokenCountReceived => ChatProgressStatus::Succeeded,
            _ => continue,
        };
        let artifact_ref = entry
            .refs
            .iter()
            .find_map(|ref_| ref_.ref_.clone())
            .or_else(|| entry.metadata.get("artifact_ref").cloned());
        views.push(ChatCompactionView {
            id: format!("{}:compaction:{}", run_id_label(&run.run_id), entry.seq),
            status,
            reason: (!entry.summary.is_empty()).then(|| entry.summary.clone()),
            before_tokens: metadata_u64(&entry.metadata, "before_tokens")
                .or_else(|| metadata_u64(&entry.metadata, "input_tokens")),
            after_tokens: metadata_u64(&entry.metadata, "after_tokens"),
            artifact_ref,
        });
    }
    views
}

fn metadata_u64(metadata: &BTreeMap<String, String>, key: &str) -> Option<u64> {
    metadata
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
}

fn preview(value: &str) -> String {
    let mut out = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > 180 {
        out.truncate(179);
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_agent::{
        ActiveToolBatch, RunId, ToolBatchId, ToolBatchPlan, ToolCallObserved, ToolCallStatus,
        ToolExecutionPlan,
    };

    #[test]
    fn projects_parallel_tool_batch_groups_and_statuses() {
        let run_id = RunId {
            session_id: aos_agent::SessionId("018f2a66-31cc-7b25-a4f7-37e3310fdc6a".into()),
            run_seq: 2,
        };
        let batch = ActiveToolBatch {
            tool_batch_id: ToolBatchId {
                run_id,
                batch_seq: 1,
            },
            plan: ToolBatchPlan {
                observed_calls: vec![
                    ToolCallObserved {
                        call_id: "a".into(),
                        tool_name: "grep".into(),
                        ..ToolCallObserved::default()
                    },
                    ToolCallObserved {
                        call_id: "b".into(),
                        tool_name: "read".into(),
                        ..ToolCallObserved::default()
                    },
                    ToolCallObserved {
                        call_id: "c".into(),
                        tool_name: "edit".into(),
                        ..ToolCallObserved::default()
                    },
                ],
                execution_plan: ToolExecutionPlan {
                    groups: vec![vec!["a".into(), "b".into()], vec!["c".into()]],
                },
                ..ToolBatchPlan::default()
            },
            call_status: BTreeMap::from([
                ("a".into(), ToolCallStatus::Succeeded),
                ("b".into(), ToolCallStatus::Pending),
                ("c".into(), ToolCallStatus::Queued),
            ]),
            ..ActiveToolBatch::default()
        };
        let view = project_tool_batch(&batch, &BlobCache::default());
        assert_eq!(view.status, ChatProgressStatus::Running);
        assert_eq!(view.calls[0].group_index, Some(1));
        assert_eq!(view.calls[1].group_index, Some(1));
        assert_eq!(view.calls[2].group_index, Some(2));
        assert_eq!(view.calls[1].status, ChatProgressStatus::Running);
    }
}
