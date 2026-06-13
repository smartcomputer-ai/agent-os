use anyhow::{Context, Result, anyhow};
use aos_agent::{
    SessionId, SessionInput, SessionInputKind, SessionLifecycle, SessionState, SessionStatus,
};
use aos_cbor::Hash;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::chat::protocol::ChatSessionSummary;
use crate::chat::session::{
    SESSION_INPUT_SCHEMA, SESSION_WORKFLOW, decode_cell_key, encode_session_key_b64,
};
use crate::chat::sse::JournalEventStream;
use crate::client::ApiClient;
use crate::commands::common::{encode_path_segment, universe_query_for_world};

#[derive(Clone)]
pub(crate) struct ChatControlClient {
    api: ApiClient,
    world_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionStateSnapshot {
    pub journal_head: u64,
    pub state: Option<SessionState>,
}

impl ChatControlClient {
    pub(crate) fn new(api: ApiClient, world_id: impl Into<String>) -> Self {
        Self {
            api,
            world_id: world_id.into(),
        }
    }

    pub(crate) fn world_id(&self) -> &str {
        &self.world_id
    }

    pub(crate) async fn upload_blob(&self, bytes: Vec<u8>) -> Result<String> {
        let hash = Hash::of_bytes(&bytes).to_hex();
        let universe_id = universe_query_for_world(&self.api, &self.world_id)
            .await?
            .into_iter()
            .find_map(|(key, value)| (key == "universe_id").then_some(value).flatten())
            .ok_or_else(|| anyhow!("world runtime response missing universe_id"))?;
        self.api
            .put_bytes(
                &format!("/v1/cas/blobs/{hash}?universe_id={universe_id}"),
                bytes,
            )
            .await
            .with_context(|| format!("upload CAS blob {hash}"))?;
        Ok(hash)
    }

    pub(crate) async fn fetch_blob(&self, hash: &str) -> Result<Vec<u8>> {
        self.api
            .get_bytes(
                &format!("/v1/cas/blobs/{hash}"),
                &universe_query_for_world(&self.api, &self.world_id).await?,
            )
            .await
            .with_context(|| format!("fetch CAS blob {hash}"))
    }

    pub(crate) async fn submit_session_input(&self, input: &SessionInput) -> Result<Value> {
        self.api
            .post_json(
                &format!("/v1/worlds/{}/events", self.world_id),
                &json!({
                    "schema": SESSION_INPUT_SCHEMA,
                    "value_json": input,
                    "submission_id": submission_id(input),
                }),
            )
            .await
            .context("submit aos.agent session input")
    }

    pub(crate) async fn fetch_session_state(
        &self,
        session_id: &str,
    ) -> Result<SessionStateSnapshot> {
        let workflow = encode_path_segment(SESSION_WORKFLOW);
        let key_b64 = encode_session_key_b64(session_id)?;
        let data = self
            .api
            .get_json(
                &format!("/v1/worlds/{}/state/{workflow}", self.world_id),
                &[("key_b64", Some(key_b64))],
            )
            .await?;
        let journal_head = data
            .get("journal_head")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let Some(state_b64) = data.get("state_b64").and_then(Value::as_str) else {
            return Ok(SessionStateSnapshot {
                journal_head,
                state: None,
            });
        };
        let bytes = BASE64_STANDARD
            .decode(state_b64)
            .with_context(|| format!("decode SessionState payload '{state_b64}'"))?;
        let state = serde_cbor::from_slice::<SessionState>(&bytes)
            .context("decode aos.agent SessionState")?;
        Ok(SessionStateSnapshot {
            journal_head,
            state: Some(state),
        })
    }

