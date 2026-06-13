use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::chat::protocol::{ChatProgressStatus, ChatToolCallView, ChatToolChainView};
use crate::chat::tui::markdown::append_markdown;
use crate::chat::tui::theme::COMPOSER_BG;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatCellKind {
    Message,
    Reasoning,
    Run,
    ToolChain,
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

    fn display_lines(&self, width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        if self.role == "assistant" {
            return assistant_message_lines(&self.content, width);
        }

        let pending = self.role == "user_pending";
        let user_like = matches!(self.role.as_str(), "user" | "user_pending");
        let row_bg = user_like.then_some(COMPOSER_BG);
        let marker = match self.role.as_str() {
            "user" | "user_pending" => "› ",
            "assistant" => "• ",
            _ => "  ",
        };
        let marker_style = match self.role.as_str() {
            "user" => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::DIM),
            "assistant" => Style::default().fg(Color::DarkGray),
            "user_pending" => Style::default().fg(Color::DarkGray),
            _ => Style::default().fg(Color::DarkGray),
        };
        let marker_style = with_bg(marker_style, row_bg);
        let content_style = if pending {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        let content_style = with_bg(content_style, row_bg);
        let mut lines = Vec::new();

        let content_lines = wrap_message_lines(&self.content, width.saturating_sub(2).max(1));
        for (index, line) in content_lines.iter().enumerate() {
            let prefix = if index == 0 { marker } else { "  " };
            let prefix_style = if index == 0 {
                marker_style
            } else {
                with_bg(Style::default(), row_bg)
            };
            let mut spans = vec![
                Span::styled(prefix, prefix_style),
                Span::styled(line.clone(), content_style),
            ];
            if let Some(bg) = row_bg {
                let used_width = text_width(prefix).saturating_add(text_width(line));
                let padding_width = usize::from(width).saturating_sub(used_width);
                if padding_width > 0 {
                    spans.push(Span::styled(
                        " ".repeat(padding_width),
                        Style::default().bg(bg),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
        if matches!(self.role.as_str(), "user" | "user_pending" | "assistant") {
            lines.push(Line::default());
        }
        lines
    }
}

fn assistant_message_lines(content: &str, width: u16) -> Vec<Line<'static>> {
    let marker_style = Style::default().fg(Color::DarkGray);
    let mut rendered = Vec::new();
    append_markdown(
        content,
        Some(usize::from(width.saturating_sub(2).max(1))),
        &mut rendered,
    );
    if rendered.is_empty() {
        rendered.push(Line::default());
    }

    let mut lines = Vec::new();
    for (index, line) in rendered.into_iter().enumerate() {
        let prefix = if index == 0 { "• " } else { "  " };
        let mut spans = vec![Span::styled(prefix, marker_style)];
        spans.extend(line.spans);
        lines.push(Line::from(spans).style(line.style));
    }
    lines.push(Line::default());
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReasoningCell {
    id: String,
    content: String,
}

impl ReasoningCell {
    pub(crate) fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
        }
    }
}

impl ChatCell for ReasoningCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::Reasoning
    }

    fn display_lines(&self, width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        reasoning_lines(&self.content, width, true)
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

    pub(crate) fn blank(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: String::new(),
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
pub(crate) struct ToolChainCell {
    id: String,
    chains: Vec<ChatToolChainView>,
    display: ToolChainDisplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolChainDisplay {
    Collapsed,
    Expanded,
}

impl ToolChainCell {
    pub(crate) fn new(id: impl Into<String>, chains: Vec<ChatToolChainView>) -> Self {
        Self::expanded(id, chains)
    }

    pub(crate) fn collapsed(id: impl Into<String>, chains: Vec<ChatToolChainView>) -> Self {
        Self {
            id: id.into(),
            chains,
            display: ToolChainDisplay::Collapsed,
        }
    }

    pub(crate) fn expanded(id: impl Into<String>, chains: Vec<ChatToolChainView>) -> Self {
        Self {
            id: id.into(),
            chains,
            display: ToolChainDisplay::Expanded,
        }
    }
}

impl ChatCell for ToolChainCell {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ChatCellKind {
        ChatCellKind::ToolChain
    }

    fn display_lines(&self, width: u16, _state: &CellRenderState) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (chain_index, chain) in self.chains.iter().enumerate() {
            if chain_index > 0 && matches!(self.display, ToolChainDisplay::Expanded) {
                lines.push(Line::default());
            }
            if let Some(reasoning) = chain.reasoning.as_ref() {
                lines.extend(reasoning_lines(&reasoning.content, width, false));
            }
            lines.push(tool_chain_header(chain));
            if matches!(self.display, ToolChainDisplay::Collapsed) {
                continue;
            }

            let grouped = group_tool_calls(&chain.calls);
            for group in grouped {
                if let Some(group_index) = group.group_index {
                    let label = if group.calls.len() > 1 {
                        format!("  group {group_index} parallel")
                    } else {
                        format!("  group {group_index}")
                    };
                    lines.push(Line::styled(label, Style::default().fg(Color::DarkGray)));
                }
                for call in group.calls {
                    lines.extend(tool_call_lines(call, width));
                }
            }
        }
        lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorCell {
    id: String,
    message: String,
}

fn with_bg(style: Style, bg: Option<Color>) -> Style {
    if let Some(bg) = bg {
        style.bg(bg)
    } else {
        style
    }
}

fn wrap_message_lines(content: &str, width: u16) -> Vec<String> {
    if content.is_empty() {
        return vec![String::new()];
    }
    let width = usize::from(width.max(1));
    let mut out = Vec::new();
    for source_line in content.lines() {
        if source_line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in source_line.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(0)
                .max(1);
            if current_width > 0 && current_width.saturating_add(ch_width) > width {
                out.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(ch);
            current_width = current_width.saturating_add(ch_width);
        }
        out.push(current);
    }
    out
}

fn reasoning_lines(content: &str, width: u16, trailing_blank: bool) -> Vec<Line<'static>> {
    let marker_style = Style::default().fg(Color::DarkGray);
    let content_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::ITALIC);
    let mut lines = Vec::new();
    let wrapped = wrap_message_lines(content, width.saturating_sub(2).max(1));
    for (index, line) in wrapped.iter().enumerate() {
        let prefix = if index == 0 { "• " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(prefix, marker_style),
            Span::styled(line.clone(), content_style),
        ]));
    }
    if trailing_blank {
        lines.push(Line::default());
    }
    lines
}

fn text_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| {
            unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(0)
                .max(1)
        })
        .sum()
}

