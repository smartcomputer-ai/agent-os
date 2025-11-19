use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use aos_air_exec::{Value as ExprValue, ValueKey};
use aos_air_types::{AirNode, HashRef, Manifest, Name};
use aos_effects::builtins::{HeaderMap, HttpRequestParams};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use aos_wasm_build::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_cbor;
use serde_json;
use walkdir::WalkDir;

const REDUCER_NAME: &str = "demo/ChainComp@1";
const EVENT_SCHEMA: &str = "demo/ChainEvent@1";
const REDUCER_CRATE: &str = "examples/05-chain-comp/reducer";

#[test]
fn chain_compensation_refunds_failed_reservation() -> Result<()> {
    let workspace = workspace_root()?;
    let example_root = workspace.join("examples/05-chain-comp");
    ensure!(example_root.exists(), "example 05 directory missing");

    reset_journal(&example_root)?;
    let wasm_bytes = compile_reducer(&workspace.join(REDUCER_CRATE))?;
    let store = Arc::new(FsStore::open(&example_root).context("open FsStore")?);
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .context("store reducer wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

    let mut loaded = load_manifest_from_assets(&example_root)?;
    patch_module_hash(&mut loaded, &wasm_hash_ref)?;

    let mut kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        Box::new(FsJournal::open(&example_root)?),
        kernel_config(&example_root)?,
    )?;

    submit_start_event(&mut kernel)?;

    let mut harness = HttpHarness::new();

    let mut charge = harness.collect_requests(&mut kernel)?;
    assert_eq!(charge.len(), 1, "expected one charge intent");
    assert!(charge[0].params.url.contains("charge"));
    harness.respond_with(
        &mut kernel,
        charge.remove(0),
        MockHttpResponse::json(201, "{\"charge\":\"ok\"}"),
    )?;

    let mut reserve = harness.collect_requests(&mut kernel)?;
    assert_eq!(reserve.len(), 1, "expected one reserve intent");
    assert!(reserve[0].params.url.contains("reserve"));
    harness.respond_with(
        &mut kernel,
        reserve.remove(0),
        MockHttpResponse::json(503, "{\"reserve\":\"error\"}"),
    )?;

    let mut refund = harness.collect_requests(&mut kernel)?;
    assert_eq!(
        refund.len(),
        1,
        "expected refund intent after reserve failure"
    );
    assert!(refund[0].params.url.contains("refund"));
    harness.respond_with(
        &mut kernel,
        refund.remove(0),
        MockHttpResponse::json(202, "{\"refund\":\"ok\"}"),
    )?;

    assert!(harness.collect_requests(&mut kernel)?.is_empty());
    assert!(kernel.pending_plan_receipts().is_empty());

    let final_bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .context("missing reducer state")?;
    let state: ChainStateView =
        serde_cbor::from_slice(&final_bytes).context("decode reducer state")?;
    assert_eq!(state.phase, ChainPhaseView::Refunded);
    let saga = state.current_saga.context("expected active saga")?;
    assert_eq!(saga.charge_status, Some(201));
    assert_eq!(saga.reserve_status, Some(503));
    assert_eq!(saga.refund_status, Some(202));

    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .context("derive workspace root")
}

fn reset_journal(example_root: &Path) -> Result<()> {
    let journal_dir = example_root.join(".aos").join("journal");
    if journal_dir.exists() {
        fs::remove_dir_all(&journal_dir)
            .with_context(|| format!("remove {}", journal_dir.display()))?;
    }
    Ok(())
}

fn compile_reducer(source_dir: &Path) -> Result<Vec<u8>> {
    let utf_path = Utf8PathBuf::from_path_buf(source_dir.to_path_buf())
        .map_err(|_| anyhow::anyhow!("path is not utf-8: {}", source_dir.display()))?;
    let cache_dir = source_dir
        .parent()
        .context("determine example root")?
        .join(".aos")
        .join("cache")
        .join("modules");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    let mut request = BuildRequest::new(utf_path);
    request.config.release = false;
    request.cache_dir = Some(cache_dir);
    let artifact = Builder::compile(request).context("compile reducer via aos-wasm-build")?;
    Ok(artifact.wasm_bytes)
}

fn kernel_config(example_root: &Path) -> Result<KernelConfig> {
    let cache_dir = example_root.join(".aos").join("cache").join("wasmtime");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;
    Ok(KernelConfig {
        module_cache_dir: Some(cache_dir),
        eager_module_load: true,
    })
}

