use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use tokio::sync::mpsc;

use crate::chat::client::ChatControlClient;
use crate::chat::driver::{ChatSessionDriver, ChatSessionDriverOptions};
use crate::chat::protocol::{
    ChatCommand, ChatDelta, ChatDraftOverrideMask, ChatDraftSettings, ChatErrorView, ChatEvent,
    ChatMessageView,
};
use crate::chat::tui::app_event::UiEvent;
use crate::chat::tui::app_event_sender::AppEventSender;
use crate::chat::tui::bottom_pane::BottomPaneState;
use crate::chat::tui::bottom_pane::composer::ComposerAction;
use crate::chat::tui::frame::FrameRequester;
use crate::chat::tui::terminal::Tui;
use crate::chat::tui::transcript::TranscriptState;

#[derive(Clone)]
pub(crate) struct ChatTuiShellOptions {
    pub(crate) client: ChatControlClient,
    pub(crate) session_id: String,
    pub(crate) draft_settings: ChatDraftSettings,
    pub(crate) draft_overrides: ChatDraftOverrideMask,
    pub(crate) from: Option<u64>,
}

pub(crate) async fn run_shell(options: ChatTuiShellOptions) -> Result<()> {
    let view_options = ChatTuiViewOptions {
        world_id: options.client.world_id().to_string(),
        session_id: options.session_id.clone(),
    };
    let (driver, initial_events) = ChatSessionDriver::open(
        options.client,
        ChatSessionDriverOptions {
            session_id: options.session_id,
            draft_settings: options.draft_settings,
            draft_overrides: options.draft_overrides,
            from: options.from,
        },
    )
    .await?;

    let mut tui = Tui::init().context("initialize chat TUI")?;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let app_event_tx = AppEventSender::new(event_tx);
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    spawn_driver_task(driver, command_rx, app_event_tx.clone());
    let mut app = ChatTuiApp::new(view_options, app_event_tx.clone(), command_tx);
    let mut terminal_events = EventStream::new();
    let mut draw_rx = tui.draw_receiver();
    let frame_requester = tui.frame_requester();
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    for event in initial_events {
        app_event_tx.chat(event);
    }
    frame_requester.schedule_frame();

    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                if app.handle_ui_event(event, &frame_requester) {
                    break;
                }
            }
            draw = draw_rx.recv() => {
                if draw.is_ok() {
                    tui.terminal.draw(|frame| app.render(frame))?;
                }
            }
            event = terminal_events.next() => {
                let Some(event) = event else {
                    break;
                };
                app.handle_terminal_event(event?, &frame_requester);
            }
            signal = &mut ctrl_c => {
                signal.context("listen for Ctrl-C")?;
                break;
            }
        }
    }

    Ok(())
}

fn spawn_driver_task(
    mut driver: ChatSessionDriver,
    mut command_rx: mpsc::UnboundedReceiver<ChatCommand>,
    app_event_tx: AppEventSender,
) {
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            let should_follow = should_follow_after(&command);
            match driver.handle_command(command).await {
                Ok(events) => {
                    for event in events {
                        app_event_tx.chat(event);
                    }
                }
                Err(error) => {
                    app_event_tx.chat(driver_error(error));
                    continue;
                }
            }
            if should_follow
                && let Err(error) = driver
                    .follow_until_quiescent(Duration::from_secs(300), |event| {
                        app_event_tx.chat(event);
                    })
                    .await
            {
                app_event_tx.chat(driver_error(error));
            }
        }
    });
}

fn should_follow_after(command: &ChatCommand) -> bool {
    matches!(
        command,
        ChatCommand::SubmitUserMessage { .. }
            | ChatCommand::SteerRun { .. }
            | ChatCommand::InterruptRun { .. }
            | ChatCommand::ResumeSession
    )
}

