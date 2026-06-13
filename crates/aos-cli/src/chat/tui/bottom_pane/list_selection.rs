use aos_agent::ReasoningEffort;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::chat::protocol::{
    ChatSessionSummary, DEFAULT_CHAT_MODEL, DEFAULT_CHAT_PROVIDER, reasoning_effort_label,
};
use crate::chat::tui::slash::{SlashCommandKind, matching_slash_commands};

const MODEL_CHOICES: &[&str] = &[
    DEFAULT_CHAT_MODEL,
    "gpt-5.4",
    "gpt-5.3-codex-spark",
    "gpt-5.2-codex",
    "gpt-5.2",
];
const PROVIDER_CHOICES: &[&str] = &[DEFAULT_CHAT_PROVIDER, "openai", "anthropic", "mock"];
const TOKEN_CHOICES: &[Option<u64>] = &[
    None,
    Some(1024),
    Some(2048),
    Some(4096),
    Some(8192),
    Some(16384),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListSelectionView {
    title: Option<String>,
    rows: Vec<ListSelectionRow>,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListSelectionRow {
    label: String,
    description: String,
    value: PickerSelection,
    disabled_reason: Option<String>,
    current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerSelection {
    Model(String),
    Provider(String),
    Effort(Option<ReasoningEffort>),
    MaxTokens(Option<u64>),
    SlashCommand(SlashCommandKind),
    Session(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListSelectionAction {
    None,
    Changed,
    Close,
    Selected(PickerSelection),
    Rejected(String),
}

impl ListSelectionView {
    pub(crate) fn slash_commands(query: &str) -> Self {
        let rows = matching_slash_commands(query)
            .into_iter()
            .map(|command| {
                ListSelectionRow::new(
                    format!("/{}", command.name()),
                    command.description(),
                    PickerSelection::SlashCommand(command),
                )
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Self::new_untitled(vec![
                ListSelectionRow::new(
                    "no matches",
                    "keep typing or press Esc",
                    PickerSelection::SlashCommand(SlashCommandKind::Help),
                )
                .with_disabled_reason(Some("no matching command".into())),
            ]);
        }
        Self::new_untitled(rows)
    }

    pub(crate) fn sessions(
        sessions: &[ChatSessionSummary],
        current_session_id: Option<&str>,
    ) -> Self {
        let rows = sessions
            .iter()
            .map(|summary| {
                let current = current_session_id == Some(summary.session_id.as_str());
                ListSelectionRow::new(
                    short(&summary.session_id),
                    session_description(summary, current),
                    PickerSelection::Session(summary.session_id.clone()),
                )
                .with_current(current)
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Self::new(
                "Select session",
                vec![
                    ListSelectionRow::new(
                        "no sessions",
                        "start one with /new",
                        PickerSelection::Session(String::new()),
                    )
                    .with_disabled_reason(Some("no known sessions in this world".into())),
                ],
            );
        }
        Self::new("Select session", rows)
    }

    pub(crate) fn model(current: &str, editable: bool) -> Self {
        let disabled_reason = (!editable)
            .then(|| "model switching is locked after a run has been accepted".to_string());
        let mut choices = MODEL_CHOICES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        ensure_choice(&mut choices, current);
        Self::new(
            "Select model",
            choices
                .into_iter()
                .map(|model| {
                    ListSelectionRow::new(
                        model.clone(),
                        if model == current {
                            "current model"
                        } else {
                            "use for future runs in this session"
                        },
                        PickerSelection::Model(model.clone()),
                    )
                    .with_current(model == current)
                    .with_disabled_reason(disabled_reason.clone())
                })
                .collect(),
        )
    }

    pub(crate) fn provider(current: &str, editable: bool) -> Self {
        let disabled_reason = (!editable)
            .then(|| "provider switching is locked after a run has been accepted".to_string());
        let mut choices = PROVIDER_CHOICES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        ensure_choice(&mut choices, current);
        Self::new(
            "Select provider",
            choices
                .into_iter()
                .map(|provider| {
                    ListSelectionRow::new(
                        provider.clone(),
                        if provider == current {
                            "current provider"
                        } else {
                            "use for future runs in this session"
                        },
                        PickerSelection::Provider(provider.clone()),
                    )
                    .with_current(provider == current)
                    .with_disabled_reason(disabled_reason.clone())
                })
                .collect(),
        )
    }

    pub(crate) fn effort(current: Option<ReasoningEffort>, editable: bool) -> Self {
        let disabled_reason =
            (!editable).then(|| "wait for the current run before changing effort".to_string());
        let choices = [
            None,
            Some(ReasoningEffort::Low),
            Some(ReasoningEffort::Medium),
            Some(ReasoningEffort::High),
        ];
        Self::new(
            "Select thinking effort",
            choices
                .into_iter()
                .map(|effort| {
                    let label = reasoning_effort_label(effort).to_string();
                    ListSelectionRow::new(
                        label.clone(),
                        if effort == current {
                            "current effort"
                        } else {
                            "use for future runs"
                        },
                        PickerSelection::Effort(effort),
                    )
                    .with_current(effort == current)
                    .with_disabled_reason(disabled_reason.clone())
                })
                .collect(),
        )
    }

    pub(crate) fn max_tokens(current: Option<u64>, editable: bool) -> Self {
        let disabled_reason =
            (!editable).then(|| "wait for the current run before changing max tokens".to_string());
        let mut choices = TOKEN_CHOICES.to_vec();
        if !choices.contains(&current) {
            choices.push(current);
        }
        Self::new(
            "Select max tokens",
            choices
                .into_iter()
                .map(|tokens| {
                    let label = tokens
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".into());
                    ListSelectionRow::new(
                        label.clone(),
                        if tokens == current {
                            "current limit"
                        } else {
                            "use for future runs"
                        },
                        PickerSelection::MaxTokens(tokens),
                    )
                    .with_current(tokens == current)
                    .with_disabled_reason(disabled_reason.clone())
                })
                .collect(),
        )
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> ListSelectionAction {
        match key.code {
            KeyCode::Esc => ListSelectionAction::Close,
            KeyCode::Up => {
                self.move_previous();
                ListSelectionAction::Changed
            }
            KeyCode::Down => {
                self.move_next();
                ListSelectionAction::Changed
            }
            KeyCode::Home => {
                self.selected = 0;
                ListSelectionAction::Changed
            }
            KeyCode::End => {
                self.selected = self.rows.len().saturating_sub(1);
                ListSelectionAction::Changed
            }
            KeyCode::Enter => self
                .rows
                .get(self.selected)
                .map(|row| match row.disabled_reason.as_ref() {
                    Some(reason) => ListSelectionAction::Rejected(reason.clone()),
                    None => ListSelectionAction::Selected(row.value.clone()),
                })
                .unwrap_or(ListSelectionAction::None),
            _ => ListSelectionAction::None,
        }
    }

    pub(crate) fn desired_height(&self) -> u16 {
        self.title_height() + self.rows.len().min(8) as u16
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.render_lines(area.width)).render(area, buf);
    }

    fn new(title: impl Into<String>, rows: Vec<ListSelectionRow>) -> Self {
        Self::with_title(Some(title.into()), rows)
    }

    fn new_untitled(rows: Vec<ListSelectionRow>) -> Self {
        Self::with_title(None, rows)
    }

    fn with_title(title: Option<String>, rows: Vec<ListSelectionRow>) -> Self {
        let selected = rows.iter().position(|row| row.current).unwrap_or(0);
        Self {
            title,
            rows,
            selected,
        }
    }

    fn move_previous(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.rows.len() - 1);
    }

    fn render_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.rows.len() + usize::from(self.title.is_some()));
        if let Some(title) = &self.title {
            lines.push(Line::from(vec![
                Span::styled(
                    title.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  Esc close", Style::default().fg(Color::DarkGray)),
            ]));
        }
        for (idx, row) in self.rows.iter().enumerate() {
            let selected = idx == self.selected;
            let base = if row.disabled_reason.is_some() {
                Style::default().fg(Color::DarkGray)
            } else if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let marker = if selected { "> " } else { "  " };
            let current = if row.current { " *" } else { "" };
            let detail = row
                .disabled_reason
                .as_deref()
                .unwrap_or(row.description.as_str());
            lines.push(Line::from(vec![
                Span::styled(marker, base),
                Span::styled(row.label.clone(), base),
                Span::styled(current, Style::default().fg(Color::Green)),
                Span::styled("  ", Style::default().fg(Color::DarkGray)),
                Span::styled(detail.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
        lines
    }

    fn title_height(&self) -> u16 {
        u16::from(self.title.is_some())
    }
}

impl ListSelectionRow {
    fn new(
        label: impl Into<String>,
        description: impl Into<String>,
        value: PickerSelection,
    ) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
            value,
            disabled_reason: None,
            current: false,
        }
    }

    fn with_disabled_reason(mut self, disabled_reason: Option<String>) -> Self {
        self.disabled_reason = disabled_reason;
        self
    }

    fn with_current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
}

fn ensure_choice(choices: &mut Vec<String>, current: &str) {
    if !current.is_empty() && !choices.iter().any(|choice| choice == current) {
        choices.push(current.to_string());
    }
}

fn session_description(summary: &ChatSessionSummary, current: bool) -> String {
    let mut parts = Vec::new();
    if current {
        parts.push("current".to_string());
    }
    if let Some(lifecycle) = summary.lifecycle {
        parts.push(format!("{lifecycle:?}").to_ascii_lowercase());
    } else if let Some(status) = summary.status {
        parts.push(format!("{status:?}").to_ascii_lowercase());
    }
    parts.push(format!("{} runs", summary.run_count));
    if let Some(model) = summary.model.as_ref() {
        parts.push(model.clone());
    }
    parts.join("  ")
}

fn short(value: &str) -> String {
    value.get(..8).unwrap_or(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn picker_confirms_selected_value() {
        let mut picker = ListSelectionView::effort(None, true);
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            ListSelectionAction::Changed
        );
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ListSelectionAction::Selected(PickerSelection::Effort(Some(ReasoningEffort::Low)))
        );
    }

    #[test]
    fn disabled_picker_rows_do_not_confirm() {
        let mut picker = ListSelectionView::model(DEFAULT_CHAT_MODEL, false);
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ListSelectionAction::Rejected(
                "model switching is locked after a run has been accepted".into()
            )
        );
    }

    #[test]
    fn slash_command_picker_filters_by_prefix() {
        let mut picker = ListSelectionView::slash_commands("mo");
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ListSelectionAction::Selected(PickerSelection::SlashCommand(SlashCommandKind::Model))
        );
    }
}