fn submit_start_event(kernel: &mut Kernel<FsStore>) -> Result<()> {
    let event = ChainEventEnvelope::Start {
        order_id: "ORDER-1".into(),
        customer_id: "cust-123".into(),
        amount_cents: 1999,
        reserve_sku: "sku-123".into(),
        charge: ChainTargetEnvelope {
            name: "charge".into(),
            method: "POST".into(),
            url: "https://example.com/charge".into(),
        },
        reserve: ChainTargetEnvelope {
            name: "reserve".into(),
            method: "POST".into(),
            url: "https://example.com/reserve".into(),
        },
        notify: ChainTargetEnvelope {
            name: "notify".into(),
            method: "POST".into(),
            url: "https://example.com/notify".into(),
        },
        refund: ChainTargetEnvelope {
            name: "refund".into(),
            method: "POST".into(),
            url: "https://example.com/refund".into(),
        },
    };
    let payload = serde_cbor::to_vec(&event)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn load_manifest_from_assets(example_root: &Path) -> Result<LoadedManifest> {
    let mut manifest: Option<Manifest> = None;
    let mut schemas: HashMap<Name, aos_air_types::DefSchema> = HashMap::new();
    let mut modules: HashMap<Name, aos_air_types::DefModule> = HashMap::new();
    let mut plans: HashMap<Name, aos_air_types::DefPlan> = HashMap::new();
    let mut caps: HashMap<Name, aos_air_types::DefCap> = HashMap::new();
    let mut policies: HashMap<Name, aos_air_types::DefPolicy> = HashMap::new();

    for dir in ["air", "plans"] {
        let dir_path = example_root.join(dir);
        if !dir_path.exists() {
            continue;
        }
        for path in collect_json_files(&dir_path)? {
            for node in parse_air_nodes(&path)? {
                match node {
                    AirNode::Manifest(found) => {
                        manifest = Some(found);
                    }
                    AirNode::Defschema(schema) => {
                        ensure!(
                            schemas.insert(schema.name.clone(), schema).is_none(),
                            "duplicate schema definition"
                        );
                    }
                    AirNode::Defmodule(module) => {
                        ensure!(
                            modules.insert(module.name.clone(), module).is_none(),
                            "duplicate module definition"
                        );
                    }
                    AirNode::Defplan(plan) => {
                        ensure!(
                            plans.insert(plan.name.clone(), plan).is_none(),
                            "duplicate plan definition"
                        );
                    }
                    AirNode::Defcap(cap) => {
                        ensure!(
                            caps.insert(cap.name.clone(), cap).is_none(),
                            "duplicate cap definition"
                        );
                    }
                    AirNode::Defpolicy(policy) => {
                        ensure!(
                            policies.insert(policy.name.clone(), policy).is_none(),
                            "duplicate policy definition"
                        );
                    }
                }
            }
        }
    }

    Ok(LoadedManifest {
        manifest: manifest.context("manifest node missing")?,
        modules,
        plans,
        caps,
        policies,
        schemas,
    })
}

fn collect_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry.context("walk assets directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        if matches!(path.extension().and_then(|s| s.to_str()), Some(ext) if ext.eq_ignore_ascii_case("json"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_air_nodes(path: &Path) -> Result<Vec<AirNode>> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let trimmed = data.trim_start();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        serde_json::from_str(&data).context("parse AIR node array")
    } else {
        let node: AirNode = serde_json::from_str(&data).context("parse AIR node")?;
        Ok(vec![node])
    }
}

