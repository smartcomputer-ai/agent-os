use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RtOptions {
    width: usize,
}

impl RtOptions {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
        }
    }
}

pub(crate) fn adaptive_wrap_line(line: &Line<'_>, options: RtOptions) -> Vec<Line<'static>> {
    let width = options.width;
    let tokens = tokenize_line(line);
    if tokens.is_empty() {
        return vec![Line::default()];
    }

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0usize;

    for token in tokens {
        if token.text == "\n" {
            rows.push(Line::from(std::mem::take(&mut row)).style(line.style));
            row_width = 0;
            continue;
        }

        let token_width = UnicodeWidthStr::width(token.text.as_str());
        if row_width > 0 && row_width.saturating_add(token_width) > width {
            rows.push(Line::from(std::mem::take(&mut row)).style(line.style));
            row_width = 0;
        }

        if token_width > width && !is_url_like(&token.text) {
            let mut remainder = token.text.as_str();
            while !remainder.is_empty() {
                let remaining_width = width.saturating_sub(row_width).max(1);
                let (head, tail) = split_at_display_width(remainder, remaining_width);
                row.push(Span::styled(head.to_string(), token.style));
                row_width = row_width.saturating_add(UnicodeWidthStr::width(head));
                remainder = tail;
                if !remainder.is_empty() {
                    rows.push(Line::from(std::mem::take(&mut row)).style(line.style));
                    row_width = 0;
                }
            }
        } else {
            row_width = row_width.saturating_add(token_width);
            row.push(Span::styled(token.text, token.style));
        }
    }

    if !row.is_empty() || rows.is_empty() {
        rows.push(Line::from(row).style(line.style));
    }
    rows
}

pub(crate) fn line_contains_url_like(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.split_whitespace())
        .any(is_url_like)
}

pub(crate) fn line_has_mixed_url_and_non_url_tokens(line: &Line<'_>) -> bool {
    let mut has_url = false;
    let mut has_non_url = false;
    for token in line
        .spans
        .iter()
        .flat_map(|span| span.content.split_whitespace())
    {
        if is_url_like(token) {
            has_url = true;
        } else {
            has_non_url = true;
        }
    }
    has_url && has_non_url
}

#[derive(Debug, Clone)]
struct StyledToken {
    text: String,
    style: Style,
}

fn tokenize_line(line: &Line<'_>) -> Vec<StyledToken> {
    let mut tokens = Vec::new();
    for span in &line.spans {
        let style = span.style.patch(line.style);
        let mut current = String::new();
        let mut current_is_space: Option<bool> = None;
        for ch in span.content.chars() {
            if ch == '\n' {
                if !current.is_empty() {
                    tokens.push(StyledToken {
                        text: std::mem::take(&mut current),
                        style,
                    });
                }
                current_is_space = None;
                tokens.push(StyledToken {
                    text: "\n".to_string(),
                    style,
                });
                continue;
            }
            let is_space = ch.is_whitespace();
            if current_is_space.is_some_and(|was_space| was_space != is_space) {
                tokens.push(StyledToken {
                    text: std::mem::take(&mut current),
                    style,
                });
            }
            current_is_space = Some(is_space);
            current.push(ch);
        }
        if !current.is_empty() {
            tokens.push(StyledToken {
                text: current,
                style,
            });
        }
    }
    tokens
}

fn split_at_display_width(value: &str, width: usize) -> (&str, &str) {
    if width == 0 {
        return ("", value);
    }

    let mut consumed_bytes = 0usize;
    let mut consumed_width = 0usize;
    for (idx, ch) in value.char_indices() {
        let ch_width = UnicodeWidthStr::width(ch.to_string().as_str()).max(1);
        if consumed_width > 0 && consumed_width.saturating_add(ch_width) > width {
            break;
        }
        consumed_bytes = idx + ch.len_utf8();
        consumed_width = consumed_width.saturating_add(ch_width);
        if consumed_width >= width {
            break;
        }
    }

    if consumed_bytes == 0 {
        let Some((idx, ch)) = value.char_indices().next() else {
            return ("", "");
        };
        consumed_bytes = idx + ch.len_utf8();
    }
    value.split_at(consumed_bytes)
}

fn is_url_like(token: &str) -> bool {
    token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("file://")
        || token.starts_with("ssh://")
        || token.starts_with("git@")
}
