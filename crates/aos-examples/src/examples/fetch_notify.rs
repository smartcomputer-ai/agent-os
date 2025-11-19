use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::support::http_harness::{HttpHarness, MockHttpResponse};
use crate::support::manifest_loader;
use crate::support::util;

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

    let mut http = HttpHarness::new();
    let requests = http.collect_requests(&mut kernel)?;
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
    http.respond_with(&mut kernel, request, MockHttpResponse::json(200, body))?;

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
