pub(crate) mod composer;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::chat::protocol::{ChatEvent, ChatSettingsView, ChatStatus, reasoning_effort_label};
use crate::chat::tui::bottom_pane::composer::{ComposerState, composer_paragraph};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BottomPaneState {
    composer: ComposerState,
    status: String,
    settings: Option<ChatSettingsView>,
}

impl Default for BottomPaneState {
    fn default() -> Self {
        Self {
            composer: ComposerState::default(),
            status: "ready".into(),
            settings: None,
        }
    }
}

impl BottomPaneState {
    pub(crate) fn composer_mut(&mut self) -> &mut ComposerState {
        &mut self.composer
    }

    pub(crate) fn desired_height(&self) -> u16 {
        2 + self.composer.desired_height()
    }

    pub(crate) fn apply_chat_event(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::Connected(info) => {
                self.status = "connected".into();
                self.settings = Some(info.settings.clone());
            }
            ChatEvent::StatusChanged(ChatStatus {
                status, settings, ..
            }) => {
                self.status = status.clone();
                self.settings = Some(settings.clone());
            }
            ChatEvent::Error(error) => {
                self.status = error.message.clone();
            }
            _ => {}
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(self.composer.desired_height()),
        ])
        .split(inner);

        Paragraph::new(self.status_line()).render(chunks[0], buf);
        composer_paragraph(&self.composer).render(chunks[1], buf);
    }

    pub(crate) fn cursor_position(&self, area: Rect) -> Option<Position> {
        let inner = area.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 0,
        });
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(self.composer.desired_height()),
        ])
        .split(inner);
        self.composer.cursor_position(chunks[1])
    }

    fn status_line(&self) -> Line<'static> {
        let mut spans = vec![Span::styled(
            self.status.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )];
        if let Some(settings) = &self.settings {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                settings.model.clone(),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::raw("  effort "));
            spans.push(Span::styled(
                reasoning_effort_label(settings.reasoning_effort),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}