fn driver_error(error: anyhow::Error) -> ChatEvent {
    ChatEvent::Error(ChatErrorView {
        message: error.to_string(),
        action: None,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ChatTuiViewOptions {
    pub(crate) world_id: String,
    pub(crate) session_id: String,
}

pub(crate) struct ChatTuiApp {
    options: ChatTuiViewOptions,
    transcript: TranscriptState,
    bottom_pane: BottomPaneState,
    app_event_tx: AppEventSender,
    command_tx: mpsc::UnboundedSender<ChatCommand>,
    next_local_message: u64,
}

impl ChatTuiApp {
    pub(crate) fn new(
        options: ChatTuiViewOptions,
        app_event_tx: AppEventSender,
        command_tx: mpsc::UnboundedSender<ChatCommand>,
    ) -> Self {
        Self {
            options,
            transcript: TranscriptState::default(),
            bottom_pane: BottomPaneState::default(),
            app_event_tx,
            command_tx,
            next_local_message: 0,
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent, frame_requester: &FrameRequester) -> bool {
        match event {
            UiEvent::ExitRequested => return true,
            UiEvent::Chat(event) => {
                self.bottom_pane.apply_chat_event(&event);
                self.transcript.apply_chat_event(event);
                frame_requester.schedule_frame();
            }
            UiEvent::ComposerChanged => {
                frame_requester.schedule_frame();
            }
            UiEvent::Resize { cols, rows } => {
                let _ = (cols, rows);
                frame_requester.schedule_frame();
            }
        }
        false
    }

    fn handle_terminal_event(&mut self, event: Event, frame_requester: &FrameRequester) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match self.bottom_pane.composer_mut().handle_key(key) {
                    ComposerAction::None => {}
                    ComposerAction::Changed => {
                        self.app_event_tx.send(UiEvent::ComposerChanged);
                    }
                    ComposerAction::Submit(text) => self.submit_local_text(text),
                    ComposerAction::ExitRequested => self.app_event_tx.exit(),
                }
            }
            Event::Paste(text) => {
                self.bottom_pane.composer_mut().insert_str(&text);
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            Event::Resize(cols, rows) => {
                self.app_event_tx.send(UiEvent::Resize { cols, rows });
            }
            _ => {}
        }
        frame_requester.schedule_frame();
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let bottom_height = self.bottom_pane.desired_height().min(area.height);
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(bottom_height),
        ])
        .split(area);

        Paragraph::new(self.title_line()).render(chunks[0], frame.buffer_mut());
        self.transcript.render(chunks[1], frame.buffer_mut());
        self.bottom_pane.render(chunks[2], frame.buffer_mut());
        if let Some(position) = self.bottom_pane.cursor_position(chunks[2]) {
            frame.set_cursor_position(position);
        }
    }

    fn submit_local_text(&mut self, text: String) {
        let trimmed = text.trim();
        if matches!(trimmed, "/quit" | "/exit") {
            self.app_event_tx.exit();
            return;
        }
        if trimmed == "/help" {
            self.local_notice(
                "P3 shell commands: /help, /quit. Model and effort pickers are the next TUI slice.",
            );
            return;
        }
        self.local_user_message(text.clone());
        if let Err(error) = self
            .command_tx
            .send(ChatCommand::SubmitUserMessage { text })
        {
            self.app_event_tx.chat(ChatEvent::Error(ChatErrorView {
                message: format!("chat driver is not available: {error}"),
                action: None,
            }));
        }
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.next_local_message = self.next_local_message.saturating_add(1);
        format!("{prefix}:{}", self.next_local_message)
    }

    fn local_notice(&mut self, content: impl Into<String>) {
        let id = self.next_id("local-notice");
        self.app_event_tx
            .chat(ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
                session_id: self.options.session_id.clone(),
                message: ChatMessageView {
                    id,
                    role: "system".into(),
                    content: content.into(),
                    ref_: None,
                },
            }));
    }

    fn local_user_message(&mut self, content: impl Into<String>) {
        let id = self.next_id("local-user");
        self.app_event_tx
            .chat(ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
                session_id: self.options.session_id.clone(),
                message: ChatMessageView {
                    id,
                    role: "user_pending".into(),
                    content: content.into(),
                    ref_: None,
                },
            }));
    }

    fn title_line(&self) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                "AOS Chat",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  world "),
            Span::styled(
                short(&self.options.world_id),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  session "),
            Span::styled(
                short(&self.options.session_id),
                Style::default().fg(Color::Cyan),
            ),
        ])
    }
}

