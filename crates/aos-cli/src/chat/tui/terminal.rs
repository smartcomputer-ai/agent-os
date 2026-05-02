use std::fmt;
use std::io::{IsTerminal, Result, Stdout, Write, stdout};
use std::panic;
use std::sync::Once;

use crossterm::Command;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::queue;
use crossterm::style::{
    Color as CrosstermColor, Colors, Print, SetAttribute, SetBackgroundColor, SetColors,
    SetForegroundColor,
};
use crossterm::terminal::{Clear, disable_raw_mode, enable_raw_mode};
use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use tokio::sync::broadcast;

use crate::chat::tui::frame::FrameRequester;

pub(crate) type ChatBackend = CrosstermBackend<Stdout>;

static PANIC_HOOK: Once = Once::new();

pub(crate) struct Tui {
    pub(crate) terminal: ChatTerminal,
    frame_requester: FrameRequester,
    draw_tx: broadcast::Sender<()>,
    pending_history_lines: Vec<Line<'static>>,
}

pub(crate) struct TuiFrame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor_position: Option<Position>,
}

impl TuiFrame<'_> {
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }

    pub(crate) fn set_cursor_position(&mut self, position: Position) {
        self.cursor_position = Some(position);
    }
}

pub(crate) struct ChatTerminal {
    backend: ChatBackend,
    buffers: [Buffer; 2],
    current: usize,
    viewport_area: Rect,
    last_known_screen_size: Size,
    last_known_cursor_pos: Position,
    hidden_cursor: bool,
}

impl Tui {
    pub(crate) fn init() -> Result<Self> {
        if !std::io::stdin().is_terminal() {
            return Err(std::io::Error::other("stdin is not a terminal"));
        }
        if !stdout().is_terminal() {
            return Err(std::io::Error::other("stdout is not a terminal"));
        }

        set_panic_hook();
        enable_terminal_modes()?;

        let (draw_tx, _) = broadcast::channel(1);
        let frame_requester = FrameRequester::new(draw_tx.clone());
        let terminal = ChatTerminal::new(CrosstermBackend::new(stdout()))?;

        Ok(Self {
            terminal,
            frame_requester,
            draw_tx,
            pending_history_lines: Vec::new(),
        })
    }

    pub(crate) fn frame_requester(&self) -> FrameRequester {
        self.frame_requester.clone()
    }

    pub(crate) fn draw_receiver(&self) -> broadcast::Receiver<()> {
        self.draw_tx.subscribe()
    }

    pub(crate) fn insert_history_lines(&mut self, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        self.pending_history_lines.extend(lines);
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn clear_viewport(&mut self) -> Result<()> {
        self.pending_history_lines.clear();
        self.terminal.clear()
    }

    pub(crate) fn draw(
        &mut self,
        desired_height: u16,
        render: impl FnOnce(&mut TuiFrame<'_>),
    ) -> Result<()> {
        self.terminal.update_inline_viewport(desired_height)?;
        self.flush_pending_history_lines()?;
        self.terminal.draw(render)
    }

    fn flush_pending_history_lines(&mut self) -> Result<()> {
        if self.pending_history_lines.is_empty() {
            return Ok(());
        }

        let lines = std::mem::take(&mut self.pending_history_lines);
        self.terminal.insert_history_lines(lines)
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.terminal.clear();
        let _ = restore_terminal_modes();
    }
}

impl ChatTerminal {
    fn new(mut backend: ChatBackend) -> Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        Ok(Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            viewport_area: Rect::new(0, cursor_pos.y, screen_size.width, 0),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            hidden_cursor: false,
        })
    }

