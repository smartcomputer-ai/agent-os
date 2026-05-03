use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub(crate) fn append_markdown(
    markdown_source: &str,
    width: Option<usize>,
    lines: &mut Vec<Line<'static>>,
) {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown_source, options);
    let mut writer = MarkdownWriter::new(width);
    writer.run(parser);
    lines.extend(writer.lines);
}

struct MarkdownWriter {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    current_text: String,
    inline_styles: Vec<Style>,
    block_stack: Vec<BlockContext>,
    list_stack: Vec<ListContext>,
    code_block: Option<CodeBlock>,
    needs_blank: bool,
    width: Option<usize>,
}

#[derive(Debug, Clone)]
struct BlockContext {
    prefix: String,
    subsequent_prefix: String,
    first_line: bool,
}

#[derive(Debug, Clone)]
struct ListContext {
    next_number: Option<u64>,
}

#[derive(Debug, Clone)]
struct CodeBlock {
    lang: Option<String>,
    lines: Vec<String>,
}

impl MarkdownWriter {
    fn new(width: Option<usize>) -> Self {
        Self {
            lines: Vec::new(),
            current: Vec::new(),
            current_text: String::new(),
            inline_styles: Vec::new(),
            block_stack: Vec::new(),
            list_stack: Vec::new(),
            code_block: None,
            needs_blank: false,
            width,
        }
    }

    fn run<'a>(&mut self, parser: Parser<'a>) {
        for event in parser {
            self.handle_event(event);
        }
        self.flush_current();
    }

    fn handle_event<'a>(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::SoftBreak | Event::HardBreak => self.soft_break(),
            Event::Rule => self.rule(),
            Event::Html(html) | Event::InlineHtml(html) => self.text(html),
            Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
        }
    }

    fn start_tag<'a>(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote => self.start_blockquote(),
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(start) => self.list_stack.push(ListContext { next_number: start }),
            Tag::Item => self.start_item(),
            Tag::Emphasis => {
                self.push_inline_style(Style::default().add_modifier(Modifier::ITALIC))
            }
            Tag::Strong => self.push_inline_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_inline_style(Style::default().add_modifier(Modifier::CROSSED_OUT))
            }
            Tag::Link { dest_url, .. } => self.push_inline_style(link_style(dest_url.as_ref())),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_current();
                self.needs_blank = true;
            }
            TagEnd::Heading(_) => {
                self.flush_current();
                self.pop_inline_style();
                self.needs_blank = true;
            }
            TagEnd::BlockQuote => {
                self.flush_current();
                self.block_stack.pop();
                self.needs_blank = true;
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.needs_blank = true;
            }
            TagEnd::Item => {
                self.flush_current();
                self.block_stack.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.pop_inline_style();
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_paragraph(&mut self) {
        self.push_blank_if_needed();
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.push_blank_if_needed();
        let marker = format!("{} ", "#".repeat(level as usize));
        self.push_span(Span::styled(marker, heading_style(level)));
        self.push_inline_style(heading_style(level));
    }

    fn start_blockquote(&mut self) {
        self.push_blank_if_needed();
        self.block_stack.push(BlockContext {
            prefix: "> ".into(),
            subsequent_prefix: "> ".into(),
            first_line: true,
        });
    }

    fn start_code_block<'a>(&mut self, kind: CodeBlockKind<'a>) {
        self.flush_current();
        self.push_blank_if_needed();
        let lang = match kind {
            CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
            _ => None,
        };
        self.code_block = Some(CodeBlock {
            lang,
            lines: Vec::new(),
        });
    }

    fn end_code_block(&mut self) {
        let Some(block) = self.code_block.take() else {
            return;
        };
        let prefix = self.current_prefix();
        if let Some(lang) = block.lang.as_ref() {
            self.lines.push(Line::from(vec![
                Span::raw(prefix.clone()),
                Span::styled(format!("```{lang}"), code_fence_style()),
            ]));
        }
        for line in block.lines {
            self.lines.push(Line::from(vec![
                Span::raw(prefix.clone()),
                Span::styled(line, code_block_style()),
            ]));
        }
        if block.lang.is_some() {
            self.lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled("```", code_fence_style()),
            ]));
        }
        self.needs_blank = true;
    }

    fn start_item(&mut self) {
        self.flush_current();
        self.push_blank_if_needed();
        let depth = self.list_stack.len().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let marker = if let Some(list) = self.list_stack.last_mut() {
            if let Some(number) = list.next_number {
                list.next_number = Some(number.saturating_add(1));
                format!("{indent}{number}. ")
            } else {
                format!("{indent}- ")
            }
        } else {
            "- ".into()
        };
        let subsequent_prefix = " ".repeat(marker.chars().count());
        self.block_stack.push(BlockContext {
            prefix: marker,
            subsequent_prefix,
            first_line: true,
        });
    }

    fn text<'a>(&mut self, text: CowStr<'a>) {
        if let Some(block) = self.code_block.as_mut() {
            block.lines.extend(split_code_lines(text.as_ref()));
            return;
        }
        self.push_span(Span::styled(text.to_string(), self.current_style()));
    }

    fn code<'a>(&mut self, code: CowStr<'a>) {
        self.push_span(Span::styled(code.to_string(), inline_code_style()));
    }

    fn soft_break(&mut self) {
        if let Some(block) = self.code_block.as_mut() {
            block.lines.push(String::new());
        } else {
            self.flush_current();
        }
    }

    fn rule(&mut self) {
        self.flush_current();
        self.push_blank_if_needed();
        self.lines
            .push(Line::styled("---", Style::default().fg(Color::DarkGray)));
        self.needs_blank = true;
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.current_text.push_str(span.content.as_ref());
        self.current.push(span);
    }

    fn flush_current(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let prefix = self.current_prefix();
        let subsequent_prefix = self.current_subsequent_prefix();
        let content_width = self
            .width
            .unwrap_or(usize::MAX)
            .saturating_sub(prefix_width(&prefix))
            .max(1);
        let wrapped = wrap_spans(&self.current, content_width);
        for (index, line) in wrapped.into_iter().enumerate() {
            let line_prefix = if index == 0 {
                prefix.clone()
            } else {
                subsequent_prefix.clone()
            };
            let mut spans = Vec::new();
            if !line_prefix.is_empty() {
                spans.push(Span::raw(line_prefix));
            }
            spans.extend(line);
            self.lines.push(Line::from(spans));
        }
        for block in &mut self.block_stack {
            block.first_line = false;
        }
        self.current.clear();
        self.current_text.clear();
        self.needs_blank = false;
    }

    fn push_blank_if_needed(&mut self) {
        if self.needs_blank && !self.lines.last().is_some_and(line_is_blank) {
            self.lines.push(Line::default());
        }
        self.needs_blank = false;
    }

    fn push_inline_style(&mut self, style: Style) {
        self.inline_styles.push(style);
    }

    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    fn current_style(&self) -> Style {
        self.inline_styles
            .iter()
            .copied()
            .fold(Style::default(), Style::patch)
    }

    fn current_prefix(&self) -> String {
        self.block_stack
            .iter()
            .map(|block| {
                if block.first_line {
                    block.prefix.as_str()
                } else {
                    block.subsequent_prefix.as_str()
                }
            })
            .collect()
    }

    fn current_subsequent_prefix(&self) -> String {
        self.block_stack
            .iter()
            .map(|block| block.subsequent_prefix.as_str())
            .collect()
    }
}