fn short(value: &str) -> String {
    value.get(..8).unwrap_or(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_agent::RunLifecycle;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::sync::mpsc;

    use crate::chat::protocol::{
        ChatConnectionInfo, ChatRunView, ChatSettingsView, ChatStatus, ChatTurn, run_status,
    };

    #[test]
    fn shell_renders_fake_cells() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        let mut app = ChatTuiApp::new(
            ChatTuiViewOptions {
                world_id: "018f2a66-31cc-7b25-a4f7-37e3310fdc6a".into(),
                session_id: "018f2a66-31cc-7b25-a4f7-37e3310fdc6b".into(),
            },
            app_event_tx,
            command_tx,
        );
        for event in fixture_events(&app.options) {
            app.handle_ui_event(UiEvent::Chat(event), &FrameRequester::test_dummy());
        }

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let rendered = format!("{}", terminal.backend());
        let rule = "\u{2500}".repeat(80);
        let expected = snapshot_lines([
            pad("AOS Chat  world 018f2a66  session 018f2a66"),
            pad("user"),
            pad("  Hello from AOS Chat."),
            pad(""),
            pad("assistant"),
            pad("  Simulated assistant response. Live engine wiring comes next."),
            pad(""),
            pad("run 0 Running gpt-5.3-codex"),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            rule.clone(),
            rule,
            pad("P3a shell  gpt-5.3-codex  effort none"),
            pad("> "),
        ]);

        assert_eq!(rendered, expected);
    }

    #[test]
    fn submit_local_text_echoes_user_message_immediately() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let app_event_tx = AppEventSender::new(tx);
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let mut app = ChatTuiApp::new(
            ChatTuiViewOptions {
                world_id: "018f2a66-31cc-7b25-a4f7-37e3310fdc6a".into(),
                session_id: "018f2a66-31cc-7b25-a4f7-37e3310fdc6b".into(),
            },
            app_event_tx,
            command_tx,
        );

        app.submit_local_text("hello now".into());

        let event = rx.try_recv().expect("local echo event");
        app.handle_ui_event(event, &FrameRequester::test_dummy());
        let command = command_rx.try_recv().expect("submit command");
        assert_eq!(
            command,
            ChatCommand::SubmitUserMessage {
                text: "hello now".into()
            }
        );

        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(format!("{}", terminal.backend()).contains("hello now"));
    }

    fn pad(line: &str) -> String {
        format!("{line:<80}")
    }

    fn snapshot_lines(lines: impl IntoIterator<Item = String>) -> String {
        lines
            .into_iter()
            .map(|line| format!("\"{line}\"\n"))
            .collect()
    }

    fn fixture_events(options: &ChatTuiViewOptions) -> Vec<ChatEvent> {
        let settings = ChatSettingsView {
            provider: "openai-responses".into(),
            model: "gpt-5.3-codex".into(),
            reasoning_effort: None,
            max_tokens: None,
            provider_editable: true,
            model_editable: true,
            effort_editable: true,
            max_tokens_editable: true,
        };
        vec![
            ChatEvent::Connected(ChatConnectionInfo {
                world_id: options.world_id.clone(),
                session_id: options.session_id.clone(),
                journal_next_from: None,
                settings: settings.clone(),
            }),
            ChatEvent::TranscriptDelta(ChatDelta::ReplaceTurns {
                session_id: options.session_id.clone(),
                turns: vec![ChatTurn {
                    turn_id: "p3a-shell".into(),
                    user: Some(ChatMessageView {
                        id: "p3a-user".into(),
                        role: "user".into(),
                        content: "Hello from AOS Chat.".into(),
                        ref_: None,
                    }),
                    assistant: Some(ChatMessageView {
                        id: "p3a-assistant".into(),
                        role: "assistant".into(),
                        content: "Simulated assistant response. Live engine wiring comes next."
                            .into(),
                        ref_: None,
                    }),
                    run: Some(ChatRunView {
                        id: "p3a-run".into(),
                        run_seq: 0,
                        lifecycle: RunLifecycle::Running,
                        status: run_status(RunLifecycle::Running),
                        provider: "openai-responses".into(),
                        model: "gpt-5.3-codex".into(),
                        reasoning_effort: None,
                        input_refs: Vec::new(),
                        output_ref: None,
                        started_at_ns: 0,
                        updated_at_ns: 0,
                    }),
                    tool_chains: Vec::new(),
                }],
            }),
            ChatEvent::StatusChanged(ChatStatus {
                session_id: options.session_id.clone(),
                status: "P3a shell".into(),
                detail: None,
                settings,
            }),
        ]
    }
}
