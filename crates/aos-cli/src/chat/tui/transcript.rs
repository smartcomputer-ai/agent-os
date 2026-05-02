use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::chat::protocol::{ChatDelta, ChatEvent};
use crate::chat::tui::cell::{
    CellRenderState, ChatCell, ErrorCell, MessageCell, NoticeCell, RunCell,
};

#[derive(Debug, Default)]
pub(crate) struct TranscriptState {
    cells: Vec<Box<dyn ChatCell>>,
    active_cell: Option<Box<dyn ChatCell>>,
    active_cell_revision: u64,
}

impl TranscriptState {
    pub(crate) fn apply_chat_event(&mut self, event: ChatEvent) {
        match event {
            ChatEvent::Connected(info) => {
                self.replace_or_push(Box::new(NoticeCell::new(
                    "connected",
                    format!(
                        "connected world {} session {}",
                        short(&info.world_id),
                        short(&info.session_id)
                    ),
                )));
            }
            ChatEvent::SessionSelected(summary) => {
                self.replace_or_push(Box::new(NoticeCell::new(
                    "session",
                    format!(
                        "session {}  runs {}",
                        short(&summary.session_id),
                        summary.run_count
                    ),
                )));
            }
            ChatEvent::HistoryReset { session_id } => {
                self.cells.clear();
                self.active_cell = None;
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                self.cells.push(Box::new(NoticeCell::new(
                    "history-reset",
                    format!("switched to session {}", short(&session_id)),
                )));
            }
            ChatEvent::TranscriptDelta(delta) => self.apply_delta(delta),
            ChatEvent::RunChanged(run) => {
                self.active_cell = Some(Box::new(RunCell::new(
                    format!("active-run:{}", run.id),
                    format!("run {} {:?} {}", run.run_seq, run.lifecycle, run.model),
                )));
                self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            }
            ChatEvent::ToolChainsChanged { chains, .. } => {
                if let Some(chain) = chains.first() {
                    self.active_cell = Some(Box::new(RunCell::new(
                        format!("active-tools:{}", chain.id),
                        format!("{} {:?}", chain.title, chain.status),
                    )));
                    self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
                }
            }
            ChatEvent::CompactionsChanged { compactions, .. } => {
                if let Some(compaction) = compactions.last() {
                    self.replace_or_push(Box::new(NoticeCell::new(
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
                self.replace_or_push(Box::new(NoticeCell::new(
                    format!("gap:{requested_from}"),
                    format!("journal gap requested {requested_from}, retained {retained_from}"),
                )));
            }
            ChatEvent::Reconnecting { from, reason } => {
                self.replace_or_push(Box::new(NoticeCell::new(
                    "reconnecting",
                    format!("reconnecting from {from}: {reason}"),
                )));
            }
            ChatEvent::Error(error) => {
                self.replace_or_push(Box::new(ErrorCell::new(
                    format!("error:{}", self.cells.len()),
                    error.message,
                )));
            }
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = Vec::new();
        let render_state = CellRenderState;
        for cell in &self.cells {
            let _ = (
                cell.kind(),
                cell.desired_height(inner.width, &render_state),
                cell.is_stream_continuation(),
            );
            lines.extend(cell.display_lines(inner.width, &render_state));
            lines.push(Line::default());
        }
        if let Some(active_cell) = self.active_cell.as_ref() {
            lines.extend(active_cell.display_lines(inner.width, &render_state));
        }
        if lines.is_empty() {
            lines.push(Line::styled(
                "AOS Chat shell",
                Style::default().fg(Color::DarkGray),
            ));
        }

        let visible = visible_tail(lines, inner.height);
        Paragraph::new(visible)
            .wrap(Wrap { trim: false })
            .render(inner, buf);
    }

    fn apply_delta(&mut self, delta: ChatDelta) {
        match delta {
            ChatDelta::ReplaceTurns { turns, .. } => {
                self.cells.clear();
                for turn in turns {
                    if let Some(user) = turn.user {
                        self.cells.push(Box::new(MessageCell::new(
                            user.id,
                            user.role,
                            user.content,
                        )));
                    }
                    if let Some(assistant) = turn.assistant {
                        self.cells.push(Box::new(MessageCell::new(
                            assistant.id,
                            assistant.role,
                            assistant.content,
                        )));
                    }
                    if let Some(run) = turn.run {
                        self.cells.push(Box::new(RunCell::new(
                            format!("run:{}", run.id),
                            format!("run {} {:?} {}", run.run_seq, run.lifecycle, run.model),
                        )));
                    }
                }
            }
            ChatDelta::AppendMessage { message, .. } => {
                self.cells.push(Box::new(MessageCell::new(
                    message.id,
                    message.role,
                    message.content,
                )));
            }
        }
    }

    fn replace_or_push(&mut self, cell: Box<dyn ChatCell>) {
        if let Some(existing) = self
            .cells
            .iter_mut()
            .find(|existing| existing.id() == cell.id())
        {
            *existing = cell;
        } else {
            self.cells.push(cell);
        }
    }
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
