use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use aos_agent::{ActiveToolBatch, RunRecord, RunState, SessionState};
use aos_effect_types::LlmOutputEnvelope;
use serde::Deserialize;
use serde_json::Value;

use crate::chat::client::ChatControlClient;

#[derive(Debug, Clone, Default)]
pub(crate) struct BlobCache {
    entries: BTreeMap<String, BlobEntry>,
}

#[derive(Debug, Clone)]
enum BlobEntry {
    Bytes(Vec<u8>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserMessageBlob {
    pub role: String,
    pub content: String,
}

impl BlobCache {
    pub(crate) async fn prefetch_session_state(
        &mut self,
        client: &ChatControlClient,
        state: &SessionState,
    ) {
        let mut refs = BTreeSet::new();
        collect_state_refs(state, &mut refs);
        for ref_ in refs {
            if self.entries.contains_key(&ref_) {
                continue;
            }
            let entry = match client.fetch_blob(&ref_).await {
                Ok(bytes) => BlobEntry::Bytes(bytes),
                Err(err) => BlobEntry::Error(err.to_string()),
            };
            self.entries.insert(ref_, entry);
        }
    }

    pub(crate) fn insert_bytes(&mut self, ref_: String, bytes: Vec<u8>) {
        self.entries.insert(ref_, BlobEntry::Bytes(bytes));
    }

    pub(crate) fn user_message(&self, ref_: &str) -> Option<UserMessageBlob> {
        let bytes = self.bytes(ref_)?;
        decode_user_message(bytes).ok()
    }

    pub(crate) fn assistant_text(&self, ref_: &str) -> Option<String> {
        let bytes = self.bytes(ref_)?;
        decode_llm_output(bytes)
            .ok()
            .and_then(|output| output.assistant_text)
    }

    pub(crate) fn preview_json_or_text(&self, ref_: &str) -> Option<String> {
        let bytes = self.bytes(ref_)?;
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            return Some(compact_preview(&value.to_string(), 180));
        }
        std::str::from_utf8(bytes)
            .ok()
            .map(|text| compact_preview(text, 180))
    }

    pub(crate) fn error(&self, ref_: &str) -> Option<&str> {
        match self.entries.get(ref_) {
            Some(BlobEntry::Error(message)) => Some(message.as_str()),
            _ => None,
        }
    }

    fn bytes(&self, ref_: &str) -> Option<&[u8]> {
        match self.entries.get(ref_) {
            Some(BlobEntry::Bytes(bytes)) => Some(bytes.as_slice()),
            _ => None,
        }
    }
}

fn collect_state_refs(state: &SessionState, refs: &mut BTreeSet<String>) {
    for input in &state.turn_state.pinned_inputs {
        refs.insert(input.content_ref.clone());
    }
    for input in &state.turn_state.durable_inputs {
        refs.insert(input.content_ref.clone());
    }
    for run in &state.run_history {
        collect_run_record_refs(run, refs);
    }
    if let Some(run) = &state.current_run {
        collect_run_state_refs(run, refs);
    }
    if let Some(ref_) = &state.last_output_ref {
        refs.insert(ref_.clone());
    }
    if let Some(batch) = state.active_tool_batch.as_ref() {
        collect_tool_batch_refs(batch, refs);
    }
}

fn collect_run_record_refs(run: &RunRecord, refs: &mut BTreeSet<String>) {
    refs.extend(run.input_refs.iter().cloned());
    if let Some(outcome) = &run.outcome
        && let Some(ref_) = &outcome.output_ref
    {
        refs.insert(ref_.clone());
    }
}

fn collect_run_state_refs(run: &RunState, refs: &mut BTreeSet<String>) {
    refs.extend(run.input_refs.iter().cloned());
    if let Some(ref_) = &run.last_output_ref {
        refs.insert(ref_.clone());
    }
    if let Some(outcome) = &run.outcome
        && let Some(ref_) = &outcome.output_ref
    {
        refs.insert(ref_.clone());
    }
    if let Some(batch) = run.active_tool_batch.as_ref() {
        collect_tool_batch_refs(batch, refs);
    }
}

fn collect_tool_batch_refs(batch: &ActiveToolBatch, refs: &mut BTreeSet<String>) {
    for call in &batch.plan.observed_calls {
        if let Some(ref_) = &call.arguments_ref {
            refs.insert(ref_.clone());
        }
    }
    for call in &batch.plan.planned_calls {
        if let Some(ref_) = &call.arguments_ref {
            refs.insert(ref_.clone());
        }
    }
    if let Some(ref_) = &batch.results_ref {
        refs.insert(ref_.clone());
    }
}

fn decode_user_message(bytes: &[u8]) -> Result<UserMessageBlob> {
    #[derive(Deserialize)]
    struct Message {
        #[serde(default)]
        role: Option<String>,
        content: String,
    }

    if let Ok(message) = serde_json::from_slice::<Message>(bytes) {
        return Ok(UserMessageBlob {
            role: message.role.unwrap_or_else(|| "user".into()),
            content: message.content,
        });
    }
    let text = std::str::from_utf8(bytes).context("user message blob is not UTF-8 JSON/text")?;
    Ok(UserMessageBlob {
        role: "user".into(),
        content: text.to_string(),
    })
}

fn decode_llm_output(bytes: &[u8]) -> Result<LlmOutputEnvelope> {
    serde_json::from_slice(bytes).context("decode LLM output envelope")
}

fn compact_preview(value: &str, max_chars: usize) -> String {
    let mut out = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > max_chars {
        out.truncate(max_chars.saturating_sub(1));
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_user_message_json_blob() {
        let message = decode_user_message(br#"{"role":"user","content":"hello"}"#).unwrap();
        assert_eq!(
            message,
            UserMessageBlob {
                role: "user".into(),
                content: "hello".into()
            }
        );
    }

    #[test]
    fn decodes_llm_output_assistant_text() {
        let output = decode_llm_output(br#"{"assistant_text":"done"}"#).unwrap();
        assert_eq!(output.assistant_text.as_deref(), Some("done"));
    }
}