fn patch_module_hash(loaded: &mut LoadedManifest, wasm_hash: &HashRef) -> Result<()> {
    let module = loaded
        .modules
        .get_mut(REDUCER_NAME)
        .context("module missing from manifest")?;
    module.wasm_hash = wasm_hash.clone();
    if let Some(entry) = loaded
        .manifest
        .modules
        .iter_mut()
        .find(|entry| entry.name == REDUCER_NAME)
    {
        entry.hash = wasm_hash.clone();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
enum ChainEventEnvelope {
    Start {
        order_id: String,
        customer_id: String,
        amount_cents: u64,
        reserve_sku: String,
        charge: ChainTargetEnvelope,
        reserve: ChainTargetEnvelope,
        notify: ChainTargetEnvelope,
        refund: ChainTargetEnvelope,
    },
}

#[derive(Debug, Clone, Serialize)]
struct ChainTargetEnvelope {
    name: String,
    method: String,
    url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
enum ChainPhaseView {
    Idle,
    Charging,
    Reserving,
    Notifying,
    Refunding,
    Completed,
    Refunded,
}

#[derive(Debug, Clone, Deserialize)]
struct ChainStateView {
    phase: ChainPhaseView,
    next_request_id: u64,
    current_saga: Option<ChainSagaView>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChainSagaView {
    request_id: u64,
    order_id: String,
    customer_id: String,
    amount_cents: u64,
    reserve_sku: String,
    charge_status: Option<i64>,
    reserve_status: Option<i64>,
    notify_status: Option<i64>,
    refund_status: Option<i64>,
    last_error: Option<String>,
    charge_target: ChainTargetView,
    reserve_target: ChainTargetView,
    notify_target: ChainTargetView,
    refund_target: ChainTargetView,
}

#[derive(Debug, Clone, Deserialize)]
struct ChainTargetView {
    name: String,
    method: String,
    url: String,
}

struct HttpHarness;

impl HttpHarness {
    fn new() -> Self {
        Self
    }

    fn collect_requests(
        &mut self,
        kernel: &mut Kernel<FsStore>,
    ) -> Result<Vec<HttpRequestContext>> {
        let mut out = Vec::new();
        loop {
            let intents = kernel.drain_effects();
            if intents.is_empty() {
                break;
            }
            for intent in intents {
                match intent.kind.as_str() {
                    EffectKind::HTTP_REQUEST => {
                        let params_value: ExprValue = serde_cbor::from_slice(&intent.params_cbor)
                            .context("decode http request params")?;
                        let params = http_params_from_value(params_value)?;
                        out.push(HttpRequestContext { intent, params });
                    }
                    other => {
                        anyhow::bail!("unexpected effect kind {other}");
                    }
                }
            }
        }
        Ok(out)
    }

    fn respond_with(
        &self,
        kernel: &mut Kernel<FsStore>,
        ctx: HttpRequestContext,
        response: MockHttpResponse,
    ) -> Result<()> {
        let receipt_value =
            build_http_receipt_value(response.status, &response.headers, response.body);
        let receipt = EffectReceipt {
            intent_hash: ctx.intent.intent_hash,
            adapter_id: "http.mock".into(),
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

#[derive(Debug)]
struct HttpRequestContext {
    intent: EffectIntent,
    params: HttpRequestParams,
}

#[derive(Debug, Clone)]
struct MockHttpResponse {
    status: i64,
    headers: HeaderMap,
    body: String,
}

impl MockHttpResponse {
    fn json(status: i64, body: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        Self {
            status,
            headers,
            body: body.into(),
        }
    }
}

fn http_params_from_value(value: ExprValue) -> Result<HttpRequestParams> {
    let record = match value {
        ExprValue::Record(map) => map,
        other => anyhow::bail!("http params must be a record, got {:?}", other),
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
        Some(other) => anyhow::bail!("field '{field}' must be text, got {:?}", other),
        None => anyhow::bail!("field '{field}' missing from http params"),
    }
}

fn value_to_headers(value: &ExprValue) -> Result<HeaderMap> {
    match value {
        ExprValue::Map(map) => {
            let mut headers = HeaderMap::new();
            for (key, entry) in map {
                let name = match key {
                    ValueKey::Text(text) => text.clone(),
                    other => anyhow::bail!("header key must be text, got {:?}", other),
                };
                let val = match entry {
                    ExprValue::Text(text) => text.clone(),
                    other => anyhow::bail!("header value must be text, got {:?}", other),
                };
                headers.insert(name, val);
            }
            Ok(headers)
        }
        ExprValue::Null | ExprValue::Unit => Ok(HeaderMap::new()),
        other => anyhow::bail!("headers must be a map, got {:?}", other),
    }
}

fn build_http_receipt_value(status: i64, headers: &HeaderMap, body: String) -> ExprValue {
    let mut record = indexmap::IndexMap::new();
    record.insert("status".into(), ExprValue::Int(status));
    record.insert("headers".into(), headers_to_value(headers));
    record.insert("body_preview".into(), ExprValue::Text(body));
    let mut timings = indexmap::IndexMap::new();
    timings.insert("start_ns".into(), ExprValue::Nat(10));
    timings.insert("end_ns".into(), ExprValue::Nat(20));
    record.insert("timings".into(), ExprValue::Record(timings));
    record.insert("adapter_id".into(), ExprValue::Text("http.mock".into()));
    ExprValue::Record(record)
}

fn headers_to_value(headers: &HeaderMap) -> ExprValue {
    let mut map = aos_air_exec::ValueMap::new();
    for (key, value) in headers {
        map.insert(ValueKey::Text(key.clone()), ExprValue::Text(value.clone()));
    }
    ExprValue::Map(map)
}
