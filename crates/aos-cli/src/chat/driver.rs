use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_agent::{
    HostSessionOpenConfig, HostTargetConfig, ReasoningEffort, SessionConfig, SessionId,
    SessionInput, SessionInputKind, SessionLifecycle, SessionState, ToolProfileBuilder,
    ToolRegistryBuilder, ToolSpec, local_coding_agent_tool_profile_for_provider,
    local_coding_agent_tool_profiles, local_coding_agent_tool_registry, tool_bundle_inspect,
    tool_bundle_workspace,
};
use serde_json::json;
use tokio::time::sleep;

use crate::chat::blob_cache::BlobCache;
use crate::chat::client::{ChatControlClient, bare_summary, summary_from_state};
use crate::chat::projection::ChatProjection;
use crate::chat::prompts::selected_prompt_text;
use crate::chat::protocol::{
    ChatCommand, ChatConnectionInfo, ChatDelta, ChatDraftOverrideMask, ChatDraftSettings,
    ChatErrorView, ChatEvent, ChatMessageView, ChatPromptConfig, ChatSettingsView, ChatStatus,
    ChatToolMode, session_active,
};
use crate::chat::session::{new_session_id, validate_session_id};
use crate::chat::sse::JournalSseEvent;

#[derive(Debug, Clone)]
pub(crate) struct ChatSessionDriverOptions {
    pub session_id: String,
    pub draft_settings: ChatDraftSettings,
    pub draft_overrides: ChatDraftOverrideMask,
    pub tool_mode: ChatToolMode,
    pub prompt_config: ChatPromptConfig,
    pub workdir: String,
    pub from: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_coding_bootstrap_selects_provider_profile() {
        let bootstrap =
            build_tool_bootstrap(ChatToolMode::LocalCoding, "openai-responses").expect("bootstrap");

        assert_eq!(bootstrap.default_profile, "openai");
        assert!(bootstrap.registry.contains_key("host.exec"));
        assert!(bootstrap.registry.contains_key("host.fs.apply_patch"));
        assert!(
            bootstrap
                .profiles
                .get("openai")
                .is_some_and(|tools| tools.iter().any(|tool| tool == "host.exec"))
        );
    }

    #[test]
    fn workspace_bootstrap_uses_workspace_only_profile() {
        let bootstrap =
            build_tool_bootstrap(ChatToolMode::Workspace, "openai-responses").expect("bootstrap");

        assert_eq!(bootstrap.default_profile, "workspace");
        assert!(bootstrap.registry.contains_key("workspace.read"));
        assert!(!bootstrap.registry.contains_key("host.exec"));
    }

    #[test]
    fn local_host_config_uses_requested_workdir() {
        let config = local_host_session_open_config("/tmp/aos-chat");

        match config.target {
            HostTargetConfig::Local { workdir, .. } => {
                assert_eq!(workdir.as_deref(), Some("/tmp/aos-chat"));
            }
            HostTargetConfig::Sandbox { .. } => panic!("expected local target"),
        }
    }
}

pub(crate) struct ChatSessionDriver {
    client: ChatControlClient,
    projection: ChatProjection,
    blob_cache: BlobCache,
    draft_settings: ChatDraftSettings,
    draft_overrides: ChatDraftOverrideMask,
    tool_mode: ChatToolMode,
    prompt_config: ChatPromptConfig,
    prompt_ref: Option<String>,
    workdir: String,
    base_session_config: SessionConfig,
    observed_clock: u64,
}

struct ToolBootstrap {
    registry: BTreeMap<String, ToolSpec>,
    profiles: BTreeMap<String, Vec<String>>,
    default_profile: String,
}

fn build_tool_bootstrap(mode: ChatToolMode, provider: &str) -> Result<ToolBootstrap> {
    match mode {
        ChatToolMode::None => Ok(ToolBootstrap {
            registry: BTreeMap::new(),
            profiles: BTreeMap::new(),
            default_profile: String::new(),
        }),
        ChatToolMode::Inspect => {
            let bundle = tool_bundle_inspect();
            let registry = ToolRegistryBuilder::new()
                .with_bundle(bundle.clone())
                .build()
                .map_err(|err| anyhow!(err))?;
            let profile = ToolProfileBuilder::new()
                .with_bundle(bundle)
                .build_for_registry(&registry)
                .map_err(|err| anyhow!(err))?;
            Ok(tool_bootstrap_with_profile(registry, "inspect", profile))
        }
        ChatToolMode::Workspace => {
            let bundle = tool_bundle_workspace();
            let registry = ToolRegistryBuilder::new()
                .with_bundle(bundle.clone())
                .build()
                .map_err(|err| anyhow!(err))?;
            let profile = ToolProfileBuilder::new()
                .with_bundle(bundle)
                .build_for_registry(&registry)
                .map_err(|err| anyhow!(err))?;
            Ok(tool_bootstrap_with_profile(registry, "workspace", profile))
        }
        ChatToolMode::LocalCoding => {
            let registry = local_coding_agent_tool_registry();
            let profiles = local_coding_agent_tool_profiles();
            let default_profile = local_coding_agent_tool_profile_for_provider(provider);
            Ok(ToolBootstrap {
                registry,
                profiles,
                default_profile,
            })
        }
    }
}

fn tool_bootstrap_with_profile(
    registry: BTreeMap<String, ToolSpec>,
    profile_id: &str,
    profile: Vec<String>,
) -> ToolBootstrap {
    let mut profiles = BTreeMap::new();
    profiles.insert(profile_id.into(), profile);
    ToolBootstrap {
        registry,
        profiles,
        default_profile: profile_id.into(),
    }
}