fn split_code_lines(text: &str) -> Vec<String> {
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if text.ends_with('\n') {
        lines.push(String::new());
    }
    lines
}

fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut current_width = 0usize;
    for span in spans {
        for ch in span.content.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(0)
                .max(1);
            if current_width > 0 && current_width.saturating_add(ch_width) > width {
                lines.push(Vec::new());
                current_width = 0;
            }
            let line = lines.last_mut().expect("line");
            if let Some(last) = line.last_mut()
                && last.style == span.style
            {
                last.content.to_mut().push(ch);
            } else {
                line.push(Span::styled(ch.to_string(), span.style));
            }
            current_width = current_width.saturating_add(ch_width);
        }
    }
    lines
}

fn prefix_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| {
            unicode_width::UnicodeWidthChar::width(ch)
                .unwrap_or(0)
                .max(1)
        })
        .sum()
}

fn line_is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

fn heading_style(level: HeadingLevel) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => base.fg(Color::White),
        _ => base.fg(Color::Gray),
    }
}

fn inline_code_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn code_block_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn code_fence_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn link_style(_dest: &str) -> Style {
    Style::default().fg(Color::Cyan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn renders_common_markdown_blocks() {
        let mut lines = Vec::new();
        append_markdown("# Title\n\n- one\n- two\n\n> quote", Some(80), &mut lines);

        let rendered = text(&lines);
        assert_eq!(rendered[0], "# Title");
        assert!(rendered.iter().any(|line| line == "- one"));
        assert!(rendered.iter().any(|line| line == "- two"));
        assert!(rendered.iter().any(|line| line == "> quote"));
    }

    #[test]
    fn nested_lists_use_parent_continuation_indent() {
        let mut lines = Vec::new();
        append_markdown(
            "- Required:\n  - $kind = \"manifest\"\n  - air_version = \"2\"",
            Some(80),
            &mut lines,
        );

        let rendered = text(&lines);
        assert_eq!(
            rendered,
            vec![
                "- Required:",
                "    - $kind = \"manifest\"",
                "    - air_version = \"2\"",
            ]
        );
    }

    #[test]
    fn fenced_code_preserves_lines() {
        let mut lines = Vec::new();
        append_markdown("```rust\nfn main() {}\n```", Some(80), &mut lines);

        let rendered = text(&lines);
        assert_eq!(rendered, vec!["```rust", "fn main() {}", "", "```"]);
    }
}
