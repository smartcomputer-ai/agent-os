use std::collections::{BTreeMap, BTreeSet};

use crate::chat::protocol::{
    ChatDelta, ChatEvent, ChatProgressStatus, ChatTurn, reasoning_effort_label,
};

#[derive(Debug, Default)]
pub(crate) struct PlainRenderer {
    seen_messages: BTreeSet<String>,
    run_status: BTreeMap<String, ChatProgressStatus>,
    tool_status: BTreeMap<String, ChatProgressStatus>,
    compaction_status: BTreeMap<String, ChatProgressStatus>,
}

impl PlainRenderer {
    pub(crate) fn render_events(&mut self, events: &[ChatEvent]) {
        for event in events {
            self.render_event(event);
        }
    }

    pub(crate) fn render_history(&mut self, turns: &[ChatTurn]) {
        for turn in turns {
            self.render_turn(turn);
        }
    }

    pub(crate) fn render_event(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::Connected(info) => {
                println!(
                    "connected world {} session {} model {} effort {}",
                    info.world_id,
                    info.session_id,
                    info.settings.model,
                    reasoning_effort_label(info.settings.reasoning_effort)
                );
            }
            ChatEvent::SessionSelected(summary) => {
                let lifecycle = summary
                    .lifecycle
                    .map(|value| format!("{value:?}").to_ascii_lowercase())
                    .unwrap_or_else(|| "new".into());
                println!("session {} {}", summary.session_id, lifecycle);
            }
            ChatEvent::HistoryReset { session_id } => {
                self.seen_messages.clear();
                self.run_status.clear();
                self.tool_status.clear();
                self.compaction_status.clear();
                println!("history reset {session_id}");
            }
            ChatEvent::TranscriptDelta(ChatDelta::ReplaceTurns { turns, .. }) => {
                for turn in turns {
                    self.render_turn(turn);
                }
            }
            ChatEvent::TranscriptDelta(ChatDelta::AppendMessage { message, .. }) => {
                if self.seen_messages.insert(message.id.clone()) {
                    println!("{}: {}", message.role, message.content);
                }
            }
            ChatEvent::RunChanged(run) => {
                if self.run_status.get(&run.id) != Some(&run.status) {
                    self.run_status.insert(run.id.clone(), run.status);
                    println!("run {} {}", run.run_seq, status_label(run.status));
                }
            }
            ChatEvent::ToolChainsChanged { chains, .. } => {
                for chain in chains {
                    for call in &chain.calls {
                        let key = format!("{}:{}", chain.id, call.id);
                        if self.tool_status.get(&key) != Some(&call.status) {
                            self.tool_status.insert(key, call.status);
                            let group = call
                                .group_index
                                .map(|value| format!(" group {value}"))
                                .unwrap_or_default();
                            println!(
                                "tool {}{} {}",
                                call.tool_name,
                                group,
                                status_label(call.status)
                            );
                        }
                    }
                }
            }
            ChatEvent::CompactionsChanged { compactions, .. } => {
                for compaction in compactions {
                    if self.compaction_status.get(&compaction.id) != Some(&compaction.status) {
                        self.compaction_status
                            .insert(compaction.id.clone(), compaction.status);
                        println!("compaction {}", status_label(compaction.status));
                    }
                }
            }
            ChatEvent::StatusChanged(status) => {
                println!("status {}", status.status);
            }
            ChatEvent::GapObserved {
                requested_from,
                retained_from,
            } => {
                println!("gap requested_from {requested_from} retained_from {retained_from}");
            }
            ChatEvent::Reconnecting { from, reason } => {
                println!("reconnecting from {from}: {reason}");
            }
            ChatEvent::Error(error) => {
                eprintln!("error: {}", error.message);
                if let Some(action) = &error.action {
                    eprintln!("action: {action}");
                }
            }
        }
    }

    fn render_turn(&mut self, turn: &ChatTurn) {
        if let Some(user) = &turn.user
            && self.seen_messages.insert(user.id.clone())
        {
            println!("{}: {}", user.role, user.content);
        }
        if let Some(run) = &turn.run
            && self.run_status.get(&run.id) != Some(&run.status)
        {
            self.run_status.insert(run.id.clone(), run.status);
            println!("run {} {}", run.run_seq, status_label(run.status));
        }
        for chain in &turn.tool_chains {
            for call in &chain.calls {
                let key = format!("{}:{}", chain.id, call.id);
                if self.tool_status.get(&key) != Some(&call.status) {
                    self.tool_status.insert(key, call.status);
                    println!("tool {} {}", call.tool_name, status_label(call.status));
                }
            }
        }
        if let Some(assistant) = &turn.assistant
            && self.seen_messages.insert(assistant.id.clone())
        {
            println!("assistant: {}", assistant.content);
        }
    }
}

fn status_label(status: ChatProgressStatus) -> &'static str {
    match status {
        ChatProgressStatus::Queued => "queued",
        ChatProgressStatus::Running => "running",
        ChatProgressStatus::Waiting => "waiting",
        ChatProgressStatus::Succeeded => "ok",
        ChatProgressStatus::Failed => "failed",
        ChatProgressStatus::Cancelled => "cancelled",
        ChatProgressStatus::Stale => "stale",
        ChatProgressStatus::Unknown => "unknown",
    }
}
