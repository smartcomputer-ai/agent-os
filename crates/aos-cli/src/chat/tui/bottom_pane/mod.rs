pub(crate) mod composer;
pub(crate) mod list_selection;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::chat::protocol::{
    ChatEvent, ChatProgressStatus, ChatSettingsView, ChatStatus, DEFAULT_CHAT_MODEL,
    DEFAULT_CHAT_PROVIDER, reasoning_effort_label,
};
use crate::chat::tui::bottom_pane::composer::{ComposerState, composer_band_paragraph};
use crate::chat::tui::bottom_pane::list_selection::{
    ListSelectionAction, ListSelectionView, PickerSelection,
};
use crate::chat::tui::slash::{SlashCommandKind, slash_query};
use crate::chat::tui::theme::composer_band_style;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BottomPaneState {
    composer: ComposerState,
    status: String,
    run_control_active: bool,
    current_session_id: Option<String>,
    settings: Option<ChatSettingsView>,
    active_view: Option<BottomPaneView>,
    slash_popup: Option<ListSelectionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BottomPaneView {
    Picker(ListSelectionView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BottomPaneAction {
    None,
    Changed,
    Submit(String),
    ExitRequested,
    PickerSelected(PickerSelection),
    PickerRejected(String),
    SlashCommandSelected(SlashCommandKind),
}

impl Default for BottomPaneState {
    fn default() -> Self {
        Self {
            composer: ComposerState::default(),
            status: "ready".into(),
            run_control_active: false,
            current_session_id: None,
            settings: None,
            active_view: None,
            slash_popup: None,
        }
    }
}

impl BottomPaneState {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> BottomPaneAction {
        if let Some(view) = self.active_view.as_mut() {
            return match view.handle_key(key) {
                BottomPaneViewAction::None => BottomPaneAction::None,
                BottomPaneViewAction::Changed => BottomPaneAction::Changed,
                BottomPaneViewAction::Close => {
                    self.active_view = None;
                    BottomPaneAction::Changed
                }
                BottomPaneViewAction::PickerSelected(selection) => {
                    self.active_view = None;
                    BottomPaneAction::PickerSelected(selection)
                }
                BottomPaneViewAction::PickerRejected(reason) => {
                    self.active_view = None;
                    BottomPaneAction::PickerRejected(reason)
                }
            };
        }
        if self.slash_popup.is_some() && slash_popup_owns_key(&key) {
            return self.handle_slash_popup_key(key);
        }
        match self.composer.handle_key(key) {
            crate::chat::tui::bottom_pane::composer::ComposerAction::None => BottomPaneAction::None,
            crate::chat::tui::bottom_pane::composer::ComposerAction::Changed => {
                self.sync_slash_popup();
                BottomPaneAction::Changed
            }
            crate::chat::tui::bottom_pane::composer::ComposerAction::Submit(text) => {
                self.slash_popup = None;
                BottomPaneAction::Submit(text)
            }
            crate::chat::tui::bottom_pane::composer::ComposerAction::ExitRequested => {
                BottomPaneAction::ExitRequested
            }
        }
    }

    pub(crate) fn insert_paste(&mut self, text: &str) {
        if self.active_view.is_none() {
            self.composer.insert_str(text);
            self.sync_slash_popup();
        }
    }

    pub(crate) fn open_model_picker(&mut self) {
        let (current, editable) = self
            .settings
            .as_ref()
            .map(|settings| (settings.model.as_str(), settings.model_editable))
            .unwrap_or((DEFAULT_CHAT_MODEL, true));
        self.active_view = Some(BottomPaneView::Picker(ListSelectionView::model(
            current, editable,
        )));
        self.slash_popup = None;
    }

    pub(crate) fn open_provider_picker(&mut self) {
        let (current, editable) = self
            .settings
            .as_ref()
            .map(|settings| (settings.provider.as_str(), settings.provider_editable))
            .unwrap_or((DEFAULT_CHAT_PROVIDER, true));
        self.active_view = Some(BottomPaneView::Picker(ListSelectionView::provider(
            current, editable,
        )));
        self.slash_popup = None;
    }

    pub(crate) fn open_effort_picker(&mut self) {
        let (current, editable) = self
            .settings
            .as_ref()
            .map(|settings| (settings.reasoning_effort, settings.effort_editable))
            .unwrap_or((None, true));
        self.active_view = Some(BottomPaneView::Picker(ListSelectionView::effort(
            current, editable,
        )));
        self.slash_popup = None;
    }

    pub(crate) fn open_max_tokens_picker(&mut self) {
        let (current, editable) = self
            .settings
            .as_ref()
            .map(|settings| (settings.max_tokens, settings.max_tokens_editable))
            .unwrap_or((None, true));
        self.active_view = Some(BottomPaneView::Picker(ListSelectionView::max_tokens(
            current, editable,
        )));
        self.slash_popup = None;
    }

    pub(crate) fn open_session_picker(
        &mut self,
        sessions: &[crate::chat::protocol::ChatSessionSummary],
    ) {
        self.active_view = Some(BottomPaneView::Picker(ListSelectionView::sessions(
            sessions,
            self.current_session_id.as_deref(),
        )));
        self.slash_popup = None;
    }

    pub(crate) fn desired_height(&self) -> u16 {
        self.content_height() + self.footer_height()
    }

    pub(crate) fn apply_chat_event(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::Connected(info) => {
                self.status = "connected".into();
                self.current_session_id = Some(info.session_id.clone());
                self.settings = Some(info.settings.clone());
            }
            ChatEvent::SessionSelected(summary) => {
                self.current_session_id = Some(summary.session_id.clone());
            }
            ChatEvent::HistoryReset { session_id } => {
                self.current_session_id = Some(session_id.clone());
                self.run_control_active = false;
            }
            ChatEvent::RunChanged(run) => {
                self.run_control_active = matches!(
                    run.status,
                    ChatProgressStatus::Queued | ChatProgressStatus::Running
                );
                if self.run_control_active {
                    self.status = format!("run {} running", run.run_seq);
                }
            }
            ChatEvent::StatusChanged(ChatStatus {
                session_id,
                status,
                settings,
                ..
            }) => {
                self.status = status.clone();
                self.run_control_active = status_allows_run_control(status);
                self.current_session_id = Some(session_id.clone());
                self.settings = Some(settings.clone());
            }
            ChatEvent::GapObserved { .. } => {
                self.status = "journal gap; refreshed".into();
            }
            ChatEvent::Reconnecting { from, .. } => {
                self.status = format!("reconnecting journal #{from}");
            }
            ChatEvent::Error(error) => {
                self.status = error.message.clone();
            }
            _ => {}
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let chunks = if let Some(view) = self.active_view.as_ref() {
            Layout::vertical([Constraint::Length(view.desired_height())]).split(area)
        } else if let Some(slash_popup) = self.slash_popup.as_ref() {
            Layout::vertical([
                Constraint::Length(self.composer_band_height()),
                Constraint::Length(slash_popup.desired_height()),
            ])
            .split(area)
        } else {
            Layout::vertical([
                Constraint::Length(self.composer_band_height()),
                Constraint::Length(1),
            ])
            .split(area)
        };

        if let Some(view) = self.active_view.as_ref() {
            view.render(chunks[0], buf);
        } else if let Some(slash_popup) = self.slash_popup.as_ref() {
            self.render_composer_band(chunks[0], buf);
            slash_popup.render(chunks[1], buf);
        } else {
            self.render_composer_band(chunks[0], buf);
            Paragraph::new(self.status_line()).render(chunks[1], buf);
        }
    }

    pub(crate) fn cursor_position(&self, area: Rect) -> Option<Position> {
        if self.active_view.is_some() {
            return None;
        }
        if let Some(slash_popup) = self.slash_popup.as_ref() {
            let chunks = Layout::vertical([
                Constraint::Length(self.composer_band_height()),
                Constraint::Length(slash_popup.desired_height()),
            ])
            .split(area);
            return self
                .composer
                .cursor_position(self.composer_text_area(chunks[0]));
        }
        let chunks = Layout::vertical([
            Constraint::Length(self.composer_band_height()),
            Constraint::Length(1),
        ])
        .split(area);
        self.composer
            .cursor_position(self.composer_text_area(chunks[0]))
    }

    pub(crate) fn cursor_style(&self) -> SetCursorStyle {
        if self.active_view.is_some() {
            SetCursorStyle::DefaultUserShape
        } else {
            SetCursorStyle::SteadyBar
        }
    }

    fn content_height(&self) -> u16 {
        if let Some(view) = self.active_view.as_ref() {
            return view.desired_height();
        }
        let popup_height = self
            .slash_popup
            .as_ref()
            .map(ListSelectionView::desired_height)
            .unwrap_or(0);
        popup_height + self.composer_band_height()
    }

    fn footer_height(&self) -> u16 {
        u16::from(self.active_view.is_none() && self.slash_popup.is_none())
    }

    fn composer_band_height(&self) -> u16 {
        self.composer.desired_height().saturating_add(2).min(7)
    }

    fn render_composer_band(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("")
            .style(composer_band_style())
            .render(area, buf);
        composer_band_paragraph(&self.composer).render(self.composer_text_area(area), buf);
    }

    fn composer_text_area(&self, area: Rect) -> Rect {
        if area.height >= 3 {
            Rect {
                y: area.y.saturating_add(1),
                height: area.height.saturating_sub(2),
                ..area
            }
        } else {
            area
        }
    }

    fn handle_slash_popup_key(&mut self, key: KeyEvent) -> BottomPaneAction {
        let Some(slash_popup) = self.slash_popup.as_mut() else {
            return BottomPaneAction::None;
        };
        match slash_popup.handle_key(key) {
            ListSelectionAction::None => BottomPaneAction::None,
            ListSelectionAction::Changed => BottomPaneAction::Changed,
            ListSelectionAction::Close => {
                self.slash_popup = None;
                BottomPaneAction::Changed
            }
            ListSelectionAction::Rejected(reason) => {
                self.slash_popup = None;
                BottomPaneAction::PickerRejected(reason)
            }
            ListSelectionAction::Selected(PickerSelection::SlashCommand(command)) => {
                self.slash_popup = None;
                self.composer.clear();
                BottomPaneAction::SlashCommandSelected(command)
            }
            ListSelectionAction::Selected(selection) => {
                self.slash_popup = None;
                BottomPaneAction::PickerSelected(selection)
            }
        }
    }

    fn sync_slash_popup(&mut self) {
        self.slash_popup = slash_query(self.composer.text()).map(ListSelectionView::slash_commands);
    }

    fn status_line(&self) -> Line<'static> {
        let status_style = if self.run_control_active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![Span::styled(self.status.clone(), status_style)];
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
        if self.run_control_active {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "Ctrl-C interrupt",
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}

fn status_allows_run_control(status: &str) -> bool {
    matches!(status, "running" | "cancelling" | "paused")
}

enum BottomPaneViewAction {
    None,
    Changed,
    Close,
    PickerSelected(PickerSelection),
    PickerRejected(String),
}

impl BottomPaneView {
    fn handle_key(&mut self, key: KeyEvent) -> BottomPaneViewAction {
        match self {
            BottomPaneView::Picker(view) => match view.handle_key(key) {
                ListSelectionAction::None => BottomPaneViewAction::None,
                ListSelectionAction::Changed => BottomPaneViewAction::Changed,
                ListSelectionAction::Close => BottomPaneViewAction::Close,
                ListSelectionAction::Selected(selection) => {
                    BottomPaneViewAction::PickerSelected(selection)
                }
                ListSelectionAction::Rejected(reason) => {
                    BottomPaneViewAction::PickerRejected(reason)
                }
            },
        }
    }

    fn desired_height(&self) -> u16 {
        match self {
            BottomPaneView::Picker(view) => view.desired_height(),
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            BottomPaneView::Picker(view) => view.render(area, buf),
        }
    }
}

fn slash_popup_owns_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        crossterm::event::KeyCode::Esc
            | crossterm::event::KeyCode::Up
            | crossterm::event::KeyCode::Down
            | crossterm::event::KeyCode::Home
            | crossterm::event::KeyCode::End
            | crossterm::event::KeyCode::Enter
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::protocol::{ChatRunView, run_status};
    use aos_agent::RunLifecycle;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn slash_popup_keeps_cursor_in_composer_band_above_results() {
        let mut pane = BottomPaneState::default();
        for ch in ['/', 'p'] {
            assert_eq!(
                pane.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                BottomPaneAction::Changed
            );
        }

        let cursor = pane
            .cursor_position(Rect::new(0, 0, 80, 8))
            .expect("composer cursor");

        assert_eq!(cursor.y, 1);
    }

    #[test]
    fn empty_composer_cursor_is_on_prompt_row() {
        let pane = BottomPaneState::default();
        let cursor = pane
            .cursor_position(Rect::new(0, 0, 80, pane.desired_height()))
            .expect("composer cursor");

        assert_eq!(pane.desired_height(), 4);
        assert_eq!(cursor, Position::new(2, 1));
    }

    #[test]
    fn multiline_composer_renders_text_after_newline() {
        let mut pane = BottomPaneState::default();
        for ch in ['s', 'u', 'p', 'e', 'r'] {
            assert_eq!(
                pane.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                BottomPaneAction::Changed
            );
        }
        assert_eq!(
            pane.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            BottomPaneAction::Changed
        );
        for ch in ['v', 'i', 's', 'i', 'b', 'l', 'e'] {
            assert_eq!(
                pane.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                BottomPaneAction::Changed
            );
        }

        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame.area(), frame.buffer_mut()))
            .unwrap();

        assert!(format!("{}", terminal.backend()).contains("visible"));
    }

    #[test]
    fn reconnect_and_gap_events_update_status_line() {
        let mut pane = BottomPaneState::default();

        pane.apply_chat_event(&ChatEvent::Reconnecting {
            from: 42,
            reason: "stream closed".into(),
        });
        assert!(
            pane.status_line()
                .to_string()
                .contains("reconnecting journal #42")
        );

        pane.apply_chat_event(&ChatEvent::GapObserved {
            requested_from: 40,
            retained_from: 45,
        });
        assert!(
            pane.status_line()
                .to_string()
                .contains("journal gap; refreshed")
        );
    }

    #[test]
    fn active_run_status_shows_interrupt_hint() {
        let mut pane = BottomPaneState::default();
        pane.apply_chat_event(&ChatEvent::RunChanged(ChatRunView {
            id: "run-1".into(),
            run_seq: 7,
            lifecycle: RunLifecycle::Running,
            status: run_status(RunLifecycle::Running),
            provider: "openai-responses".into(),
            model: "gpt-5.3-codex".into(),
            reasoning_effort: None,
            input_refs: Vec::new(),
            output_ref: None,
            started_at_ns: 0,
            updated_at_ns: 0,
        }));

        let rendered = pane.status_line().to_string();
        assert!(rendered.contains("run 7 running"));
        assert!(rendered.contains("Ctrl-C interrupt"));
    }
}
