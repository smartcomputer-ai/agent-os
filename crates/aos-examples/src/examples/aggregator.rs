use std::path::Path;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_cbor::Hash;
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Deserialize, Serialize};
use serde_cbor;

use crate::examples::http_harness::{HttpHarness, MockHttpResponse};
use crate::examples::util;
use crate::manifest_loader;

const REDUCER_NAME: &str = "demo/Aggregator@1";
const EVENT_SCHEMA: &str = "demo/AggregatorEvent@1";
const MODULE_PATH: &str = "examples/04-aggregator/reducer";

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AggregatorEventEnvelope {
    Start {
        topic: String,
        primary: AggregationTargetEnvelope,
        secondary: AggregationTargetEnvelope,
        tertiary: AggregationTargetEnvelope,
    },
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

#[derive(Debug, Clone, Serialize, Deserialize)]
enum AggregatorPcView {
    Idle,
    Running,
    Done,
}

pub fn run(example_root: &Path) -> Result<()> {
    util::reset_journal(example_root)?;
    let wasm_bytes = util::compile_reducer(MODULE_PATH)?;
    let store = std::sync::Arc::new(FsStore::open(example_root).context("open FsStore")?);
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .context("store reducer wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;

    let mut loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 04 must provide AIR JSON assets"))?;
    patch_module_hash(&mut loaded, &wasm_hash_ref)?;

    let journal = Box::new(FsJournal::open(example_root)?);
    let kernel_config = util::kernel_config(example_root)?;
    let mut kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        journal,
        kernel_config.clone(),
    )?;

    println!("→ Aggregator demo");
    submit_start(
        &mut kernel,
        AggregatorEventEnvelope::Start {
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
        },
    )?;

    let mut harness = HttpHarness::new();
    let mut requests = harness.collect_requests(&mut kernel)?;
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
    harness.respond_with(
        &mut kernel,
        ctx_b,
        MockHttpResponse::json(200, "{\"source\":\"beta\"}"),
    )?;
    harness.respond_with(
        &mut kernel,
        ctx_c,
        MockHttpResponse::json(201, "{\"source\":\"gamma\"}"),
    )?;
    harness.respond_with(
        &mut kernel,
        ctx_a,
        MockHttpResponse::json(202, "{\"source\":\"alpha\"}"),
    )?;

    let final_bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing reducer state"))?;
    let state: AggregatorStateView = serde_cbor::from_slice(&final_bytes)?;
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

    drop(kernel);

    let mut replay_loaded = manifest_loader::load_from_assets(store.clone(), example_root)?
        .ok_or_else(|| anyhow!("example 04 must provide AIR JSON assets"))?;
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

fn submit_start(kernel: &mut Kernel<FsStore>, event: AggregatorEventEnvelope) -> Result<()> {
    match &event {
        AggregatorEventEnvelope::Start { topic, .. } => {
            println!("     aggregate start → topic={topic}");
        }
    }
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
