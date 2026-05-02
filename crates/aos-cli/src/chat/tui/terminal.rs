use std::io::{IsTerminal, Result, Stdout, stdout};
use std::panic;
use std::sync::Once;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use tokio::sync::broadcast;

use crate::chat::tui::frame::FrameRequester;

pub(crate) type ChatTerminal = Terminal<CrosstermBackend<Stdout>>;

static PANIC_HOOK: Once = Once::new();

pub(crate) struct Tui {
    pub(crate) terminal: ChatTerminal,
    frame_requester: FrameRequester,
    draw_tx: broadcast::Sender<()>,
    pending_history_lines: Vec<Line<'static>>,
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
        let terminal = init_terminal()?;

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

    pub(crate) fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> Result<()> {
        self.flush_pending_history_lines()?;
        self.terminal.draw(render).map(|_| ())
    }

    fn flush_pending_history_lines(&mut self) -> Result<()> {
        if self.pending_history_lines.is_empty() {
            return Ok(());
        }

        let width = self.terminal.size()?.width.max(1);
        let lines = std::mem::take(&mut self.pending_history_lines);
        let height = history_lines_height(&lines, width);
        if height == 0 {
            return Ok(());
        }

        self.terminal.insert_before(height, |buf| {
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .render(buf.area, buf);
        })
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

fn default_viewport_height() -> u16 {
    7
}

fn init_terminal() -> Result<ChatTerminal> {
    let viewport_height = default_viewport_height();
    let backend = CrosstermBackend::new(stdout());
    match Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(viewport_height),
        },
    ) {
        Ok(terminal) => Ok(terminal),
        Err(_) => Terminal::new(CrosstermBackend::new(stdout())),
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
    if let Err(err) = execute!(stdout(), EnableBracketedPaste, Hide) {
        let _ = disable_raw_mode();
        return Err(err);
    }
    Ok(())
}

fn restore_terminal_modes() -> Result<()> {
    let mut first_error = execute!(stdout(), DisableBracketedPaste, Show).err();
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
