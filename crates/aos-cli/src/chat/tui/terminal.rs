use std::io::{IsTerminal, Result, Write, stdout};
use std::panic;
use std::sync::Once;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::queue;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::text::Line;
use tokio::sync::broadcast;

use crate::chat::tui::custom_terminal::{ChatTerminal, TuiFrame};
use crate::chat::tui::frame::FrameRequester;
use crate::chat::tui::insert_history::{InsertHistoryMode, insert_history_lines_with_mode};

static PANIC_HOOK: Once = Once::new();

pub(crate) struct Tui {
    pub(crate) terminal: ChatTerminal,
    frame_requester: FrameRequester,
    draw_tx: broadcast::Sender<()>,
    pending_history_lines: Vec<Line<'static>>,
    insert_history_mode: InsertHistoryMode,
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
        let insert_history_mode = InsertHistoryMode::new(std::env::var_os("ZELLIJ").is_some());

        Ok(Self {
            terminal,
            frame_requester,
            draw_tx,
            pending_history_lines: Vec::new(),
            insert_history_mode,
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
        self.synchronized_terminal_update(|terminal| terminal.clear_visible_screen())
    }

    pub(crate) fn draw(
        &mut self,
        desired_height: u16,
        render: impl FnOnce(&mut TuiFrame<'_>),
    ) -> Result<()> {
        let mode = self.insert_history_mode;
        let history_lines = std::mem::take(&mut self.pending_history_lines);
        self.synchronized_terminal_update(move |terminal| {
            let mut needs_full_repaint = terminal.update_inline_viewport(desired_height, mode)?;
            if !history_lines.is_empty() {
                terminal.clear()?;
            }
            needs_full_repaint |= insert_history_lines_with_mode(terminal, history_lines, mode)?;
            if needs_full_repaint {
                terminal.invalidate_viewport();
            }
            terminal.draw(render)
        })
    }

    pub(crate) fn draw_with_resize_reflow(
        &mut self,
        desired_height: u16,
        history_lines: Vec<Line<'static>>,
        render: impl FnOnce(&mut TuiFrame<'_>),
    ) -> Result<()> {
        self.pending_history_lines.clear();
        let mode = self.insert_history_mode;
        self.synchronized_terminal_update(move |terminal| {
            let mut needs_full_repaint =
                terminal.update_inline_viewport_for_resize_reflow(desired_height, mode)?;
            terminal.reset_inline_viewport_for_reflow(desired_height)?;
            needs_full_repaint |= insert_history_lines_with_mode(terminal, history_lines, mode)?;
            if needs_full_repaint {
                terminal.invalidate_viewport();
            }
            terminal.draw(render)
        })
    }

    fn synchronized_terminal_update(
        &mut self,
        update: impl FnOnce(&mut ChatTerminal) -> Result<()>,
    ) -> Result<()> {
        queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;
        let update_result = update(&mut self.terminal);
        let end_result = queue!(self.terminal.backend_mut(), EndSynchronizedUpdate)
            .and_then(|_| Write::flush(self.terminal.backend_mut()));
        update_result.and(end_result)
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.terminal.clear();
        let _ = restore_terminal_modes();
    }
}

fn enable_terminal_modes() -> Result<()> {
    enable_raw_mode()?;
    if let Err(err) = crossterm::execute!(
        stdout(),
        EnableBracketedPaste,
        crossterm::cursor::Hide,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    ) {
        let _ = disable_raw_mode();
        return Err(err);
    }
    Ok(())
}

fn restore_terminal_modes() -> Result<()> {
    let mut first_error = crossterm::execute!(
        stdout(),
        DisableBracketedPaste,
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        crossterm::cursor::Show
    )
    .err();
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
