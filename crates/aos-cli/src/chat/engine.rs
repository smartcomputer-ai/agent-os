use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_agent::{
    ReasoningEffort, SessionConfig, SessionId, SessionInput, SessionInputKind, SessionLifecycle,
    SessionState,
};
use serde_json::json;
use tokio::time::sleep;

use crate::chat::blob_cache::BlobCache;
use crate::chat::client::{ChatControlClient, bare_summary, summary_from_state};
use crate::chat::projection::ChatProjection;
use crate::chat::protocol::{
    ChatCommand, ChatConnectionInfo, ChatDelta, ChatDraftOverrideMask, ChatDraftSettings,
    ChatErrorView, ChatEvent, ChatMessageView, ChatSettingsView, ChatStatus, session_active,
};
use crate::chat::session::validate_session_id;
use crate::chat::sse::JournalSseEvent;

#[derive(Debug, Clone)]
pub(crate) struct ChatEngineOptions {
    pub session_id: String,
    pub draft_settings: ChatDraftSettings,
    pub draft_overrides: ChatDraftOverrideMask,
    pub from: Option<u64>,
}

pub(crate) struct ChatEngine {
    client: ChatControlClient,
    projection: ChatProjection,
    blob_cache: BlobCache,
    draft_settings: ChatDraftSettings,
    draft_overrides: ChatDraftOverrideMask,
    base_session_config: SessionConfig,
    observed_clock: u64,
}

impl ChatEngine {
    pub(crate) async fn open(
        client: ChatControlClient,
        options: ChatEngineOptions,
    ) -> Result<(Self, Vec<ChatEvent>)> {
        let session_id = validate_session_id(&options.session_id)?;
        let mut engine = Self {
            projection: ChatProjection::new(client.world_id().to_string(), session_id.clone()),
            client,
            blob_cache: BlobCache::default(),
            draft_settings: options.draft_settings,
            draft_overrides: options.draft_overrides,
            base_session_config: SessionConfig::default(),
            observed_clock: 0,
        };
        let mut events = vec![ChatEvent::Connected(ChatConnectionInfo {
            world_id: engine.client.world_id().to_string(),
            session_id: session_id.clone(),
            journal_next_from: options.from,
            settings: engine.settings_view(),
        })];
        events.extend(engine.refresh().await?);
        if let Some(from) = options.from {
            engine.projection.journal_next_from = from;
        }
        Ok((engine, events))
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.projection.session_id
    }

    pub(crate) fn turns(&self) -> &[crate::chat::protocol::ChatTurn] {
        &self.projection.turns
    }

    pub(crate) async fn handle_command(&mut self, command: ChatCommand) -> Result<Vec<ChatEvent>> {
        match command {
            ChatCommand::SubmitUserMessage { text } => self.submit_user_message(text).await,
            ChatCommand::SetDraftProvider { provider } => self.set_provider(provider),
            ChatCommand::SetDraftModel { model } => self.set_model(model),
            ChatCommand::SetDraftReasoningEffort { effort } => self.set_effort(effort),
            ChatCommand::SetDraftMaxTokens { max_tokens } => self.set_max_tokens(max_tokens),
            ChatCommand::SteerRun { text } => self.steer_run(text).await,
            ChatCommand::InterruptRun { reason } => self.interrupt_run(reason).await,
            ChatCommand::PauseSession => {
                self.lifecycle_input(SessionInputKind::SessionPaused).await
            }
            ChatCommand::ResumeSession => {
                self.lifecycle_input(SessionInputKind::SessionResumed).await
            }
            ChatCommand::SwitchSession { session_id } => self.switch_session(session_id).await,
            ChatCommand::Refresh => self.refresh().await,
            ChatCommand::Shutdown => Ok(vec![ChatEvent::StatusChanged(ChatStatus {
                session_id: self.session_id().to_string(),
                status: "shutdown".into(),
                detail: None,
                settings: self.settings_view(),
            })]),
        }
    }

