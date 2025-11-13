use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_effects::builtins::{HeaderMap, HttpRequestParams};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::examples::util;
use crate::manifest_loader;

const REDUCER_NAME: &str = "demo/FetchNotify@1";
const EVENT_SCHEMA: &str = "demo/FetchNotifyEvent@1";
const MODULE_PATH: &str = "examples/03-fetch-notify/reducer";
const HTTP_ADAPTER_ID: &str = "http.mock";

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
    util::reset_journal(example_root)?;
    let wasm_bytes = util::compile_reducer(MODULE_PATH)?;
    let store = Arc::new(FsStore::open(example_root).context("open FsStore")?);
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .context("store reducer wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

    let mut loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 03 must provide AIR JSON assets"))?;
    if let Some(plan) = loaded.plans.get("demo/fetch_plan@1") {
        log::debug!("loaded plan steps: {:?}", plan.steps);
    }
    if let Some(schema) = loaded.schemas.get("demo/FetchNotifyEvent@1") {
        log::debug!("event schema ty: {:?}", schema.ty);
    }
    patch_module_hash(&mut loaded, &wasm_hash_ref)?;

    let journal = Box::new(FsJournal::open(example_root)?);
    let kernel_config = util::kernel_config(example_root)?;
    let mut kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        journal,
        kernel_config.clone(),
    )?;

    println!("→ Fetch & Notify demo");
    submit_start(
        &mut kernel,
        FetchEventEnvelope::Start {
            url: "https://example.com/data.json".into(),
            method: "GET".into(),
        },
    )?;

    let mut http = HttpHarness::new(store.clone());
    drain_http_effects(&mut kernel, &mut http)?;

    let final_bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing reducer state"))?;
    let state: FetchStateView = serde_cbor::from_slice(&final_bytes)?;
    println!(
        "   completed: pc={:?} status={:?} preview={:?}",
        state.pc, state.last_status, state.last_body_preview
    );

    drop(kernel);

    // Replay and compare state hashes.
    let mut replay_loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 03 must provide AIR JSON assets"))?;
    patch_module_hash(&mut replay_loaded, &wasm_hash_ref)?;
    let replay_journal = Box::new(FsJournal::open(example_root)?);
    let mut replay = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        replay_loaded,
        replay_journal,
        kernel_config,
    )?;
    replay.tick_until_idle()?;
    let replay_bytes = replay
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing replay state"))?;
    if replay_bytes != final_bytes {
        return Err(anyhow!("replay mismatch: reducer state diverged"));
    }
    let state_hash = Hash::of_bytes(&final_bytes).to_hex();
    println!("   replay check: OK (state hash {state_hash})\n");

    Ok(())
}

fn submit_start(kernel: &mut Kernel<FsStore>, event: FetchEventEnvelope) -> Result<()> {
    let (url, method) = match &event {
        FetchEventEnvelope::Start { url, method } => (url, method),
    };
    println!("     start fetch → url={} method={}", url, method);
    let payload = serde_cbor::to_vec(&event)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn patch_module_hash(loaded: &mut LoadedManifest, wasm_hash: &HashRef) -> Result<()> {
    let module = loaded
        .modules
        .get_mut(REDUCER_NAME)
        .ok_or_else(|| anyhow!("module '{REDUCER_NAME}' missing from manifest"))?;
    module.wasm_hash = wasm_hash.clone();
    Ok(())
}

fn drain_http_effects(kernel: &mut Kernel<FsStore>, harness: &mut HttpHarness) -> Result<()> {
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            match intent.kind.as_str() {
                EffectKind::HTTP_REQUEST => harness.handle_request(kernel, intent)?,
                other => return Err(anyhow!("unexpected effect kind {other}")),
            }
        }
    }
    Ok(())
}

struct HttpHarness {
    store: Arc<FsStore>,
}

impl HttpHarness {
    fn new(store: Arc<FsStore>) -> Self {
        Self { store }
    }

    fn handle_request(&mut self, kernel: &mut Kernel<FsStore>, intent: EffectIntent) -> Result<()> {
        let params_value: ExprValue = serde_cbor::from_slice(&intent.params_cbor)
            .context("decode http request params value")?;
        log::debug!("http.intent params value = {:?}", params_value);
        let params = http_params_from_value(params_value)?;
        println!("     http.request {} {}", params.method, params.url);
        let body = format!(
            "{{\"url\":\"{}\",\"method\":\"{}\",\"demo\":true}}",
            params.url, params.method
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        let receipt_value = build_http_receipt_value(200, &headers, body.clone());
        let receipt = EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: HTTP_ADAPTER_ID.into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt_value)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        };
        kernel.handle_receipt(receipt)?;
        kernel.tick_until_idle()?;
        Ok(())
    }
}

fn http_params_from_value(value: ExprValue) -> Result<HttpRequestParams> {
    let record = match value {
        ExprValue::Record(map) => map,
        other => return Err(anyhow!("http params must be a record, got {:?}", other)),
    };
    let method = record_text(&record, "method")?;
    let url = record_text(&record, "url")?;
    let headers = match record.get("headers") {
        Some(value) => value_to_headers(value)?,
        None => HeaderMap::new(),
    };
    Ok(HttpRequestParams {
        method,
        url,
        headers,
        body_ref: None,
    })
}

fn record_text(record: &indexmap::IndexMap<String, ExprValue>, field: &str) -> Result<String> {
    match record.get(field) {
        Some(ExprValue::Text(text)) => Ok(text.clone()),
        Some(other) => Err(anyhow!("field '{field}' must be text, got {:?}", other)),
        None => Err(anyhow!("field '{field}' missing from http params")),
    }
}

fn value_to_headers(value: &ExprValue) -> Result<HeaderMap> {
    match value {
        ExprValue::Map(map) => {
            let mut headers = HeaderMap::new();
            for (key, entry) in map {
                let name = match key {
                    ValueKey::Text(text) => text.clone(),
                    other => return Err(anyhow!("header key must be text, got {:?}", other)),
                };
                let val = match entry {
                    ExprValue::Text(text) => text.clone(),
                    other => return Err(anyhow!("header value must be text, got {:?}", other)),
                };
                headers.insert(name, val);
            }
            Ok(headers)
        }
        ExprValue::Null | ExprValue::Unit => Ok(HeaderMap::new()),
        other => Err(anyhow!("headers must be a map, got {:?}", other)),
    }
}

fn build_http_receipt_value(status: i64, headers: &HeaderMap, body_preview: String) -> ExprValue {
    let mut record = indexmap::IndexMap::new();
    record.insert("status".into(), ExprValue::Int(status));
    record.insert("headers".into(), headers_to_value(headers));
    record.insert("body_preview".into(), ExprValue::Text(body_preview));
    let mut timings = indexmap::IndexMap::new();
    timings.insert("start_ns".into(), ExprValue::Nat(10));
    timings.insert("end_ns".into(), ExprValue::Nat(20));
    record.insert("timings".into(), ExprValue::Record(timings));
    record.insert("adapter_id".into(), ExprValue::Text(HTTP_ADAPTER_ID.into()));
    ExprValue::Record(record)
}

fn headers_to_value(headers: &HeaderMap) -> ExprValue {
    let mut map = aos_air_exec::ValueMap::new();
    for (key, value) in headers {
        map.insert(ValueKey::Text(key.clone()), ExprValue::Text(value.clone()));
    }
    ExprValue::Map(map)
}
