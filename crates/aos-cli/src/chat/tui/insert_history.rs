use std::fmt;
use std::io::{Result, Write};

use crossterm::Command;
use crossterm::cursor::{MoveDown, MoveTo, MoveToColumn, RestorePosition, SavePosition};
use crossterm::queue;
use crossterm::style::{
    Attribute, Color as CrosstermColor, Colors, Print, SetAttribute, SetBackgroundColor, SetColors,
    SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};

use crate::chat::tui::custom_terminal::ChatTerminal;
use crate::chat::tui::wrapping::{
    RtOptions, adaptive_wrap_line, line_contains_url_like, line_has_mixed_url_and_non_url_tokens,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertHistoryMode {
    Standard,
    Zellij,
}

impl InsertHistoryMode {
    pub(crate) fn new(is_zellij: bool) -> Self {
        if is_zellij {
            Self::Zellij
        } else {
            Self::Standard
        }
    }
}

pub(crate) fn insert_history_lines_with_mode(
    terminal: &mut ChatTerminal,
    lines: Vec<Line<'static>>,
    mode: InsertHistoryMode,
) -> Result<bool> {
    if lines.is_empty() || terminal.viewport_area().is_empty() {
        return Ok(false);
    }

    let screen_size = terminal.size()?;
    let mut area = terminal.viewport_area();
    let last_cursor_pos = terminal.last_known_cursor_pos();
    let wrap_width = area.width.max(1) as usize;
    let (wrapped, wrapped_rows) = wrap_history_lines(&lines, wrap_width);
    if wrapped_rows == 0 {
        return Ok(false);
    }

    let mut should_update_area = false;
    let mut needs_full_repaint = false;

    match mode {
        InsertHistoryMode::Zellij => {
            let space_below = screen_size.height.saturating_sub(area.bottom());
            let shift_down = wrapped_rows.min(space_below);
            let scroll_up_amount = wrapped_rows.saturating_sub(shift_down);
            {
                let writer = terminal.backend_mut();
                if scroll_up_amount > 0 {
                    queue!(writer, MoveTo(0, screen_size.height.saturating_sub(1)))?;
                    for _ in 0..scroll_up_amount {
                        queue!(writer, Print("\n"))?;
                    }
                    needs_full_repaint = true;
                }

                if shift_down > 0 {
                    area.y += shift_down;
                    should_update_area = true;
                }

                let cursor_top = area.top().saturating_sub(scroll_up_amount + shift_down);
                queue!(writer, MoveTo(0, cursor_top))?;
                for (index, line) in wrapped.iter().enumerate() {
                    if index > 0 {
                        queue!(writer, Print("\r\n"))?;
                    }
                    write_history_line(writer, line, wrap_width)?;
                }
                queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;
            }
        }
        InsertHistoryMode::Standard => {
            let cursor_top = if area.bottom() < screen_size.height {
                let scroll_amount = wrapped_rows.min(screen_size.height - area.bottom());
                {
                    let writer = terminal.backend_mut();
                    let top_1based = area.top() + 1;
                    queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
                    queue!(writer, MoveTo(0, area.top()))?;
                    for _ in 0..scroll_amount {
                        queue!(writer, Print("\x1bM"))?;
                    }
                    queue!(writer, ResetScrollRegion)?;
                }
                let cursor_top = area.top().saturating_sub(1);
                area.y += scroll_amount;
                should_update_area = true;
                cursor_top
            } else {
                area.top().saturating_sub(1)
            };

            {
                let writer = terminal.backend_mut();
                queue!(writer, SetScrollRegion(1..area.top()))?;
                queue!(writer, MoveTo(0, cursor_top))?;
                for line in &wrapped {
                    queue!(writer, Print("\r\n"))?;
                    write_history_line(writer, line, wrap_width)?;
                }
                queue!(writer, ResetScrollRegion)?;
                queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;
            }
        }
    }

    if should_update_area {
        terminal.set_viewport_area(area);
    }
    terminal.note_history_rows_inserted(wrapped_rows);
    terminal.invalidate_viewport();
    Ok(needs_full_repaint)
}

pub(crate) fn scroll_region_up<W: Write>(
    writer: &mut W,
    region: std::ops::Range<u16>,
    scroll_by: u16,
) -> Result<()> {
    if scroll_by == 0 || region.is_empty() {
        return Ok(());
    }

    queue!(writer, SetScrollRegion(region.start + 1..region.end))?;
    queue!(writer, MoveTo(0, region.end.saturating_sub(1)))?;
    for _ in 0..scroll_by {
        queue!(writer, Print("\n"))?;
    }
    queue!(writer, ResetScrollRegion)
}

fn wrap_history_lines(lines: &[Line<'static>], wrap_width: usize) -> (Vec<Line<'static>>, u16) {
    let mut wrapped = Vec::new();
    let mut wrapped_rows = 0usize;

    for line in lines {
        let line_wrapped =
            if line_contains_url_like(line) && !line_has_mixed_url_and_non_url_tokens(line) {
                vec![line.clone()]
            } else {
                adaptive_wrap_line(line, RtOptions::new(wrap_width))
            };
        wrapped_rows += line_wrapped
            .iter()
            .map(|wrapped_line| wrapped_line.width().max(1).div_ceil(wrap_width))
            .sum::<usize>();
        wrapped.extend(line_wrapped);
    }

    (wrapped, wrapped_rows.try_into().unwrap_or(u16::MAX))
}

fn write_history_line<W: Write>(writer: &mut W, line: &Line<'_>, wrap_width: usize) -> Result<()> {
    let physical_rows = line.width().max(1).div_ceil(wrap_width) as u16;
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(
                writer,
                MoveDown(1),
                MoveToColumn(0),
                Clear(ClearType::UntilNewLine)
            )?;
        }
        queue!(writer, RestorePosition)?;
    }

    queue!(
        writer,
        SetColors(Colors::new(
            line.style
                .fg
                .map(Into::into)
                .unwrap_or(CrosstermColor::Reset),
            line.style
                .bg
                .map(Into::into)
                .unwrap_or(CrosstermColor::Reset),
        )),
        Clear(ClearType::UntilNewLine)
    )?;

    let merged_spans: Vec<Span<'_>> = line
        .spans
        .iter()
        .map(|span| Span {
            style: span.style.patch(line.style),
            content: span.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())?;
    queue!(
        writer,
        SetForegroundColor(CrosstermColor::Reset),
        SetBackgroundColor(CrosstermColor::Reset),
        SetAttribute(Attribute::Reset),
    )
}

fn write_spans<'a, W, I>(writer: &mut W, spans: I) -> Result<()>
where
    W: Write,
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    for span in spans {
        let span_modifier = span.style.add_modifier - span.style.sub_modifier;
        if span_modifier != modifier {
            queue_modifier_diff(writer, modifier, span_modifier)?;
            modifier = span_modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(next_fg.into(), next_bg.into()))
            )?;
            fg = next_fg;
            bg = next_bg;
        }
        queue!(writer, Print(span.content.clone()))?;
    }
    Ok(())
}

fn queue_modifier_diff<W: Write>(writer: &mut W, from: Modifier, to: Modifier) -> Result<()> {
    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attribute::NoReverse))?;
    }
    if removed.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attribute::NormalIntensity))?;
        if to.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(Attribute::Dim))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attribute::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attribute::NoUnderline))?;
    }
    if removed.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attribute::NormalIntensity))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attribute::NotCrossedOut))?;
    }

    let added = to - from;
    if added.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attribute::Reverse))?;
    }
    if added.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attribute::Bold))?;
    }
    if added.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attribute::Italic))?;
    }
    if added.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attribute::Underlined))?;
    }
    if added.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attribute::Dim))?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attribute::CrossedOut))?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetScrollRegion(std::ops::Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "SetScrollRegion must be emitted as ANSI",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "ResetScrollRegion must be emitted as ANSI",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}