#[derive(Debug, Clone, Copy)]
struct ToolGroup<'a> {
    group_index: Option<u64>,
    calls: &'a [ChatToolCallView],
}

fn group_tool_calls(calls: &[ChatToolCallView]) -> Vec<ToolGroup<'_>> {
    if calls.is_empty() {
        return Vec::new();
    }

    let mut groups = Vec::new();
    let mut start = 0usize;
    let mut current_group = calls[0].group_index;
    for (index, call) in calls.iter().enumerate().skip(1) {
        if call.group_index != current_group {
            groups.push(ToolGroup {
                group_index: current_group,
                calls: &calls[start..index],
            });
            start = index;
            current_group = call.group_index;
        }
    }
    groups.push(ToolGroup {
        group_index: current_group,
        calls: &calls[start..],
    });
    groups
}

fn tool_chain_header(chain: &ChatToolChainView) -> Line<'static> {
    let title = if chain.title.is_empty() {
        format!("tools {} calls", chain.calls.len())
    } else {
        chain.title.clone()
    };
    let mut spans = vec![
        Span::styled("tools ", Style::default().fg(Color::Yellow)),
        Span::styled(
            title
                .strip_prefix("tools ")
                .map(str::to_owned)
                .unwrap_or(title),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            progress_label(chain.status),
            progress_style(chain.status).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(summary) = chain.summary.as_ref().filter(|summary| !summary.is_empty()) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            summary.clone(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn tool_call_lines(call: &ChatToolCallView, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let status = progress_label(call.status);
    let mut title = call.tool_name.clone();
    if let Some(resource_key) = call.resource_key.as_ref().filter(|value| !value.is_empty()) {
        title.push(' ');
        title.push_str(resource_key);
    }
    let available = usize::from(width.saturating_sub(18).max(12));
    let title = truncate(&title, available);
    lines.push(Line::from(vec![
        Span::styled("    ", Style::default()),
        Span::styled(title, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(status, progress_style(call.status)),
    ]));

    if let Some(arguments) = call
        .arguments_preview
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("      args ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate(arguments, usize::from(width.saturating_sub(12).max(12))),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if let Some(result) = call
        .result_preview
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("      result ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate(result, usize::from(width.saturating_sub(14).max(12))),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if let Some(error) = call.error.as_ref().filter(|value| !value.is_empty()) {
        lines.push(Line::from(vec![
            Span::styled("      error ", Style::default().fg(Color::Red)),
            Span::styled(
                truncate(error, usize::from(width.saturating_sub(13).max(12))),
                Style::default().fg(Color::Red),
            ),
        ]));
    }
    lines
}

fn progress_label(status: ChatProgressStatus) -> &'static str {
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

fn progress_style(status: ChatProgressStatus) -> Style {
    match status {
        ChatProgressStatus::Succeeded => Style::default().fg(Color::Green),
        ChatProgressStatus::Failed | ChatProgressStatus::Cancelled => {
            Style::default().fg(Color::Red)
        }
        ChatProgressStatus::Running => Style::default().fg(Color::Yellow),
        ChatProgressStatus::Queued | ChatProgressStatus::Waiting => {
            Style::default().fg(Color::DarkGray)
        }
        ChatProgressStatus::Stale | ChatProgressStatus::Unknown => Style::default().fg(Color::Gray),
    }
}

fn truncate(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in value.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch)
            .unwrap_or(0)
            .max(1);
        if width.saturating_add(ch_width) > max_width {
            if max_width > 3 {
                out.push_str("...");
            }
            return out;
        }
        out.push(ch);
        width = width.saturating_add(ch_width);
    }
    out
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

        assert_eq!(lines[0].spans[0].content.as_ref(), "› ");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[0].spans[0].style.bg, Some(COMPOSER_BG));
        assert_eq!(lines[0].spans[1].content.as_ref(), "hello");
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[0].spans[1].style.bg, Some(COMPOSER_BG));
    }

    #[test]
    fn user_message_background_fills_render_width() {
        let cell = MessageCell::new("user:1", "user", "hi");
        let lines = cell.display_lines(8, &CellRenderState);

        assert_eq!(lines[0].spans[0].style.bg, Some(COMPOSER_BG));
        assert_eq!(lines[0].spans[1].style.bg, Some(COMPOSER_BG));
        assert_eq!(lines[0].spans[2].content.as_ref(), "    ");
        assert_eq!(lines[0].spans[2].style.bg, Some(COMPOSER_BG));
    }

    #[test]
    fn assistant_message_uses_codex_style_marker() {
        let cell = MessageCell::new("assistant:1", "assistant", "pong");
        let lines = cell.display_lines(80, &CellRenderState);

        assert_eq!(lines[0].spans[0].content.as_ref(), "• ");
        assert_eq!(lines[0].spans[1].content.as_ref(), "pong");
    }

    #[test]
    fn assistant_message_renders_markdown_blocks() {
        let cell = MessageCell::new(
            "assistant:1",
            "assistant",
            "# Title\n\n- one\n- two\n\n```rust\nfn main() {}\n```",
        );
        let rendered = cell
            .display_lines(80, &CellRenderState)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line == "• # Title"));
        assert!(rendered.iter().any(|line| line == "  - one"));
        assert!(rendered.iter().any(|line| line == "  - two"));
        assert!(rendered.iter().any(|line| line == "  ```rust"));
        assert!(rendered.iter().any(|line| line == "  fn main() {}"));
    }

    #[test]
    fn assistant_message_rerenders_markdown_at_width() {
        let cell = MessageCell::new(
            "assistant:1",
            "assistant",
            "- alpha beta gamma delta epsilon",
        );
        let rendered = cell
            .display_lines(16, &CellRenderState)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.len() > 2);
        assert!(rendered[0].starts_with("• - "));
        assert!(rendered[1].starts_with("    "));
    }

    #[test]
    fn tool_chain_groups_parallel_calls() {
        let cell = ToolChainCell::new(
            "tools:1",
            vec![ChatToolChainView {
                id: "chain".into(),
                title: "tools 2 calls".into(),
                status: ChatProgressStatus::Running,
                reasoning: None,
                summary: Some("2 execution groups".into()),
                calls: vec![
                    ChatToolCallView {
                        id: "a".into(),
                        tool_id: None,
                        tool_name: "rg".into(),
                        status: ChatProgressStatus::Succeeded,
                        group_index: Some(1),
                        parallel_safe: Some(true),
                        resource_key: Some("SessionInput".into()),
                        arguments_preview: None,
                        result_preview: None,
                        error: None,
                    },
                    ChatToolCallView {
                        id: "b".into(),
                        tool_id: None,
                        tool_name: "read".into(),
                        status: ChatProgressStatus::Running,
                        group_index: Some(1),
                        parallel_safe: Some(true),
                        resource_key: Some("cell.rs".into()),
                        arguments_preview: Some("{\"path\":\"cell.rs\"}".into()),
                        result_preview: None,
                        error: None,
                    },
                ],
            }],
        );
        let rendered = cell
            .display_lines(80, &CellRenderState)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("tools 2 calls"));
        assert!(rendered.contains("group 1 parallel"));
        assert!(rendered.contains("rg SessionInput"));
        assert!(rendered.contains("read cell.rs"));
    }

    #[test]
    fn collapsed_tool_chain_shows_summary_only() {
        let cell = ToolChainCell::collapsed(
            "tools:1",
            vec![ChatToolChainView {
                id: "chain".into(),
                title: "tools 1 calls".into(),
                status: ChatProgressStatus::Succeeded,
                reasoning: None,
                summary: Some("1 execution groups".into()),
                calls: vec![ChatToolCallView {
                    id: "a".into(),
                    tool_id: None,
                    tool_name: "read_file".into(),
                    status: ChatProgressStatus::Succeeded,
                    group_index: Some(1),
                    parallel_safe: Some(true),
                    resource_key: None,
                    arguments_preview: Some(r#"{"path":"README.md"}"#.into()),
                    result_preview: Some(r#"{"ok":true}"#.into()),
                    error: None,
                }],
            }],
        );
        let rendered = cell
            .display_lines(80, &CellRenderState)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("tools 1 calls"));
        assert!(rendered.contains("ok"));
        assert!(!rendered.contains("read_file"));
        assert!(!rendered.contains("args"));
        assert!(!rendered.contains("result"));
    }

    #[test]
    fn collapsed_tool_chain_does_not_space_between_chains() {
        let chain = |id: &str, title: &str| ChatToolChainView {
            id: id.into(),
            title: title.into(),
            status: ChatProgressStatus::Succeeded,
            reasoning: None,
            summary: Some("1 execution groups".into()),
            calls: vec![ChatToolCallView {
                id: format!("{id}:call"),
                tool_id: None,
                tool_name: "read_file".into(),
                status: ChatProgressStatus::Succeeded,
                group_index: Some(1),
                parallel_safe: Some(false),
                resource_key: None,
                arguments_preview: None,
                result_preview: None,
                error: None,
            }],
        };
        let cell = ToolChainCell::collapsed(
            "tools:1",
            vec![
                chain("chain-1", "tools 1 calls"),
                chain("chain-2", "tools 4 calls"),
            ],
        );
        let rendered = cell
            .display_lines(80, &CellRenderState)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 2);
        assert!(rendered[0].contains("tools 1 calls"));
        assert!(rendered[1].contains("tools 4 calls"));
    }
}
