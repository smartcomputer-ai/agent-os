use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::chat::protocol::ChatToolChainView;
use crate::chat::protocol::{ChatDelta, ChatEvent, ChatProgressStatus};
use crate::chat::tui::cell::{
    CellRenderState, ChatCell, ChatCellKind, ErrorCell, MessageCell, NoticeCell, RunCell,
    ToolChainCell,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TranscriptOptions {
    pub(crate) show_tool_details: bool,
}

#[derive(Debug)]
pub(crate) struct TranscriptState {
    options: TranscriptOptions,
    cells: Vec<Box<dyn ChatCell>>,
    pending_history_cell_indices: Vec<usize>,
    emitted_history_cells: Vec<(String, String)>,
    active_cell: Option<Box<dyn ChatCell>>,
    active_cell_revision: u64,
    active_tool_chains: Option<Vec<ChatToolChainView>>,
    pending_user_messages: Vec<PendingUserMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingUserMessage {
    id: String,
    content: String,
}

impl TranscriptState {
    pub(crate) fn new(options: TranscriptOptions) -> Self {
        Self {
            options,
            cells: Vec::new(),
            pending_history_cell_indices: Vec::new(),
            emitted_history_cells: Vec::new(),
            active_cell: None,
            active_cell_revision: 0,
            active_tool_chains: None,
            pending_user_messages: Vec::new(),
        }
    }

    pub(crate) fn apply_chat_event(&mut self, event: ChatEvent) {
        match event {
            ChatEvent::Connected(info) => {
                self.replace_or_push_committed(Box::new(NoticeCell::new(
                    "connected",
                    format!(
                        "connected world {} session {}",
                        short(&info.world_id),
                        short(&info.session_id)
                    ),
                )));
            }
            ChatEvent::SessionsListed { .. } | ChatEvent::SessionSelected(_) => {}
            ChatEvent::HistoryReset { session_id } => {
                self.cells.clear();
                self.pending_history_cell_indices.clear();
                self.emitted_history_cells.clear();
                self.pending_user_messages.clear();
                self.active_cell = None;
                self.active_tool_chains = None;
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.push_committed_cell(Box::new(NoticeCell::new(
                    "history-reset",
                    format!("switched to session {}", short(&session_id)),
                )));
            }
            ChatEvent::TranscriptDelta(delta) => self.apply_delta(delta),
            ChatEvent::RunChanged(run) => {
                self.active_cell = match run.status {
                    ChatProgressStatus::Queued | ChatProgressStatus::Running => {
                        Some(Box::new(RunCell::new(
                            format!("active-run:{}", run.id),
                            format!("run {} {:?} {}", run.run_seq, run.lifecycle, run.model),
                        )))
                    }
                    _ => None,
                };
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            }
            ChatEvent::ToolChainsChanged { chains, .. } => {
                if chains.is_empty() {
                    self.flush_terminal_active_tool_chains(true);
                    self.active_tool_chains = None;
                    if self
                        .active_cell
                        .as_ref()
                        .is_some_and(|cell| cell.kind() == ChatCellKind::ToolChain)
                    {
                        self.active_cell = None;
                        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                    }
                } else {
                    let id = chains
                        .first()
                        .map(|chain| format!("active-tools:{}", chain.id))
                        .unwrap_or_else(|| "active-tools".to_string());
                    if self.active_tool_chains.as_ref().is_some_and(|previous| {
                        tool_chain_identity(previous) != tool_chain_identity(&chains)
                    }) {
                        self.flush_terminal_active_tool_chains(false);
                    }
                    self.active_tool_chains = Some(chains.clone());
                    self.active_cell = Some(Box::new(ToolChainCell::new(id, chains)));
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                }
            }
            ChatEvent::CompactionsChanged { compactions, .. } => {
                if let Some(compaction) = compactions.last() {
                    self.replace_or_push_committed(Box::new(NoticeCell::new(
                        format!("compaction:{}", compaction.id),
                        format!("context compaction {:?}", compaction.status),
                    )));
                }
            }
            ChatEvent::StatusChanged(_) => {}
            ChatEvent::GapObserved {
                requested_from,
                retained_from,
            } => {
                self.replace_or_push_committed(Box::new(NoticeCell::new(
                    "journal-gap",
                    format!(
                        "journal gap  requested #{requested_from}, retained from #{retained_from}; refreshed snapshot"
                    ),
                )));
            }
            ChatEvent::Reconnecting { from, reason } => {
                self.replace_or_push_committed(Box::new(NoticeCell::new(
                    "journal-reconnecting",
                    format!("journal reconnecting from #{from}: {reason}"),
                )));
            }
            ChatEvent::Error(error) => {
                self.replace_or_push_committed(Box::new(ErrorCell::new(
                    format!("error:{}", self.cells.len()),
                    error.message,
                )));
            }
        }
    }

    pub(crate) fn drain_pending_history_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let render_state = CellRenderState;
        let mut lines = Vec::new();
        let pending_indices = std::mem::take(&mut self.pending_history_cell_indices);
        for index in pending_indices {
            let Some(cell) = self.cells.get(index) else {
                continue;
            };
            let fingerprint = (cell.id().to_string(), cell_fingerprint(cell.as_ref()));
            if self.is_emitted_history_cell(&fingerprint) {
                continue;
            }
            let cell_lines = cell.display_lines(width, &render_state);
            if cell_lines.is_empty() {
                continue;
            }
            lines.extend(cell_lines);
            self.emitted_history_cells.push(fingerprint);
        }
        lines
    }

    pub(crate) fn reflow_history_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let render_state = CellRenderState;
        let mut lines = Vec::new();
        self.pending_history_cell_indices.clear();
        self.emitted_history_cells.clear();
        for cell in &self.cells {
            let cell_lines = cell.display_lines(width, &render_state);
            if cell_lines.is_empty() {
                continue;
            }
            self.emitted_history_cells
                .push((cell.id().to_string(), cell_fingerprint(cell.as_ref())));
            lines.extend(cell_lines);
        }
        lines
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();
        let render_state = CellRenderState;
        for pending in &self.pending_user_messages {
            let cell = MessageCell::new(&pending.id, "user_pending", &pending.content);
            lines.extend(cell.display_lines(area.width, &render_state));
            lines.push(Line::default());
        }
        if let Some(active_cell) = self.active_cell.as_ref() {
            let _ = (
                active_cell.kind(),
                active_cell.desired_height(area.width, &render_state),
                active_cell.is_stream_continuation(),
            );
            lines.extend(active_cell.display_lines(area.width, &render_state));
        }
        let visible = visible_tail(lines, area.height);
        Paragraph::new(visible)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    pub(crate) fn desired_height(&self, width: u16) -> u16 {
        let render_state = CellRenderState;
        let mut height = 0u16;
        for pending in &self.pending_user_messages {
            let cell = MessageCell::new(&pending.id, "user_pending", &pending.content);
            height = height.saturating_add(cell.desired_height(width, &render_state));
            height = height.saturating_add(1);
        }
        if let Some(active_cell) = self.active_cell.as_ref() {
            height = height.saturating_add(active_cell.desired_height(width, &render_state));
        }
        height
    }

    fn apply_delta(&mut self, delta: ChatDelta) {
        match delta {
            ChatDelta::ReplaceTurns { turns, .. } => {
                self.cells.clear();
                self.pending_history_cell_indices.clear();
                self.pending_user_messages.clear();
                self.active_cell = None;
                self.active_tool_chains = None;
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                for turn in turns {
                    let turn_id = turn.turn_id.clone();
                    if let Some(user) = turn.user {
                        self.push_committed_cell_if_changed(Box::new(MessageCell::new(
                            format!("{turn_id}:user:{}", user.id),
                            user.role,
                            user.content,
                        )));
                    }
                    if !turn.tool_chains.is_empty() {
                        self.push_committed_cell_if_changed(Box::new(
                            self.completed_tool_cell(format!("{turn_id}:tools"), turn.tool_chains),
                        ));
                        self.push_committed_cell_if_changed(Box::new(NoticeCell::blank(format!(
                            "{turn_id}:tools-spacer"
                        ))));
                    }
                    if let Some(assistant) = turn.assistant {
                        self.push_committed_cell_if_changed(Box::new(MessageCell::new(
                            format!("{turn_id}:assistant:{}", assistant.id),
                            assistant.role,
                            assistant.content,
                        )));
                    }
                    let _ = turn.run;
                }
            }
            ChatDelta::AppendMessage { message, .. } => {
                if message.role == "user_pending" {
                    self.pending_user_messages.push(PendingUserMessage {
                        id: message.id.clone(),
                        content: message.content.clone(),
                    });
                    return;
                } else if message.role == "user"
                    && let Some(index) = self
                        .pending_user_messages
                        .iter()
                        .position(|pending| pending.content == message.content)
                {
                    self.pending_user_messages.remove(index);
                }
                self.push_committed_cell(Box::new(MessageCell::new(
                    message.id,
                    message.role,
                    message.content,
                )));
            }
        }
    }

    fn push_committed_cell(&mut self, cell: Box<dyn ChatCell>) {
        self.cells.push(cell);
        self.pending_history_cell_indices
            .push(self.cells.len().saturating_sub(1));
    }

    fn push_committed_cell_if_changed(&mut self, cell: Box<dyn ChatCell>) {
        let id = cell.id().to_string();
        let fingerprint = cell_fingerprint(cell.as_ref());
        let already_committed = self.is_emitted_history_cell(&(id, fingerprint));
        self.cells.push(cell);
        if !already_committed {
            self.pending_history_cell_indices
                .push(self.cells.len().saturating_sub(1));
        }
    }

    fn replace_or_push_committed(&mut self, cell: Box<dyn ChatCell>) {
        if let Some(existing) = self
            .cells
            .iter_mut()
            .find(|existing| existing.id() == cell.id())
        {
            *existing = cell;
        } else {
            self.push_committed_cell(cell);
        }
    }

    fn is_emitted_history_cell(&self, fingerprint: &(String, String)) -> bool {
        self.emitted_history_cells
            .iter()
            .any(|emitted| emitted == fingerprint)
    }

    fn flush_terminal_active_tool_chains(&mut self, add_spacer: bool) {
        let Some(chains) = self.active_tool_chains.take() else {
            return;
        };
        if !tool_chains_terminal(&chains) {
            return;
        }
        let id = chains
            .first()
            .map(|chain| format!("tools:{}", chain.id))
            .unwrap_or_else(|| "tools".to_string());
        let spacer_id = format!("{id}:spacer");
        self.push_committed_cell_if_changed(Box::new(self.completed_tool_cell(id, chains)));
        if add_spacer {
            self.push_committed_cell_if_changed(Box::new(NoticeCell::blank(spacer_id)));
        }
    }

    fn completed_tool_cell(
        &self,
        id: impl Into<String>,
        chains: Vec<ChatToolChainView>,
    ) -> ToolChainCell {
        if self.options.show_tool_details {
            ToolChainCell::expanded(id, chains)
        } else {
            ToolChainCell::collapsed(id, chains)
        }
    }
}

impl Default for TranscriptState {
    fn default() -> Self {
        Self::new(TranscriptOptions::default())
    }
}

fn tool_chain_identity(chains: &[ChatToolChainView]) -> Option<&str> {
    chains.first().map(|chain| chain.id.as_str())
}

fn tool_chains_terminal(chains: &[ChatToolChainView]) -> bool {
    !chains.is_empty()
        && chains.iter().all(|chain| {
            matches!(
                chain.status,
                ChatProgressStatus::Succeeded
                    | ChatProgressStatus::Failed
                    | ChatProgressStatus::Cancelled
                    | ChatProgressStatus::Stale
            )
        })
}

fn cell_fingerprint(cell: &dyn ChatCell) -> String {
    cell.display_lines(u16::MAX, &CellRenderState)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn visible_tail(mut lines: Vec<Line<'static>>, height: u16) -> Vec<Line<'static>> {
    let height = usize::from(height);
    if lines.len() > height {
        lines.drain(..lines.len() - height);
    }
    lines
}

fn short(value: &str) -> String {
    value.get(..8).unwrap_or(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::protocol::{
        ChatMessageView, ChatSessionSummary, ChatToolCallView, ChatToolChainView, ChatTurn,
    };

    #[test]
    fn confirmed_user_message_replaces_matching_pending_echo() {
        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
            session_id: "s-1".into(),
            message: ChatMessageView {
                id: "local-user:1".into(),
                role: "user_pending".into(),
                content: "hello".into(),
                ref_: None,
            },
        }));
        state.apply_chat_event(ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
            session_id: "s-1".into(),
            message: ChatMessageView {
                id: "sha256:abc".into(),
                role: "user".into(),
                content: "hello".into(),
                ref_: Some("sha256:abc".into()),
            },
        }));

        assert!(state.pending_user_messages.is_empty());
        assert_eq!(state.cells.len(), 1);
        assert_eq!(state.cells[0].id(), "sha256:abc");
    }

    #[test]
    fn session_selected_does_not_emit_transcript_notice() {
        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::SessionSelected(ChatSessionSummary {
            session_id: "c4c49a6a-9426-4153-b817-856f7248aaa2".into(),
            status: None,
            lifecycle: None,
            updated_at_ns: None,
            run_count: 11,
            provider: None,
            model: None,
            active_run: None,
        }));

        assert!(state.drain_pending_history_lines(80).is_empty());
        assert!(state.cells.is_empty());
    }

    #[test]
    fn reconnect_and_gap_notices_use_stable_cells() {
        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::Reconnecting {
            from: 12,
            reason: "stream closed".into(),
        });
        state.apply_chat_event(ChatEvent::GapObserved {
            requested_from: 9,
            retained_from: 20,
        });
        state.apply_chat_event(ChatEvent::GapObserved {
            requested_from: 10,
            retained_from: 21,
        });

        assert_eq!(state.cells.len(), 2);
        assert_eq!(state.cells[0].id(), "journal-reconnecting");
        assert_eq!(state.cells[1].id(), "journal-gap");
        let history = state
            .reflow_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("journal reconnecting from #12: stream closed"));
        assert!(history.contains("journal gap  requested #10, retained from #21"));
        assert!(!history.contains("requested #9"));
    }

    #[test]
    fn tool_chain_updates_render_as_active_tool_cell() {
        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![ChatToolChainView {
                id: "run-1:1".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Running,
                calls: vec![ChatToolCallView {
                    id: "call-1".into(),
                    tool_id: None,
                    tool_name: "read".into(),
                    status: ChatProgressStatus::Running,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: Some("src/main.rs".into()),
                    arguments_preview: None,
                    result_preview: None,
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }],
        });

        let active = state.active_cell.as_ref().expect("active cell");
        assert_eq!(active.kind(), ChatCellKind::ToolChain);
        assert!(
            active
                .display_lines(80, &CellRenderState)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                .contains("read src/main.rs")
        );
    }

    #[test]
    fn tool_chain_running_updates_do_not_enter_history_until_terminal() {
        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![ChatToolChainView {
                id: "run-1:1".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Running,
                calls: vec![ChatToolCallView {
                    id: "call-1".into(),
                    tool_id: None,
                    tool_name: "list_dir".into(),
                    status: ChatProgressStatus::Running,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: Some(r#"{"path":"spec"}"#.into()),
                    result_preview: None,
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }],
        });
        assert!(state.drain_pending_history_lines(80).is_empty());

        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![ChatToolChainView {
                id: "run-1:1".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Succeeded,
                calls: vec![ChatToolCallView {
                    id: "call-1".into(),
                    tool_id: None,
                    tool_name: "list_dir".into(),
                    status: ChatProgressStatus::Succeeded,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: Some(r#"{"path":"spec"}"#.into()),
                    result_preview: Some(r#"{"ok":true}"#.into()),
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }],
        });
        assert!(state.drain_pending_history_lines(80).is_empty());

        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: Vec::new(),
        });
        let history = state
            .drain_pending_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("tools 1 calls  ok"));
        assert!(!history.contains("result"));
        assert!(!history.contains("args"));
        assert!(!history.contains("running"));
    }

    #[test]
    fn show_tool_details_keeps_completed_tool_args_and_results() {
        let mut state = TranscriptState::new(TranscriptOptions {
            show_tool_details: true,
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![ChatToolChainView {
                id: "run-1:1".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Running,
                calls: vec![ChatToolCallView {
                    id: "call-1".into(),
                    tool_id: None,
                    tool_name: "list_dir".into(),
                    status: ChatProgressStatus::Running,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: Some(r#"{"path":"spec"}"#.into()),
                    result_preview: None,
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }],
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![ChatToolChainView {
                id: "run-1:1".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Succeeded,
                calls: vec![ChatToolCallView {
                    id: "call-1".into(),
                    tool_id: None,
                    tool_name: "list_dir".into(),
                    status: ChatProgressStatus::Succeeded,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: Some(r#"{"path":"spec"}"#.into()),
                    result_preview: Some(r#"{"ok":true}"#.into()),
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }],
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: Vec::new(),
        });

        let history = state
            .drain_pending_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("tools 1 calls  ok"));
        assert!(history.contains(r#"args {"path":"spec"}"#));
        assert!(history.contains(r#"result {"ok":true}"#));
    }

    #[test]
    fn show_tool_details_expands_reconstructed_history_tool_cells() {
        let mut state = TranscriptState::new(TranscriptOptions {
            show_tool_details: true,
        });
        state.apply_chat_event(ChatEvent::TranscriptDelta(ChatDelta::ReplaceTurns {
            session_id: "s-1".into(),
            turns: vec![ChatTurn {
                turn_id: "turn-1".into(),
                user: None,
                assistant: Some(ChatMessageView {
                    id: "assistant-1".into(),
                    role: "assistant".into(),
                    content: "done".into(),
                    ref_: None,
                }),
                run: None,
                tool_chains: vec![ChatToolChainView {
                    id: "run-1:1".into(),
                    title: "tools 1 calls".into(),
                    status: ChatProgressStatus::Succeeded,
                    calls: vec![ChatToolCallView {
                        id: "call-1".into(),
                        tool_id: None,
                        tool_name: "read_file".into(),
                        status: ChatProgressStatus::Succeeded,
                        group_index: Some(1),
                        parallel_safe: Some(false),
                        resource_key: Some("README.md".into()),
                        arguments_preview: Some(r#"{"path":"README.md"}"#.into()),
                        result_preview: Some(r#"{"ok":true}"#.into()),
                        error: None,
                    }],
                    summary: Some("1 execution groups".into()),
                }],
            }],
        }));

        let history = state
            .drain_pending_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("read_file README.md  ok"));
        assert!(history.contains(r#"args {"path":"README.md"}"#));
        assert!(history.contains(r#"result {"ok":true}"#));
        assert!(history.contains("done"));
    }

    #[test]
    fn consecutive_terminal_tool_batches_are_spaced_after_the_block() {
        fn chain(id: &str, title: &str, status: ChatProgressStatus) -> ChatToolChainView {
            ChatToolChainView {
                id: id.into(),
                title: title.into(),
                status,
                calls: vec![ChatToolCallView {
                    id: format!("{id}:call"),
                    tool_id: None,
                    tool_name: "glob".into(),
                    status,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: None,
                    result_preview: None,
                    error: None,
                }],
                summary: Some("1 execution groups".into()),
            }
        }

        let mut state = TranscriptState::default();
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![chain(
                "run-1:1",
                "tools 1 calls",
                ChatProgressStatus::Running,
            )],
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![chain(
                "run-1:1",
                "tools 1 calls",
                ChatProgressStatus::Succeeded,
            )],
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![chain(
                "run-1:2",
                "tools 4 calls",
                ChatProgressStatus::Running,
            )],
        });
        let first_batch = state
            .drain_pending_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(first_batch.len(), 1);
        assert!(first_batch[0].contains("tools 1 calls"));

        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: vec![chain(
                "run-1:2",
                "tools 4 calls",
                ChatProgressStatus::Succeeded,
            )],
        });
        state.apply_chat_event(ChatEvent::ToolChainsChanged {
            session_id: "s-1".into(),
            chains: Vec::new(),
        });
        let second_batch = state
            .drain_pending_history_lines(80)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(second_batch.len(), 2);
        assert!(second_batch[0].contains("tools 4 calls"));
        assert_eq!(second_batch[1], "");
    }
}