    pub(crate) async fn refresh(&mut self) -> Result<Vec<ChatEvent>> {
        let snapshot = self.client.fetch_session_state(self.session_id()).await?;
        if let Some(state) = snapshot.state.as_ref() {
            self.observed_clock = self.observed_clock.max(state.updated_at);
            self.base_session_config = state.session_config.clone();
            self.seed_draft_from_state_if_empty(state);
            self.blob_cache
                .prefetch_session_state(&self.client, state)
                .await;
        }

        let mut events = Vec::new();
        let summary = snapshot
            .state
            .as_ref()
            .map(summary_from_state)
            .unwrap_or_else(|| bare_summary(self.session_id().to_string()));
        events.push(ChatEvent::SessionSelected(summary));
        events.extend(self.projection.apply_state(
            snapshot.journal_head,
            snapshot.state,
            &self.blob_cache,
        ));
        events.push(ChatEvent::StatusChanged(ChatStatus {
            session_id: self.session_id().to_string(),
            status: self.session_status_text(),
            detail: None,
            settings: self.settings_view(),
        }));
        Ok(events)
    }

    pub(crate) async fn follow_until_quiescent<F>(
        &mut self,
        timeout: Duration,
        mut emit: F,
    ) -> Result<()>
    where
        F: FnMut(ChatEvent),
    {
        let deadline = Instant::now() + timeout;
        let mut next_from = if self.projection.journal_next_from == 0 {
            self.client.journal_head_next_from().await?
        } else {
            self.projection.journal_next_from
        };
        let mut backoff = Duration::from_millis(250);
        loop {
            if self.is_quiescent() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for session '{}' to become idle",
                    self.session_id()
                ));
            }
            let mut stream = match self.client.stream_journal(next_from).await {
                Ok(stream) => stream,
                Err(err) => {
                    emit(ChatEvent::Reconnecting {
                        from: next_from,
                        reason: err.to_string(),
                    });
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                    continue;
                }
            };
            backoff = Duration::from_millis(250);
            loop {
                if self.is_quiescent() {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for session '{}' to become idle",
                        self.session_id()
                    ));
                }
                let event = match stream.next_event().await {
                    Ok(Some(event)) => event,
                    Ok(None) => {
                        emit(ChatEvent::Reconnecting {
                            from: next_from,
                            reason: "journal stream closed".into(),
                        });
                        break;
                    }
                    Err(err) => {
                        emit(ChatEvent::Reconnecting {
                            from: next_from,
                            reason: err.to_string(),
                        });
                        break;
                    }
                };
                match event {
                    JournalSseEvent::JournalRecord {
                        next_from: next, ..
                    } => {
                        next_from = next;
                        self.projection.journal_next_from = next;
                        for event in self.refresh().await? {
                            emit(event);
                        }
                    }
                    JournalSseEvent::WorldHead {
                        next_from: next, ..
                    } => {
                        next_from = next;
                        self.projection.journal_next_from = next;
                        for event in self.refresh().await? {
                            emit(event);
                        }
                    }
                    JournalSseEvent::Gap {
                        requested_from,
                        retained_from,
                        next_from: next,
                    } => {
                        next_from = next;
                        emit(ChatEvent::GapObserved {
                            requested_from,
                            retained_from,
                        });
                        for event in self.refresh().await? {
                            emit(event);
                        }
                    }
                    JournalSseEvent::Error { message } => {
                        emit(ChatEvent::Error(ChatErrorView {
                            message,
                            action: Some("reconnecting to the world journal".into()),
                        }));
                        break;
                    }
                    JournalSseEvent::Unknown { .. } => {}
                }
            }
        }
    }

    fn is_quiescent(&self) -> bool {
        let Some(state) = self.projection.session_state.as_ref() else {
            return false;
        };
        state.current_run.is_none()
            && state.queued_follow_up_runs.is_empty()
            && !session_active(state.lifecycle)
    }

    async fn submit_user_message(&mut self, text: String) -> Result<Vec<ChatEvent>> {
        let message = json!({
            "role": "user",
            "content": text,
            "source": {
                "kind": "aos-cli",
                "channel": "terminal",
                "session_id": self.session_id(),
            }
        });
        let bytes = serde_json::to_vec(&message).context("encode chat user message blob")?;
        let input_ref = self.client.upload_blob(bytes.clone()).await?;
        self.blob_cache.insert_bytes(input_ref.clone(), bytes);

        let input_kind = if self.should_start_first_run() {
            SessionInputKind::RunRequested {
                input_ref: input_ref.clone(),
                run_overrides: Some(self.current_session_config()),
            }
        } else {
            SessionInputKind::FollowUpInputAppended {
                input_ref: input_ref.clone(),
                run_overrides: Some(self.current_session_config()),
            }
        };
        let input = self.session_input(input_kind);
        self.client.submit_session_input(&input).await?;

        let mut events = vec![ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
            session_id: self.session_id().to_string(),
            message: ChatMessageView {
                id: input_ref.clone(),
                role: "user".into(),
                content: text,
                ref_: Some(input_ref),
            },
        })];
        events.extend(self.refresh().await?);
        Ok(events)
    }

    async fn steer_run(&mut self, text: String) -> Result<Vec<ChatEvent>> {
        let bytes = serde_json::to_vec(&json!({
            "role": "user",
            "content": text,
            "source": {
                "kind": "aos-cli",
                "channel": "terminal",
                "session_id": self.session_id(),
                "lane": "steer"
            }
        }))
        .context("encode steer instruction blob")?;
        let instruction_ref = self.client.upload_blob(bytes.clone()).await?;
        self.blob_cache.insert_bytes(instruction_ref.clone(), bytes);
        let input = self.session_input(SessionInputKind::RunSteerRequested { instruction_ref });
        self.client.submit_session_input(&input).await?;
        self.refresh().await
    }

    async fn interrupt_run(&mut self, reason: Option<String>) -> Result<Vec<ChatEvent>> {
        let reason_ref = match reason {
            Some(reason) => {
                let bytes = serde_json::to_vec(&json!({
                    "role": "user",
                    "content": reason,
                    "source": {
                        "kind": "aos-cli",
                        "channel": "terminal",
                        "session_id": self.session_id(),
                        "lane": "interrupt"
                    }
                }))
                .context("encode interrupt reason blob")?;
                let ref_ = self.client.upload_blob(bytes.clone()).await?;
                self.blob_cache.insert_bytes(ref_.clone(), bytes);
                Some(ref_)
            }
            None => None,
        };
        let input = self.session_input(SessionInputKind::RunInterruptRequested { reason_ref });
        self.client.submit_session_input(&input).await?;
        self.refresh().await
    }

    async fn lifecycle_input(&mut self, input_kind: SessionInputKind) -> Result<Vec<ChatEvent>> {
        let input = self.session_input(input_kind);
        self.client.submit_session_input(&input).await?;
        self.refresh().await
    }

    async fn switch_session(&mut self, session_id: String) -> Result<Vec<ChatEvent>> {
        let session_id = validate_session_id(&session_id)?;
        let mut events = vec![self.projection.reset(session_id)];
        events.extend(self.refresh().await?);
        Ok(events)
    }

    fn set_provider(&mut self, provider: String) -> Result<Vec<ChatEvent>> {
        if self.model_locked() {
            return Ok(vec![ChatEvent::Error(ChatErrorView {
                message:
                    "provider switching is not supported after this session has accepted a run"
                        .into(),
                action: Some("start a new session with /new for another provider".into()),
            })]);
        }
        self.draft_settings.provider = provider;
        self.draft_overrides.provider = true;
        Ok(vec![self.setting_status("provider updated")])
    }

    fn set_model(&mut self, model: String) -> Result<Vec<ChatEvent>> {
        if self.model_locked() {
            return Ok(vec![ChatEvent::Error(ChatErrorView {
                message: "model switching is not supported after this session has accepted a run"
                    .into(),
                action: Some("start a new session with /new for another model".into()),
            })]);
        }
        self.draft_settings.model = model;
        self.draft_overrides.model = true;
        Ok(vec![self.setting_status("model updated")])
    }

    fn set_effort(&mut self, effort: Option<ReasoningEffort>) -> Result<Vec<ChatEvent>> {
        if self.run_active() {
            return Ok(vec![ChatEvent::Error(ChatErrorView {
                message: "reasoning effort cannot be changed while a run is active".into(),
                action: Some(
                    "wait for the current run to finish, then set effort for the next run".into(),
                ),
            })]);
        }
        self.draft_settings.reasoning_effort = effort;
        self.draft_overrides.reasoning_effort = true;
        Ok(vec![self.setting_status("reasoning effort updated")])
    }

    fn set_max_tokens(&mut self, max_tokens: Option<u64>) -> Result<Vec<ChatEvent>> {
        if self.run_active() {
            return Ok(vec![ChatEvent::Error(ChatErrorView {
                message: "max tokens cannot be changed while a run is active".into(),
                action: Some(
                    "wait for the current run to finish, then set max tokens for the next run"
                        .into(),
                ),
            })]);
        }
        self.draft_settings.max_tokens = max_tokens;
        self.draft_overrides.max_tokens = true;
        Ok(vec![self.setting_status("max tokens updated")])
    }

    fn setting_status(&self, status: &str) -> ChatEvent {
        ChatEvent::StatusChanged(ChatStatus {
            session_id: self.session_id().to_string(),
            status: status.into(),
            detail: None,
            settings: self.settings_view(),
        })
    }

    fn session_input(&mut self, input: SessionInputKind) -> SessionInput {
        self.observed_clock = self.observed_clock.saturating_add(1).max(1);
        SessionInput {
            session_id: SessionId(self.session_id().to_string()),
            observed_at_ns: self.observed_clock,
            input,
        }
    }

    fn should_start_first_run(&self) -> bool {
        let Some(state) = self.projection.session_state.as_ref() else {
            return true;
        };
        state.next_run_seq == 0
            && state.current_run.is_none()
            && state.run_history.is_empty()
            && state.queued_follow_up_runs.is_empty()
    }

    fn current_session_config(&self) -> SessionConfig {
        let mut config = self.base_session_config.clone();
        config.provider = self.draft_settings.provider.clone();
        config.model = self.draft_settings.model.clone();
        config.reasoning_effort = self.draft_settings.reasoning_effort;
        config.max_tokens = self.draft_settings.max_tokens;
        config
    }

    fn seed_draft_from_state_if_empty(&mut self, state: &SessionState) {
        if !self.draft_overrides.provider && !state.session_config.provider.is_empty() {
            self.draft_settings.provider = state.session_config.provider.clone();
        }
        if !self.draft_overrides.model && !state.session_config.model.is_empty() {
            self.draft_settings.model = state.session_config.model.clone();
        }
        if !self.draft_overrides.reasoning_effort {
            self.draft_settings.reasoning_effort = state.session_config.reasoning_effort;
        }
        if !self.draft_overrides.max_tokens {
            self.draft_settings.max_tokens = state.session_config.max_tokens;
        }
    }

    fn model_locked(&self) -> bool {
        self.projection.session_state.as_ref().is_some_and(|state| {
            state.next_run_seq > 0 || state.current_run.is_some() || !state.run_history.is_empty()
        })
    }

    fn run_active(&self) -> bool {
        self.projection
            .session_state
            .as_ref()
            .is_some_and(|state| session_active(state.lifecycle))
    }

    fn settings_view(&self) -> ChatSettingsView {
        let provider_model_editable = !self.model_locked();
        let run_editable = !self.run_active();
        let active = self
            .projection
            .session_state
            .as_ref()
            .and_then(|state| state.active_run_config.as_ref());
        ChatSettingsView {
            provider: active
                .map(|config| config.provider.clone())
                .unwrap_or_else(|| self.draft_settings.provider.clone()),
            model: active
                .map(|config| config.model.clone())
                .unwrap_or_else(|| self.draft_settings.model.clone()),
            reasoning_effort: active
                .and_then(|config| config.reasoning_effort)
                .or(self.draft_settings.reasoning_effort),
            max_tokens: active
                .and_then(|config| config.max_tokens)
                .or(self.draft_settings.max_tokens),
            provider_editable: provider_model_editable,
            model_editable: provider_model_editable,
            effort_editable: run_editable,
            max_tokens_editable: run_editable,
        }
    }

    fn session_status_text(&self) -> String {
        self.projection
            .session_state
            .as_ref()
            .map(|state| match state.lifecycle {
                SessionLifecycle::Idle => "idle",
                SessionLifecycle::Running => "running",
                SessionLifecycle::WaitingInput => "waiting_input",
                SessionLifecycle::Paused => "paused",
                SessionLifecycle::Cancelling => "cancelling",
                SessionLifecycle::Completed => "completed",
                SessionLifecycle::Failed => "failed",
                SessionLifecycle::Cancelled => "cancelled",
                SessionLifecycle::Interrupted => "interrupted",
            })
            .unwrap_or("new")
            .to_string()
    }
}