fn local_host_session_open_config(workdir: &str) -> HostSessionOpenConfig {
    HostSessionOpenConfig {
        target: HostTargetConfig::Local {
            mounts: None,
            workdir: Some(workdir.into()),
            env: None,
            network_mode: Some("none".into()),
        },
        session_ttl_ns: None,
        labels: None,
    }
}

impl ChatSessionDriver {
    pub(crate) async fn open(
        client: ChatControlClient,
        options: ChatSessionDriverOptions,
    ) -> Result<(Self, Vec<ChatEvent>)> {
        let session_id = validate_session_id(&options.session_id)?;
        let mut driver = Self {
            projection: ChatProjection::new(client.world_id().to_string(), session_id.clone()),
            client,
            blob_cache: BlobCache::default(),
            draft_settings: options.draft_settings,
            draft_overrides: options.draft_overrides,
            tool_mode: options.tool_mode,
            prompt_config: options.prompt_config,
            prompt_ref: None,
            workdir: options.workdir,
            base_session_config: SessionConfig::default(),
            observed_clock: 0,
        };
        let mut events = vec![ChatEvent::Connected(ChatConnectionInfo {
            world_id: driver.client.world_id().to_string(),
            session_id: session_id.clone(),
            journal_next_from: options.from,
            settings: driver.settings_view(),
        })];
        events.extend(driver.refresh().await?);
        if let Some(from) = options.from {
            driver.projection.journal_next_from = from;
        }
        Ok((driver, events))
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
            ChatCommand::ListSessions => self.list_sessions().await,
            ChatCommand::NewSession => self.switch_session(new_session_id()).await,
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
        if matches!(state.lifecycle, SessionLifecycle::WaitingInput) {
            return true;
        }
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

        let mut events = Vec::new();
        if self.waiting_for_user_input() {
            let input = self.session_input(SessionInputKind::RunCompleted);
            self.client.submit_session_input(&input).await?;
            events.extend(self.refresh().await?);
        }
        if !self.run_active() {
            self.bootstrap_next_run_tools().await?;
            self.bootstrap_next_run_prompt().await?;
        }

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

        events.push(ChatEvent::TranscriptDelta(ChatDelta::AppendMessage {
            session_id: self.session_id().to_string(),
            message: ChatMessageView {
                id: input_ref.clone(),
                role: "user".into(),
                content: text,
                ref_: Some(input_ref),
            },
        }));
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

    async fn list_sessions(&mut self) -> Result<Vec<ChatEvent>> {
        Ok(vec![ChatEvent::SessionsListed {
            world_id: self.client.world_id().to_string(),
            sessions: self.client.list_sessions().await?,
        }])
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

    async fn bootstrap_next_run_tools(&mut self) -> Result<()> {
        if self.tool_mode == ChatToolMode::None {
            return Ok(());
        }
        if self
            .projection
            .session_state
            .as_ref()
            .is_some_and(|state| !state.tool_registry.is_empty())
        {
            return Ok(());
        }

        let bootstrap = build_tool_bootstrap(self.tool_mode, self.draft_settings.provider.as_str())
            .context("build chat tool registry")?;
        let input = self.session_input(SessionInputKind::ToolRegistrySet {
            registry: bootstrap.registry,
            profiles: Some(bootstrap.profiles),
            default_profile: Some(bootstrap.default_profile),
        });
        self.client.submit_session_input(&input).await.map(|_| ())
    }

    async fn bootstrap_next_run_prompt(&mut self) -> Result<()> {
        if self.prompt_ref.is_some() || self.session_has_prompt_refs() {
            return Ok(());
        }
        let Some(prompt_text) = selected_prompt_text(&self.prompt_config, self.tool_mode) else {
            return Ok(());
        };
        let message = json!({
            "role": "developer",
            "content": prompt_text,
            "source": {
                "kind": "aos-cli",
                "channel": "terminal",
                "session_id": self.session_id(),
                "prompt": "bootstrap"
            }
        });
        let bytes = serde_json::to_vec(&message).context("encode chat prompt blob")?;
        let prompt_ref = self.client.upload_blob(bytes.clone()).await?;
        self.blob_cache.insert_bytes(prompt_ref.clone(), bytes);
        self.prompt_ref = Some(prompt_ref);
        Ok(())
    }

    fn session_has_prompt_refs(&self) -> bool {
        self.projection
            .session_state
            .as_ref()
            .and_then(|state| state.session_config.default_prompt_refs.as_ref())
            .is_some_and(|refs| !refs.is_empty())
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

    fn waiting_for_user_input(&self) -> bool {
        self.projection
            .session_state
            .as_ref()
            .is_some_and(|state| matches!(state.lifecycle, SessionLifecycle::WaitingInput))
    }

    fn current_session_config(&self) -> SessionConfig {
        let mut config = self.base_session_config.clone();
        config.provider = self.draft_settings.provider.clone();
        config.model = self.draft_settings.model.clone();
        config.reasoning_effort = self.draft_settings.reasoning_effort;
        config.max_tokens = self.draft_settings.max_tokens;
        if let Some(prompt_ref) = self.prompt_ref.clone() {
            config.default_prompt_refs = Some(vec![prompt_ref]);
        }
        if matches!(self.tool_mode, ChatToolMode::LocalCoding) {
            config.default_host_session_open = Some(local_host_session_open_config(&self.workdir));
        }
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