    pub(crate) async fn list_sessions(&self) -> Result<Vec<ChatSessionSummary>> {
        let workflow = encode_path_segment(SESSION_WORKFLOW);
        let data = self
            .api
            .get_json(
                &format!("/v1/worlds/{}/state/{workflow}/cells", self.world_id),
                &[],
            )
            .await?;
        let cells = data
            .get("cells")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut summaries = Vec::new();
        for cell in cells {
            let session_id = match decode_cell_key(&cell) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let summary = match self.fetch_session_state(&session_id).await {
                Ok(snapshot) => snapshot
                    .state
                    .as_ref()
                    .map(summary_from_state)
                    .unwrap_or_else(|| bare_summary(session_id.clone())),
                Err(_) => bare_summary(session_id.clone()),
            };
            summaries.push(summary);
        }
        summaries.sort_by(|left, right| {
            right
                .updated_at_ns
                .unwrap_or_default()
                .cmp(&left.updated_at_ns.unwrap_or_default())
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(summaries)
    }

    pub(crate) async fn journal_head_next_from(&self) -> Result<u64> {
        let data = self
            .api
            .get_json(&format!("/v1/worlds/{}/journal/head", self.world_id), &[])
            .await?;
        Ok(data
            .get("journal_head")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            .saturating_add(1))
    }

    pub(crate) async fn stream_journal(&self, from: u64) -> Result<JournalEventStream> {
        let response = self
            .api
            .get_stream(
                &format!("/v1/worlds/{}/journal/stream", self.world_id),
                &[
                    ("from", Some(from.to_string())),
                    ("kind", Some("domain_event".to_string())),
                    ("kind", Some("effect_intent".to_string())),
                    ("kind", Some("effect_receipt".to_string())),
                    ("kind", Some("stream_frame".to_string())),
                ],
            )
            .await?;
        Ok(JournalEventStream::new(response))
    }
}

pub(crate) fn summary_from_state(state: &SessionState) -> ChatSessionSummary {
    ChatSessionSummary {
        session_id: state.session_id.0.clone(),
        status: Some(state.status),
        lifecycle: Some(state.lifecycle),
        updated_at_ns: Some(state.updated_at),
        run_count: state.run_history.len() as u64 + u64::from(state.current_run.is_some()),
        provider: (!state.session_config.provider.is_empty())
            .then(|| state.session_config.provider.clone()),
        model: (!state.session_config.model.is_empty()).then(|| state.session_config.model.clone()),
        active_run: state.active_run_id.as_ref().map(|run| run_id_label(run)),
    }
}

pub(crate) fn bare_summary(session_id: String) -> ChatSessionSummary {
    ChatSessionSummary {
        session_id,
        status: None,
        lifecycle: None,
        updated_at_ns: None,
        run_count: 0,
        provider: None,
        model: None,
        active_run: None,
    }
}

pub(crate) fn run_id_label(run: &aos_agent::RunId) -> String {
    format!("{}:{}", run.session_id.0, run.run_seq)
}

fn submission_id(input: &SessionInput) -> String {
    let kind = match &input.input {
        SessionInputKind::RunRequested { input_ref, .. } => {
            short_hash_suffix(input_ref).unwrap_or("run")
        }
        SessionInputKind::FollowUpInputAppended { input_ref, .. } => {
            short_hash_suffix(input_ref).unwrap_or("follow")
        }
        SessionInputKind::RunSteerRequested { instruction_ref } => {
            short_hash_suffix(instruction_ref).unwrap_or("steer")
        }
        SessionInputKind::RunInterruptRequested { .. } => "interrupt",
        SessionInputKind::SessionPaused => "pause",
        SessionInputKind::SessionResumed => "resume",
        _ => "input",
    };
    format!(
        "aos-cli-chat-{}-{}-{}",
        input.session_id.0, input.observed_at_ns, kind
    )
}

fn short_hash_suffix(value: &str) -> Option<&str> {
    value.rsplit(':').next().and_then(|hex| hex.get(..12))
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct StateCellSummaryForDocs {
    journal_head: u64,
    workflow: String,
    key_hash: Vec<u8>,
    key_bytes: Vec<u8>,
    state_hash: String,
    size: u64,
    last_active_ns: u64,
}

fn _assert_copy_contracts(_: SessionStatus, _: SessionLifecycle, _: SessionId) {}
