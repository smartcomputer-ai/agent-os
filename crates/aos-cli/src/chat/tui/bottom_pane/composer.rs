use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ComposerState {
    text: String,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    None,
    Changed,
    Submit(String),
    ExitRequested,
}

impl ComposerState {
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> ComposerAction {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ComposerAction::ExitRequested
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && self.text.is_empty() =>
            {
                ComposerAction::ExitRequested
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char('\n');
                ComposerAction::Changed
            }
            KeyCode::Enter => {
                if self.text.trim().is_empty() {
                    return ComposerAction::None;
                }
                let submitted = std::mem::take(&mut self.text);
                self.cursor = 0;
                ComposerAction::Submit(submitted)
            }
            KeyCode::Backspace => {
                if self.backspace() {
                    ComposerAction::Changed
                } else {
                    ComposerAction::None
                }
            }
            KeyCode::Delete => {
                if self.delete() {
                    ComposerAction::Changed
                } else {
                    ComposerAction::None
                }
            }
            KeyCode::Left => {
                self.move_left();
                ComposerAction::None
            }
            KeyCode::Right => {
                self.move_right();
                ComposerAction::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                ComposerAction::None
            }
            KeyCode::End => {
                self.cursor = self.text.len();
                ComposerAction::None
            }
            KeyCode::Char(ch) => {
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                    self.insert_char(ch);
                    ComposerAction::Changed
                } else {
                    ComposerAction::None
                }
            }
            _ => ComposerAction::None,
        }
    }

    pub(crate) fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub(crate) fn desired_height(&self) -> u16 {
        let line_count = self.text.lines().count().max(1) as u16;
        line_count.clamp(1, 5)
    }

    pub(crate) fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut source_lines = self.text.lines();
        let first = source_lines.next().unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(first.to_string()),
        ]));
        for line in source_lines {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(Color::Cyan)),
                Span::raw(line.to_string()),
            ]));
        }
        lines
    }

    pub(crate) fn cursor_position(&self, area: Rect) -> Option<Position> {
        if area.height == 0 || area.width == 0 {
            return None;
        }
        let before = &self.text[..self.cursor];
        let row = before.chars().filter(|ch| *ch == '\n').count() as u16;
        if row >= area.height {
            return None;
        }
        let col_text = before.rsplit('\n').next().unwrap_or_default();
        let col = UnicodeWidthStr::width(col_text) as u16;
        Some(Position::new(
            area.x.saturating_add(2).saturating_add(col),
            area.y.saturating_add(row),
        ))
    }

    fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn backspace(&mut self) -> bool {
        let Some((idx, _)) = self.text[..self.cursor].char_indices().last() else {
            return false;
        };
        self.text.drain(idx..self.cursor);
        self.cursor = idx;
        true
    }

    fn delete(&mut self) -> bool {
        let Some(ch) = self.text[self.cursor..].chars().next() else {
            return false;
        };
        let end = self.cursor + ch.len_utf8();
        self.text.drain(self.cursor..end);
        true
    }

    fn move_left(&mut self) {
        if let Some((idx, _)) = self.text[..self.cursor].char_indices().last() {
            self.cursor = idx;
        }
    }

    fn move_right(&mut self) {
        if let Some(ch) = self.text[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }
}

pub(crate) fn composer_paragraph(composer: &ComposerState) -> Paragraph<'static> {
    Paragraph::new(composer.render_lines()).wrap(Wrap { trim: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_submits_and_clears_text() {
        let mut composer = ComposerState::default();
        composer.insert_str("hello");
        let action = composer.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, ComposerAction::Submit("hello".into()));
        assert!(composer.is_empty());
    }

    #[test]
    fn ctrl_j_inserts_newline() {
        let mut composer = ComposerState::default();
        composer.insert_str("hello");
        let action = composer.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(action, ComposerAction::Changed);
        assert_eq!(composer.text(), "hello\n");
    }
}
