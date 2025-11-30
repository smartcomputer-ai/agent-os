use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::support::http_harness::{HttpHarness, MockHttpResponse};
use crate::support::reducer_harness::{ExampleReducerHarness, HarnessConfig};
use aos_testkit::MockLlmHarness;

const REDUCER_NAME: &str = "demo/LlmSummarizer@1";
const EVENT_SCHEMA: &str = "demo/LlmSummarizerEvent@1";
const MODULE_PATH: &str = "examples/07-llm-summarizer/reducer";
const DEMO_LLM_API_KEY: &str = "demo-llm-api-key";

#[derive(Debug, Clone, Serialize, Deserialize)]
enum LlmSummarizerEventEnvelope {
    Start { url: String },
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

#[derive(Debug, Clone, Serialize, Deserialize)]
enum StatePcView {
    Idle,
    Summarizing,
    Done,
}

pub fn run(example_root: &Path) -> Result<()> {
    let harness = ExampleReducerHarness::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;
    let mut run = harness.start()?;

    println!("→ LLM summarizer demo");
    let start_event = LlmSummarizerEventEnvelope::Start {
        url: "https://example.com/story.txt".into(),
    };
    run.submit_event(&start_event)?;

    let mut http = HttpHarness::new();
    let requests = http.collect_requests(run.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "summarizer plan expected 1 http intent, got {}",
            requests.len()
        ));
    }
    let http_ctx = requests.into_iter().next().unwrap();
    let document = "AOS keeps plans and reducers separate. Summaries should be deterministic so"
        .to_string()
        + " reviewers can trust the replay path.";
    let store = harness.store();
    http.respond_with_body(
        run.kernel_mut(),
        Some(store.as_ref()),
        http_ctx,
        MockHttpResponse::json(200, document.clone()),
    )?;

    let mut llm = MockLlmHarness::new(store.clone()).with_expected_api_key(DEMO_LLM_API_KEY);
    let llm_requests = llm.collect_requests(run.kernel_mut())?;
    if llm_requests.len() != 1 {
        return Err(anyhow!(
            "expected one llm.generate intent, found {}",
            llm_requests.len()
        ));
    }
    llm.respond_with(run.kernel_mut(), llm_requests.into_iter().next().unwrap())?;

    let state: LlmSummarizerStateView = run.read_state()?;
    if state.last_summary.is_none() {
        return Err(anyhow!("summary missing from reducer state"));
    }
    println!(
        "   summary={:?} tokens={{prompt:{:?}, completion:{:?}}} cost_millis={:?}",
        state.last_summary,
        state.last_tokens_prompt,
        state.last_tokens_completion,
        state.last_cost_millis
    );

    run.finish()?.verify_replay()?;

    Ok(())
}
