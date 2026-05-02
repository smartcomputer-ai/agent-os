pub(crate) mod composer;
pub(crate) mod list_selection;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::chat::protocol::{
    ChatEvent, ChatSettingsView, ChatStatus, DEFAULT_CHAT_MODEL, DEFAULT_CHAT_PROVIDER,
    reasoning_effort_label,
};
use crate::chat::tui::bottom_pane::composer::{ComposerState, composer_paragraph};
use crate::chat::tui::bottom_pane::list_selection::{
    ListSelectionAction, ListSelectionView, PickerSelection,
};
use crate::chat::tui::slash::{SlashCommandKind, slash_query};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BottomPaneState {
    composer: ComposerState,
    status: String,
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

    pub(crate) fn desired_height(&self) -> u16 {
        2 + self.content_height()
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

        let chunks = if let Some(view) = self.active_view.as_ref() {
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(view.desired_height()),
            ])
            .split(inner)
        } else if let Some(slash_popup) = self.slash_popup.as_ref() {
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(slash_popup.desired_height()),
                Constraint::Length(self.composer.desired_height()),
            ])
            .split(inner)
        } else {
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(self.composer.desired_height()),
            ])
            .split(inner)
        };

        Paragraph::new(self.status_line()).render(chunks[0], buf);
        if let Some(view) = self.active_view.as_ref() {
            view.render(chunks[1], buf);
        } else if let Some(slash_popup) = self.slash_popup.as_ref() {
            slash_popup.render(chunks[1], buf);
            composer_paragraph(&self.composer).render(chunks[2], buf);
        } else {
            composer_paragraph(&self.composer).render(chunks[1], buf);
        }
    }

    pub(crate) fn cursor_position(&self, area: Rect) -> Option<Position> {
        if self.active_view.is_some() {
            return None;
        }
        let inner = area.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 0,
        });
        if let Some(slash_popup) = self.slash_popup.as_ref() {
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(slash_popup.desired_height()),
                Constraint::Length(self.composer.desired_height()),
            ])
            .split(inner);
            return self.composer.cursor_position(chunks[2]);
        }
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(self.composer.desired_height()),
        ])
        .split(inner);
        self.composer.cursor_position(chunks[1])
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
        popup_height + self.composer.desired_height()
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
