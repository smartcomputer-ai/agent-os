use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use tokio::sync::mpsc;

use crate::chat::client::ChatControlClient;
use crate::chat::driver::{ChatSessionDriver, ChatSessionDriverOptions};
use crate::chat::protocol::{
    ChatCommand, ChatDelta, ChatDraftOverrideMask, ChatDraftSettings, ChatErrorView, ChatEvent,
    ChatMessageView,
};
use crate::chat::tui::app_event::UiEvent;
use crate::chat::tui::app_event_sender::AppEventSender;
use crate::chat::tui::bottom_pane::list_selection::PickerSelection;
use crate::chat::tui::bottom_pane::{BottomPaneAction, BottomPaneState};
use crate::chat::tui::custom_terminal::TuiFrame;
use crate::chat::tui::frame::FrameRequester;
use crate::chat::tui::slash::{SlashCommand, SlashEffort, SlashMaxTokens, parse_slash_command};
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
                    if app.take_terminal_clear_requested() {
                        tui.clear_viewport()?;
                    }
                    let width = tui.terminal.size()?.width;
                    let viewport_height = app.desired_viewport_height(width);
                    if app.take_resize_reflow_requested(width) {
                        let history_lines = app.reflow_history_lines(width);
                        tui.draw_with_resize_reflow(viewport_height, history_lines, |frame| {
                            app.render_tui_frame(frame)
                        })?;
                    } else {
                        tui.insert_history_lines(app.drain_pending_history_lines(width));
                        tui.draw(viewport_height, |frame| app.render_tui_frame(frame))?;
                    }
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
    #[allow(dead_code)]
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
    terminal_clear_requested: bool,
    resize_reflow_requested: bool,
    last_render_width: Option<u16>,
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
            terminal_clear_requested: false,
            resize_reflow_requested: false,
            last_render_width: None,
        }
    }

    fn handle_ui_event(&mut self, event: UiEvent, frame_requester: &FrameRequester) -> bool {
        match event {
            UiEvent::ExitRequested => return true,
            UiEvent::Chat(event) => {
                match &event {
                    ChatEvent::SessionsListed { sessions, .. } => {
                        self.bottom_pane.open_session_picker(sessions);
                    }
                    ChatEvent::HistoryReset { session_id } => {
                        self.options.session_id = session_id.clone();
                        self.terminal_clear_requested = true;
                    }
                    ChatEvent::SessionSelected(summary) => {
                        self.options.session_id = summary.session_id.clone();
                    }
                    _ => {}
                }
                self.bottom_pane.apply_chat_event(&event);
                self.transcript.apply_chat_event(event);
                frame_requester.schedule_frame();
            }
            UiEvent::ComposerChanged => {
                frame_requester.schedule_frame();
            }
            UiEvent::Resize { cols, rows } => {
                let _ = (cols, rows);
                self.resize_reflow_requested = true;
                frame_requester.schedule_frame();
            }
        }
        false
    }

    fn handle_terminal_event(&mut self, event: Event, frame_requester: &FrameRequester) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match self.bottom_pane.handle_key(key) {
                    BottomPaneAction::None => {}
                    BottomPaneAction::Changed => {
                        self.app_event_tx.send(UiEvent::ComposerChanged);
                    }
                    BottomPaneAction::Submit(text) => self.submit_local_text(text),
                    BottomPaneAction::ExitRequested => self.app_event_tx.exit(),
                    BottomPaneAction::PickerSelected(selection) => {
                        self.apply_picker_selection(selection);
                    }
                    BottomPaneAction::PickerRejected(reason) => {
                        self.local_error(reason);
                    }
                    BottomPaneAction::SlashCommandSelected(command) => {
                        self.apply_slash_command(command.command_without_args());
                    }
                }
            }
            Event::Paste(text) => {
                self.bottom_pane.insert_paste(&text);
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            Event::Resize(cols, rows) => {
                self.app_event_tx.send(UiEvent::Resize { cols, rows });
            }
            _ => {}
        }
        frame_requester.schedule_frame();
    }

    #[allow(dead_code)]
    pub(crate) fn render(&self, frame: &mut Frame<'_>) {
        if let Some(position) = self.render_area(frame.area(), frame.buffer_mut()) {
            frame.set_cursor_position(position);
        }
    }

    pub(crate) fn render_tui_frame(&self, frame: &mut TuiFrame<'_>) {
        if let Some(position) = self.render_area(frame.area(), frame.buffer_mut()) {
            frame.set_cursor_style(self.bottom_pane.cursor_style());
            frame.set_cursor_position(position);
        }
    }

    fn render_area(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let bottom_height = self.bottom_pane.desired_height().min(area.height);
        let spacer_height = u16::from(area.height > bottom_height);
        let transcript_height = area.height.saturating_sub(bottom_height + spacer_height);
        let transcript_area = Rect {
            height: transcript_height,
            ..area
        };
        let bottom_area = Rect {
            y: area
                .y
                .saturating_add(transcript_height)
                .saturating_add(spacer_height),
            height: bottom_height,
            ..area
        };

        if transcript_area.height > 0 {
            self.transcript.render(transcript_area, buf);
        }
        self.bottom_pane.render(bottom_area, buf);
        self.bottom_pane.cursor_position(bottom_area)
    }

    fn drain_pending_history_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        self.transcript.drain_pending_history_lines(width)
    }

    fn reflow_history_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        self.transcript.reflow_history_lines(width)
    }

    fn desired_viewport_height(&self, width: u16) -> u16 {
        self.transcript
            .desired_height(width)
            .saturating_add(self.bottom_pane.desired_height())
            .saturating_add(1)
            .max(1)
    }

    fn take_terminal_clear_requested(&mut self) -> bool {
        let requested = self.terminal_clear_requested;
        self.terminal_clear_requested = false;
        requested
    }

    fn take_resize_reflow_requested(&mut self, width: u16) -> bool {
        let width_changed = self.last_render_width.is_some_and(|last| last != width);
        self.last_render_width = Some(width);
        let requested = self.resize_reflow_requested || width_changed;
        self.resize_reflow_requested = false;
        requested
    }

    fn submit_local_text(&mut self, text: String) {
        let trimmed = text.trim();
        match parse_slash_command(trimmed) {
            Ok(Some(command)) => {
                self.apply_slash_command(command);
                return;
            }
            Ok(None) => {}
            Err(error) => {
                self.local_error(format!("{error}"));
                return;
            }
        }
        self.local_user_message(text.clone());
        self.send_chat_command(ChatCommand::SubmitUserMessage { text });
    }

    fn apply_slash_command(&mut self, command: SlashCommand) {
        match command {
            SlashCommand::Help => self.local_notice(command_help()),
            SlashCommand::NewSession => self.send_chat_command(ChatCommand::NewSession),
            SlashCommand::Sessions => self.send_chat_command(ChatCommand::ListSessions),
            SlashCommand::Resume(Some(session_id)) => {
                self.send_chat_command(ChatCommand::SwitchSession { session_id });
            }
            SlashCommand::Resume(None) => self.send_chat_command(ChatCommand::ListSessions),
            SlashCommand::Quit => self.app_event_tx.exit(),
            SlashCommand::Model(Some(model)) => {
                self.send_chat_command(ChatCommand::SetDraftModel { model });
            }
            SlashCommand::Model(None) => {
                self.bottom_pane.open_model_picker();
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            SlashCommand::Provider(Some(provider)) => {
                self.send_chat_command(ChatCommand::SetDraftProvider { provider });
            }
            SlashCommand::Provider(None) => {
                self.bottom_pane.open_provider_picker();
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            SlashCommand::Effort(SlashEffort::Pick) => {
                self.bottom_pane.open_effort_picker();
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            SlashCommand::Effort(SlashEffort::Set(effort)) => {
                self.send_chat_command(ChatCommand::SetDraftReasoningEffort { effort });
            }
            SlashCommand::MaxTokens(SlashMaxTokens::Pick) => {
                self.bottom_pane.open_max_tokens_picker();
                self.app_event_tx.send(UiEvent::ComposerChanged);
            }
            SlashCommand::MaxTokens(SlashMaxTokens::Set(max_tokens)) => {
                self.send_chat_command(ChatCommand::SetDraftMaxTokens { max_tokens });
            }
        }
    }

    fn apply_picker_selection(&mut self, selection: PickerSelection) {
        match selection {
            PickerSelection::Model(model) => {
                self.send_chat_command(ChatCommand::SetDraftModel { model });
            }
            PickerSelection::Provider(provider) => {
                self.send_chat_command(ChatCommand::SetDraftProvider { provider });
            }
            PickerSelection::Effort(effort) => {
                self.send_chat_command(ChatCommand::SetDraftReasoningEffort { effort });
            }
            PickerSelection::MaxTokens(max_tokens) => {
                self.send_chat_command(ChatCommand::SetDraftMaxTokens { max_tokens });
            }
            PickerSelection::SlashCommand(command) => {
                self.apply_slash_command(command.command_without_args());
            }
            PickerSelection::Session(session_id) => {
                self.send_chat_command(ChatCommand::SwitchSession { session_id });
            }
        }
    }

    fn send_chat_command(&mut self, command: ChatCommand) {
        if let Err(error) = self.command_tx.send(command) {
            self.app_event_tx.chat(ChatEvent::Error(ChatErrorView {
                message: format!("chat driver is not available: {error}"),
                action: None,
            }));
        }
    }

    fn local_error(&mut self, content: impl Into<String>) {
        self.app_event_tx.chat(ChatEvent::Error(ChatErrorView {
            message: content.into(),
            action: None,
        }));
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
}

fn command_help() -> &'static str {
    "commands: /new, /sessions, /resume, /model, /provider, /effort, /max-tokens, /help, /quit"
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_agent::{ReasoningEffort, RunLifecycle};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::sync::mpsc;

    use crate::chat::protocol::{
        ChatConnectionInfo, ChatRunView, ChatSessionSummary, ChatSettingsView, ChatStatus,
        ChatTurn, run_status,
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
        let history = app.drain_pending_history_lines(80);
        let history_text = history
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history_text.contains("Hello from AOS Chat."));
        assert!(history_text.contains("Simulated assistant response."));

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let rendered = format!("{}", terminal.backend());
        let expected = snapshot_lines([
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad(""),
            pad("> "),
            pad(""),
            pad("P3a shell  gpt-5.3-codex  effort none"),
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

    #[test]
    fn slash_effort_with_argument_sends_setting_command() {
        let (tx, _rx) = mpsc::unbounded_channel();
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

        app.submit_local_text("/effort high".into());

        assert_eq!(
            command_rx.try_recv().expect("setting command"),
            ChatCommand::SetDraftReasoningEffort {
                effort: Some(ReasoningEffort::High)
            }
        );
    }

    #[test]
    fn slash_effort_opens_picker_and_enter_confirms_selection() {
        let (tx, _rx) = mpsc::unbounded_channel();
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
        for event in fixture_events(&app.options) {
            app.handle_ui_event(UiEvent::Chat(event), &FrameRequester::test_dummy());
        }

        app.submit_local_text("/effort".into());
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            &FrameRequester::test_dummy(),
        );
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &FrameRequester::test_dummy(),
        );

        assert_eq!(
            command_rx.try_recv().expect("picker command"),
            ChatCommand::SetDraftReasoningEffort {
                effort: Some(ReasoningEffort::Low)
            }
        );
    }

    #[test]
    fn slash_prefix_filters_commands_and_enter_opens_selected_picker() {
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

        for ch in ['/', 'm', 'o'] {
            app.handle_terminal_event(
                Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                &FrameRequester::test_dummy(),
            );
        }

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let rendered = format!("{}", terminal.backend());
        assert!(rendered.contains("/model"));
        assert!(!rendered.contains("/provider"));

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &FrameRequester::test_dummy(),
        );

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(format!("{}", terminal.backend()).contains("Select model"));
    }

    #[test]
    fn sessions_event_opens_picker_and_selection_switches_session() {
        let (tx, _rx) = mpsc::unbounded_channel();
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
        let target_session = "018f2a66-31cc-7b25-a4f7-37e3310fdc6c".to_string();

        app.handle_ui_event(
            UiEvent::Chat(ChatEvent::SessionsListed {
                world_id: app.options.world_id.clone(),
                sessions: vec![
                    ChatSessionSummary {
                        session_id: app.options.session_id.clone(),
                        status: None,
                        lifecycle: None,
                        updated_at_ns: Some(2),
                        run_count: 1,
                        provider: Some("openai-responses".into()),
                        model: Some("gpt-5.3-codex".into()),
                        active_run: None,
                    },
                    ChatSessionSummary {
                        session_id: target_session.clone(),
                        status: None,
                        lifecycle: None,
                        updated_at_ns: Some(1),
                        run_count: 0,
                        provider: None,
                        model: None,
                        active_run: None,
                    },
                ],
            }),
            &FrameRequester::test_dummy(),
        );

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            &FrameRequester::test_dummy(),
        );
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &FrameRequester::test_dummy(),
        );

        assert_eq!(
            command_rx.try_recv().expect("switch command"),
            ChatCommand::SwitchSession {
                session_id: target_session
            }
        );
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
