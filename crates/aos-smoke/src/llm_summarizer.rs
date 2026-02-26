use std::path::Path;

use anyhow::{Result, anyhow};
use aos_air_types::HashRef;
use aos_effects::builtins::{LlmFinishReason, LlmGenerateReceipt, TokenUsage};
use aos_effects::{EffectKind, EffectReceipt, ReceiptStatus};
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};

const REDUCER_NAME: &str = "demo/LlmSummarizer@1";
const EVENT_SCHEMA: &str = "demo/LlmSummarizerEvent@1";
const MODULE_PATH: &str = "crates/aos-smoke/fixtures/07-llm-summarizer/reducer";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum LlmSummarizerEventEnvelope {
        Start { url: String },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmSummarizerStateView {
    pc: StatePcView,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_summary: Option<String>,
    last_tokens_prompt: Option<u64>,
    last_tokens_completion: Option<u64>,
    last_cost_millis: Option<u64>,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum StatePcView {
        Idle,
        Fetching,
        Summarizing,
        Done,
    }
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;

    println!("→ LLM summarizer demo");
    let start_event = LlmSummarizerEventEnvelope::Start {
        url: "https://example.com/story.txt".into(),
    };
    host.send_event(&start_event)?;

    let mut http = MockHttpHarness::new();
    let requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "summarizer workflow expected 1 http intent, got {}",
            requests.len()
        ));
    }
    let http_ctx = requests.into_iter().next().unwrap();
    let document = "AOS keeps workflows and reducers separate. Summaries should be deterministic so"
        .to_string() + " reviewers can trust the replay path.";
    let store = host.store();
    http.respond_with_body(
        host.kernel_mut(),
        Some(store.as_ref()),
        http_ctx,
        MockHttpResponse::json(200, document.clone()),
    )?;

    let intents = host.kernel_mut().drain_effects()?;
    let llm_intent = intents
        .into_iter()
        .find(|intent| intent.kind.as_str() == EffectKind::LLM_GENERATE)
        .ok_or_else(|| anyhow!("expected one llm.generate intent"))?;

    let output_ref =
        HashRef::new("sha256:1111111111111111111111111111111111111111111111111111111111111111")?;
    let llm_receipt_payload = LlmGenerateReceipt {
        output_ref: output_ref.clone(),
        raw_output_ref: None,
        provider_response_id: None,
        finish_reason: LlmFinishReason {
            reason: "stop".into(),
            raw: None,
        },
        token_usage: TokenUsage {
            prompt: 120,
            completion: 42,
            total: Some(162),
        },
        usage_details: None,
        warnings_ref: None,
        rate_limit_ref: None,
        cost_cents: Some(0),
        provider_id: "mock.llm".into(),
    };
    host.kernel_mut().handle_receipt(EffectReceipt {
        intent_hash: llm_intent.intent_hash,
        adapter_id: "mock.llm".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&llm_receipt_payload)?,
        cost_cents: Some(0),
        signature: vec![0; 64],
    })?;
    host.kernel_mut().tick_until_idle()?;

    let state: LlmSummarizerStateView = host.read_state()?;
    if state.last_summary.is_none() {
        return Err(anyhow!("summary missing from reducer state"));
    }
    println!(
        "   summary_ref={:?} tokens={{prompt:{:?}, completion:{:?}}} cost_millis={:?}",
        state.last_summary,
        state.last_tokens_prompt,
        state.last_tokens_completion,
        state.last_cost_millis
    );

    host.finish()?.verify_replay()?;

    Ok(())
}
