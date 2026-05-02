use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatCellKind {
    Message,
    Run,
    Error,
    Notice,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CellRenderState;

pub(crate) trait ChatCell: std::fmt::Debug + Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> ChatCellKind;
    fn display_lines(&self, width: u16, state: &CellRenderState) -> Vec<Line<'static>>;

    fn desired_height(&self, width: u16, state: &CellRenderState) -> u16 {
        self.display_lines(width, state)
            .len()
            .try_into()
            .unwrap_or(0)
    }

    fn is_stream_continuation(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageCell {
    id: String,
    role: String,
    content: String,
}

impl MessageCell {
    pub(crate) fn new(
        id: impl Into<String>,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            role: role.into(),
            content: content.into(),
        }
    }
}

impl ChatCell for MessageCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::Message
    }

    fn display_lines(&self, _width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        let pending = self.role == "user_pending";
        let style = match self.role.as_str() {
            "user" => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            "assistant" => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            "user_pending" => Style::default().fg(Color::DarkGray),
            _ => Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        };
        let content_style = if pending {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let role = if pending {
            "user".to_string()
        } else {
            self.role.clone()
        };
        let mut lines = Vec::new();
        lines.push(Line::from(vec![Span::styled(role, style)]));
        for line in self.content.lines() {
            lines.push(Line::from(vec![Span::styled(
                format!("  {line}"),
                content_style,
            )]));
        }
        if self.content.is_empty() {
            lines.push(Line::from(vec![Span::styled("  ", content_style)]));
        }
        lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoticeCell {
    id: String,
    text: String,
}

impl NoticeCell {
    pub(crate) fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

impl ChatCell for NoticeCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::Notice
    }

    fn display_lines(&self, _width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        vec![Line::styled(
            self.text.clone(),
            Style::default().fg(Color::DarkGray),
        )]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunCell {
    id: String,
    text: String,
}

impl RunCell {
    pub(crate) fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

impl ChatCell for RunCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::Run
    }

    fn display_lines(&self, _width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        vec![Line::styled(
            self.text.clone(),
            Style::default().fg(Color::Yellow),
        )]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorCell {
    id: String,
    message: String,
}

impl ErrorCell {
    pub(crate) fn new(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
        }
    }
}

impl ChatCell for ErrorCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::Error
    }

    fn display_lines(&self, _width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        vec![Line::styled(
            format!("error: {}", self.message),
            Style::default().fg(Color::Red),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_user_message_renders_muted_user_label_and_content() {
        let cell = MessageCell::new("local-user:1", "user_pending", "hello");
        let lines = cell.display_lines(80, &CellRenderState);

        assert_eq!(lines[0].spans[0].content.as_ref(), "user");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[1].spans[0].content.as_ref(), "  hello");
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::DarkGray));
    }
}