    pub(crate) fn size(&self) -> Result<Size> {
        self.backend.size()
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    fn update_inline_viewport(&mut self, desired_height: u16) -> Result<()> {
        let screen_size = self.backend.size()?;
        let height = desired_height.clamp(1, screen_size.height.max(1));
        let mut next = Rect {
            height,
            width: screen_size.width,
            ..self.viewport_area
        };

        if screen_size != self.last_known_screen_size {
            self.last_known_screen_size = screen_size;
            self.invalidate_viewport();
        }

        if next.bottom() > screen_size.height {
            let scroll_by = next.bottom() - screen_size.height;
            if scroll_by > 0 {
                scroll_region_up(&mut self.backend, 0..next.top(), scroll_by)?;
            }
            next.y = screen_size.height.saturating_sub(next.height);
        }

        if next != self.viewport_area {
            if !self.viewport_area.is_empty() {
                let clear_y = self.viewport_area.y.min(next.y);
                self.backend
                    .set_cursor_position(Position { x: 0, y: clear_y })?;
                self.backend.clear_region(ClearType::AfterCursor)?;
            }
            self.set_viewport_area(next);
            self.invalidate_viewport();
        }

        Ok(())
    }

    fn insert_history_lines(&mut self, lines: Vec<Line<'static>>) -> Result<()> {
        if lines.is_empty() || self.viewport_area.is_empty() {
            return Ok(());
        }

        let screen_size = self.backend.size()?;
        let mut area = self.viewport_area;
        let wrap_width = area.width.max(1) as usize;
        let wrapped_rows = history_lines_height(&lines, area.width);
        if wrapped_rows == 0 {
            return Ok(());
        }

        {
            let writer = &mut self.backend;
            let cursor_top = if area.bottom() < screen_size.height {
                let space_below = screen_size.height - area.bottom();
                let scroll_amount = wrapped_rows.min(space_below);
                let top_1based = area.top() + 1;
                queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
                queue!(writer, MoveTo(0, area.top()))?;
                for _ in 0..scroll_amount {
                    queue!(writer, Print("\x1bM"))?;
                }
                queue!(writer, ResetScrollRegion)?;
                area.y += scroll_amount;
                area.top().saturating_sub(1)
            } else {
                area.top().saturating_sub(1)
            };

            queue!(writer, SetScrollRegion(1..area.top()))?;
            queue!(writer, MoveTo(0, cursor_top))?;
            for line in &lines {
                queue!(writer, Print("\r\n"))?;
                write_history_line(writer, line, wrap_width)?;
            }
            queue!(writer, ResetScrollRegion)?;
        }

        if area != self.viewport_area {
            self.set_viewport_area(area);
        }
        self.backend
            .set_cursor_position(self.last_known_cursor_pos)?;
        Backend::flush(&mut self.backend)?;
        self.invalidate_viewport();
        self.last_known_screen_size = screen_size;
        Ok(())
    }

    fn draw(&mut self, render: impl FnOnce(&mut TuiFrame<'_>)) -> Result<()> {
        let mut frame = TuiFrame {
            area: self.viewport_area,
            buffer: self.current_buffer_mut(),
            cursor_position: None,
        };
        render(&mut frame);
        let cursor_position = frame.cursor_position;

        self.flush()?;
        match cursor_position {
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
            None => self.hide_cursor()?,
        }
        self.swap_buffers();
        Backend::flush(&mut self.backend)
    }

    fn flush(&mut self) -> Result<()> {
        let previous = &self.buffers[1 - self.current];
        let current = &self.buffers[self.current];
        let updates = previous.diff(current);
        if let Some((x, y, _)) = updates.last() {
            self.last_known_cursor_pos = Position { x: *x, y: *y };
        }
        self.backend.draw(updates.into_iter())
    }

    fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    fn hide_cursor(&mut self) -> Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    fn set_cursor_position(&mut self, position: Position) -> Result<()> {
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.backend
            .set_cursor_position(self.viewport_area.as_position())?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.invalidate_viewport();
        Ok(())
    }
}

fn history_lines_height(lines: &[Line<'static>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>()
        .try_into()
        .unwrap_or(u16::MAX)
}

fn write_history_line<W: Write>(writer: &mut W, line: &Line<'_>, wrap_width: usize) -> Result<()> {
    let physical_rows = line.width().max(1).div_ceil(wrap_width) as u16;
    if physical_rows > 1 {
        queue!(writer, crossterm::cursor::SavePosition)?;
        for _ in 1..physical_rows {
            queue!(
                writer,
                crossterm::cursor::MoveDown(1),
                crossterm::cursor::MoveToColumn(0),
                Clear(crossterm::terminal::ClearType::UntilNewLine)
            )?;
        }
        queue!(writer, crossterm::cursor::RestorePosition)?;
    }

    queue!(writer, Clear(crossterm::terminal::ClearType::UntilNewLine))?;
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    for span in &line.spans {
        let style = span.style.patch(line.style);
        let span_modifier = style.add_modifier - style.sub_modifier;
        if span_modifier != modifier {
            queue_modifier_diff(writer, modifier, span_modifier)?;
            modifier = span_modifier;
        }
        let next_fg = style.fg.unwrap_or(Color::Reset);
        let next_bg = style.bg.unwrap_or(Color::Reset);
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
    queue!(
        writer,
        SetForegroundColor(CrosstermColor::Reset),
        SetBackgroundColor(CrosstermColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;
    Ok(())
}

fn queue_modifier_diff<W: Write>(writer: &mut W, from: Modifier, to: Modifier) -> Result<()> {
    use crossterm::style::Attribute as Attr;

    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attr::NoReverse))?;
    }
    if removed.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attr::NormalIntensity))?;
        if to.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(Attr::Dim))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attr::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attr::NoUnderline))?;
    }
    if removed.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attr::NormalIntensity))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attr::NotCrossedOut))?;
    }

    let added = to - from;
    if added.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attr::Reverse))?;
    }
    if added.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attr::Bold))?;
    }
    if added.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attr::Italic))?;
    }
    if added.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attr::Underlined))?;
    }
    if added.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attr::Dim))?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attr::CrossedOut))?;
    }

    Ok(())
}

fn scroll_region_up<W: Write>(
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
    queue!(writer, ResetScrollRegion)?;
    writer.flush()
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

impl Drop for ChatTerminal {
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
    }
}

fn enable_terminal_modes() -> Result<()> {
    enable_raw_mode()?;
    if let Err(err) = crossterm::execute!(stdout(), EnableBracketedPaste, Hide) {
        let _ = disable_raw_mode();
        return Err(err);
    }
    Ok(())
}

fn restore_terminal_modes() -> Result<()> {
    let mut first_error = crossterm::execute!(stdout(), DisableBracketedPaste, Show).err();
    if let Err(err) = disable_raw_mode() {
        first_error.get_or_insert(err);
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn set_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            let _ = restore_terminal_modes();
            hook(panic_info);
        }));
    });
}
