use std::io::{Result, Stdout, Write};

use crossterm::cursor::SetCursorStyle;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType as CrosstermClearType};
use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect, Size};

use crate::chat::tui::insert_history::{InsertHistoryMode, scroll_region_up};

pub(crate) type ChatBackend = CrosstermBackend<Stdout>;

pub(crate) struct TuiFrame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor_position: Option<Position>,
    cursor_style: SetCursorStyle,
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

    pub(crate) fn set_cursor_style(&mut self, style: SetCursorStyle) {
        self.cursor_style = style;
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
    visible_history_rows: u16,
}

impl ChatTerminal {
    pub(crate) fn new(mut backend: ChatBackend) -> Result<Self> {
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
            visible_history_rows: 0,
        })
    }

    pub(crate) fn backend_mut(&mut self) -> &mut ChatBackend {
        &mut self.backend
    }

    pub(crate) fn size(&self) -> Result<Size> {
        self.backend.size()
    }

    pub(crate) fn viewport_area(&self) -> Rect {
        self.viewport_area
    }

    pub(crate) fn last_known_cursor_pos(&self) -> Position {
        self.last_known_cursor_pos
    }

    pub(crate) fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
        self.visible_history_rows = self.visible_history_rows.min(area.top());
    }

    #[allow(dead_code)]
    pub(crate) fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    pub(crate) fn note_history_rows_inserted(&mut self, inserted_rows: u16) {
        self.visible_history_rows = self
            .visible_history_rows
            .saturating_add(inserted_rows)
            .min(self.viewport_area.top());
    }

    pub(crate) fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    pub(crate) fn update_inline_viewport(
        &mut self,
        desired_height: u16,
        mode: InsertHistoryMode,
    ) -> Result<bool> {
        let screen_size = self.backend.size()?;
        let height = desired_height.clamp(1, screen_size.height.max(1));
        let mut next = Rect {
            height,
            width: screen_size.width,
            ..self.viewport_area
        };
        let mut needs_full_repaint = false;

        if screen_size != self.last_known_screen_size {
            self.last_known_screen_size = screen_size;
            self.invalidate_viewport();
        }

        if next.bottom() > screen_size.height {
            let scroll_by = next.bottom() - screen_size.height;
            if scroll_by > 0 {
                match mode {
                    InsertHistoryMode::Standard => {
                        scroll_region_up(&mut self.backend, 0..next.top(), scroll_by)?;
                    }
                    InsertHistoryMode::Zellij => {
                        scroll_zellij_expanded_viewport(&mut self.backend, screen_size, scroll_by)?;
                        needs_full_repaint = true;
                    }
                }
            }
            next.y = screen_size.height.saturating_sub(next.height);
        }

        if next != self.viewport_area {
            if !self.viewport_area.is_empty() {
                let clear_y = self.viewport_area.y.min(next.y);
                self.clear_after_position(Position { x: 0, y: clear_y })?;
            }
            self.set_viewport_area(next);
            self.invalidate_viewport();
        }

        Ok(needs_full_repaint)
    }

    pub(crate) fn update_inline_viewport_for_resize_reflow(
        &mut self,
        desired_height: u16,
        mode: InsertHistoryMode,
    ) -> Result<bool> {
        let screen_size = self.backend.size()?;
        let height = desired_height.clamp(1, screen_size.height.max(1));
        let terminal_height_shrank = screen_size.height < self.last_known_screen_size.height;
        let terminal_height_grew = screen_size.height > self.last_known_screen_size.height;
        let viewport_was_bottom_aligned =
            self.viewport_area.bottom() == self.last_known_screen_size.height;
        let mut next = Rect {
            x: 0,
            y: self.viewport_area.y,
            width: screen_size.width,
            height,
        };
        let mut needs_full_repaint = false;

        if next.bottom() > screen_size.height {
            let scroll_by = next.bottom() - screen_size.height;
            if !terminal_height_shrank {
                match mode {
                    InsertHistoryMode::Standard => {
                        scroll_region_up(&mut self.backend, 0..next.top(), scroll_by)?;
                    }
                    InsertHistoryMode::Zellij => {
                        scroll_zellij_expanded_viewport(&mut self.backend, screen_size, scroll_by)?;
                        needs_full_repaint = true;
                    }
                }
            }
            next.y = screen_size.height.saturating_sub(next.height);
        } else if terminal_height_grew && viewport_was_bottom_aligned {
            next.y = screen_size.height.saturating_sub(next.height);
        }

        if next != self.viewport_area {
            let clear_position = Position::new(0, self.viewport_area.y.min(next.y));
            self.set_viewport_area(next);
            self.clear_after_position(clear_position)?;
            needs_full_repaint = true;
        }

        self.last_known_screen_size = screen_size;
        Ok(needs_full_repaint)
    }

    pub(crate) fn reset_inline_viewport_for_reflow(&mut self, desired_height: u16) -> Result<()> {
        let screen_size = self.backend.size()?;
        let height = desired_height.clamp(1, screen_size.height.max(1));
        self.set_viewport_area(Rect::new(0, 0, screen_size.width, height));
        self.last_known_screen_size = screen_size;
        self.visible_history_rows = 0;
        self.clear_scrollback_and_visible_screen_ansi()?;
        Ok(())
    }

    pub(crate) fn draw(&mut self, render: impl FnOnce(&mut TuiFrame<'_>)) -> Result<()> {
        let area = self.viewport_area;
        let mut frame = TuiFrame {
            area,
            buffer: self.current_buffer_mut(),
            cursor_position: None,
            cursor_style: SetCursorStyle::DefaultUserShape,
        };
        render(&mut frame);
        let cursor_position = frame.cursor_position;
        let cursor_style = frame.cursor_style;

        self.flush()?;
        match cursor_position {
            Some(position) => {
                self.set_cursor_style(cursor_style)?;
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
            None => self.hide_cursor()?,
        }
        self.swap_buffers();
        Write::flush(&mut self.backend)
    }

    pub(crate) fn clear(&mut self) -> Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    pub(crate) fn clear_visible_screen(&mut self) -> Result<()> {
        let home = Position { x: 0, y: 0 };
        self.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.set_cursor_position(home)?;
        self.visible_history_rows = 0;
        self.invalidate_viewport();
        Write::flush(&mut self.backend)
    }

    pub(crate) fn clear_scrollback_and_visible_screen_ansi(&mut self) -> Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        Write::flush(&mut self.backend)?;
        self.last_known_cursor_pos = Position { x: 0, y: 0 };
        self.visible_history_rows = 0;
        self.invalidate_viewport();
        Ok(())
    }

    pub(crate) fn clear_after_position(&mut self, position: Position) -> Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.invalidate_viewport();
        Ok(())
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
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

    fn set_cursor_style(&mut self, style: SetCursorStyle) -> Result<()> {
        queue!(self.backend, style)
    }

    fn reset_cursor_style(&mut self) -> Result<()> {
        self.set_cursor_style(SetCursorStyle::DefaultUserShape)
    }

    fn set_cursor_position(&mut self, position: Position) -> Result<()> {
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }
}

impl Drop for ChatTerminal {
    fn drop(&mut self) {
        let _ = self.reset_cursor_style();
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
    }
}

fn scroll_zellij_expanded_viewport<W: Write>(
    writer: &mut W,
    screen_size: Size,
    scroll_by: u16,
) -> Result<()> {
    queue!(
        writer,
        crossterm::cursor::MoveTo(0, screen_size.height.saturating_sub(1))
    )?;
    for _ in 0..scroll_by {
        queue!(writer, crossterm::style::Print("\n"))?;
    }
    Ok(())
}

#[allow(dead_code)]
fn clear_current_line<W: Write>(writer: &mut W) -> Result<()> {
    queue!(writer, Clear(CrosstermClearType::CurrentLine))
}
