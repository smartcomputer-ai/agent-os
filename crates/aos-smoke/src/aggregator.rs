use std::path::Path;

use anyhow::{Result, anyhow};
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};

const REDUCER_NAME: &str = "demo/Aggregator@1";
const EVENT_SCHEMA: &str = "demo/AggregatorEvent@1";
const MODULE_PATH: &str = "examples/04-aggregator/reducer";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorEventEnvelope {
        Start {
            topic: String,
            primary: AggregationTargetEnvelope,
            secondary: AggregationTargetEnvelope,
            tertiary: AggregationTargetEnvelope,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregationTargetEnvelope {
    name: String,
    url: String,
    method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregatorStateView {
    pc: AggregatorPcView,
    next_request_id: u64,
    pending_request: Option<u64>,
    current_topic: Option<String>,
    pending_targets: Vec<String>,
    last_responses: Vec<AggregateResponseView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregateResponseView {
    source: String,
    status: i64,
    body_preview: String,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum AggregatorPcView {
        Idle,
        Running,
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

    println!("→ Aggregator demo");
    let start_event = AggregatorEventEnvelope::Start {
        topic: "demo-topic".into(),
        primary: AggregationTargetEnvelope {
            name: "alpha".into(),
            url: "https://example.com/api/a".into(),
            method: "GET".into(),
        },
        secondary: AggregationTargetEnvelope {
            name: "beta".into(),
            url: "https://example.com/api/b".into(),
            method: "GET".into(),
        },
        tertiary: AggregationTargetEnvelope {
            name: "gamma".into(),
            url: "https://example.com/api/c".into(),
            method: "GET".into(),
        },
    };
    let AggregatorEventEnvelope::Start { topic, .. } = &start_event;
    println!("     aggregate start → topic={topic}");
    host.send_event(&start_event)?;

    let mut http = MockHttpHarness::new();
    let mut requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 3 {
        return Err(anyhow!(
            "aggregator plan expected 3 http intents, got {}",
            requests.len()
        ));
    }
    requests.sort_by(|a, b| a.params.url.cmp(&b.params.url));
    let ctx_a = requests.remove(0);
    let ctx_b = requests.remove(0);
    let ctx_c = requests.remove(0);

    println!("     responding out of order (b → c → a)");
    http.respond_with(
        host.kernel_mut(),
        ctx_b,
        MockHttpResponse::json(200, "{\"source\":\"beta\"}"),
    )?;
    http.respond_with(
        host.kernel_mut(),
        ctx_c,
        MockHttpResponse::json(201, "{\"source\":\"gamma\"}"),
    )?;
    http.respond_with(
        host.kernel_mut(),
        ctx_a,
        MockHttpResponse::json(202, "{\"source\":\"alpha\"}"),
    )?;

    let state: AggregatorStateView = host.read_state()?;
    if !state.pending_targets.is_empty() {
        return Err(anyhow!(
            "fan-out should clear pending targets, found {:?}",
            state.pending_targets
        ));
    }
    if state.last_responses.len() != 3 {
        return Err(anyhow!(
            "expected 3 aggregated responses, got {}",
            state.last_responses.len()
        ));
    }
    let expected_sources = ["alpha", "beta", "gamma"];
    for (resp, expected) in state.last_responses.iter().zip(expected_sources) {
        if resp.source != expected {
            return Err(anyhow!(
                "response order mismatch: {:?}",
                state.last_responses
            ));
        }
    }
    println!(
        "   completed: pc={:?} responses={:?}",
        state.pc, state.last_responses
    );

    host.finish()?.verify_replay()?;

    Ok(())
}
