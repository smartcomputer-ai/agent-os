use std::io::{IsTerminal, Result, Stdout, stdout};
use std::panic;
use std::sync::Once;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::broadcast;

use crate::chat::tui::frame::FrameRequester;

pub(crate) type ChatTerminal = Terminal<CrosstermBackend<Stdout>>;

static PANIC_HOOK: Once = Once::new();

pub(crate) struct Tui {
    pub(crate) terminal: ChatTerminal,
    frame_requester: FrameRequester,
    draw_tx: broadcast::Sender<()>,
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
        let backend = CrosstermBackend::new(stdout());
        let viewport_height = crossterm::terminal::size()
            .map(|(_, rows)| rows.clamp(8, 18))
            .unwrap_or(12);
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        Ok(Self {
            terminal,
            frame_requester,
            draw_tx,
        })
    }

    pub(crate) fn frame_requester(&self) -> FrameRequester {
        self.frame_requester.clone()
    }

    pub(crate) fn draw_receiver(&self) -> broadcast::Receiver<()> {
        self.draw_tx.subscribe()
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
