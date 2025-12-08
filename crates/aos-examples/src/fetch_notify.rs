use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};

const REDUCER_NAME: &str = "demo/FetchNotify@1";
const EVENT_SCHEMA: &str = "demo/FetchNotifyEvent@1";
const MODULE_PATH: &str = "examples/03-fetch-notify/reducer";
#[derive(Debug, Clone, Serialize, Deserialize)]
enum FetchEventEnvelope {
    Start { url: String, method: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FetchStateView {
    pc: FetchPcView,
    next_request_id: u64,
    pending_request: Option<u64>,
    last_status: Option<i64>,
    last_body_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum FetchPcView {
    Idle,
    Fetching,
    Done,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;

    println!("→ Fetch & Notify demo");
    let start_event = FetchEventEnvelope::Start {
        url: "https://example.com/data.json".into(),
        method: "GET".into(),
    };
    let FetchEventEnvelope::Start { url, method } = &start_event;
    println!("     start fetch → url={url} method={method}");
    host.send_event(&start_event)?;

    let mut http = MockHttpHarness::new();
    let requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "fetch-notify demo expected a single http request, got {}",
            requests.len()
        ));
    }
    let request = requests.into_iter().next().expect("one request");
    println!(
        "     http.request {} {}",
        request.params.method, request.params.url
    );
    let body = format!(
        "{{\"url\":\"{}\",\"method\":\"{}\",\"demo\":true}}",
        request.params.url, request.params.method
    );
    http.respond_with(host.kernel_mut(), request, MockHttpResponse::json(200, body))?;

    let state: FetchStateView = host.read_state()?;
    println!(
        "   completed: pc={:?} status={:?} preview={:?}",
        state.pc, state.last_status, state.last_body_preview
    );

    host.finish()?.verify_replay()?;
    Ok(())
}
