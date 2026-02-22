use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use aos_air_types::HashRef;
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};
use aos_host::manifest_loader;
use aos_host::trace::plan_run_summary;
use aos_host::util::patch_modules;
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use serde::{Deserialize, Serialize};

use crate::util;

const MODULE_NAME: &str = "demo/FlowTracker@1";
const START_SCHEMA: &str = "demo/RuntimeHardeningStart@1";
const APPROVAL_SCHEMA: &str = "demo/ApprovalEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/11-plan-runtime-hardening/reducer";

#[derive(Debug, Clone, Serialize)]
struct StartEvent {
    request_id: u64,
    urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ApprovalEvent {
    request_id: u64,
    approved: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct FlowState {
    completed_count: u64,
    last_request_id: Option<u64>,
    last_worker_count: Option<u64>,
}

pub fn run(example_root: &Path) -> Result<()> {
    println!("→ Plan Runtime Hardening demo");
    util::reset_journal(example_root)?;

    let wasm_bytes = util::compile_reducer(MODULE_CRATE)?;
    let store = Arc::new(FsStore::open(example_root).context("open fixture store")?);
    let wasm_hash = store.put_blob(&wasm_bytes).context("store reducer wasm")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash reducer wasm")?;
    let kernel_config = util::kernel_config(example_root)?;

    let mut kernel = boot_kernel(
        store.clone(),
        example_root,
        &wasm_hash_ref,
        kernel_config.clone(),
        Box::new(MemJournal::new()),
    )?;

    // Two concurrent starts become two independent plan instances.
    submit_event_with_key(
        &mut kernel,
        START_SCHEMA,
        &StartEvent {
            request_id: 1,
            urls: vec![
                "https://example.com/req-1-a".into(),
                "https://example.com/req-1-b".into(),
            ],
        },
        1,
    )?;
    submit_event_with_key(
        &mut kernel,
        START_SCHEMA,
        &StartEvent {
            request_id: 2,
            urls: vec![
                "https://example.com/req-2-a".into(),
                "https://example.com/req-2-b".into(),
            ],
        },
        2,
    )?;

    let mut http = MockHttpHarness::new();
    ensure!(
        http.collect_requests(&mut kernel)?.is_empty(),
        "workers must not run before approval"
    );

    // Cross-talk gate: approving request 2 only must only release request 2 workers.
    submit_event_with_key(
        &mut kernel,
        APPROVAL_SCHEMA,
        &ApprovalEvent {
            request_id: 2,
            approved: true,
        },
        2,
    )?;

    let req2_requests = http.collect_requests(&mut kernel)?;
    ensure!(
        req2_requests.len() == 2,
        "expected 2 worker requests for request_id=2, got {}",
        req2_requests.len()
    );
    ensure!(
        req2_requests
            .iter()
            .all(|ctx| ctx.params.url.contains("req-2-")),
        "approval for request_id=2 should not release request_id=1"
    );
    println!("   cross-talk gate: OK (only request_id=2 released)");

    // Crash/resume while child workers wait on receipts.
    let pre_restart_entries = kernel.dump_journal()?;
    let mut kernel = boot_kernel(
        store.clone(),
        example_root,
        &wasm_hash_ref,
        kernel_config.clone(),
        Box::new(MemJournal::from_entries(&pre_restart_entries)),
    )?;

    let mut replay_requests = http.collect_requests(&mut kernel)?;
    ensure!(
        replay_requests.len() == 2,
        "expected 2 replayed pending worker requests for request_id=2, got {}",
        replay_requests.len()
    );
    while let Some(request) = replay_requests.pop() {
        http.respond_with(
            &mut kernel,
            request,
            MockHttpResponse::json(200, "{\"ok\":true}"),
        )?;
    }

    // Now approve request 1 and complete the remaining workers.
    submit_event_with_key(
        &mut kernel,
        APPROVAL_SCHEMA,
        &ApprovalEvent {
            request_id: 1,
            approved: true,
        },
        1,
    )?;
    let mut req1_requests = http.collect_requests(&mut kernel)?;
    ensure!(
        req1_requests.len() == 2,
        "expected 2 worker requests for request_id=1, got {}",
        req1_requests.len()
    );
    ensure!(
        req1_requests
            .iter()
            .all(|ctx| ctx.params.url.contains("req-1-")),
        "approval for request_id=1 should only release request_id=1"
    );
    while let Some(request) = req1_requests.pop() {
        http.respond_with(
            &mut kernel,
            request,
            MockHttpResponse::json(200, "{\"ok\":true}"),
        )?;
    }

    let state_bytes = kernel
        .reducer_state(MODULE_NAME)
        .ok_or_else(|| anyhow!("missing reducer state"))?;
    let state: FlowState = serde_cbor::from_slice(&state_bytes).context("decode flow state")?;
    ensure!(state.completed_count == 2, "expected completed_count=2");
    ensure!(
        state.last_request_id == Some(1),
        "expected last_request_id=1"
    );
    ensure!(
        state.last_worker_count == Some(2),
        "expected worker_count=2"
    );

    let summary = plan_run_summary(&kernel)?;
    ensure!(
        summary["totals"]["runs"]["started"].as_u64() == Some(8),
        "expected 8 total starts (2 parent + 2 gate + 4 worker)"
    );
    ensure!(
        summary["totals"]["runs"]["ok"].as_u64() == Some(8),
        "expected all composed plans to complete"
    );
    ensure!(
        summary["totals"]["runs"]["error"].as_u64() == Some(0),
        "expected zero plan errors"
    );

    let artifact_dir = example_root.join(".aos").join("artifacts");
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("create artifact dir {}", artifact_dir.display()))?;
    let artifact_path = artifact_dir.join("plan-summary.json");
    fs::write(&artifact_path, serde_json::to_vec_pretty(&summary)?)
        .with_context(|| format!("write {}", artifact_path.display()))?;

    println!("   crash/resume: OK (pending worker receipts recovered)");
    println!("   plan summary artifact: {}", artifact_path.display());

    Ok(())
}

fn submit_event_with_key<T: Serialize>(
    kernel: &mut Kernel<FsStore>,
    schema: &str,
    value: &T,
    request_id: u64,
) -> Result<()> {
    let payload = serde_cbor::to_vec(value).context("encode event")?;
    kernel
        .submit_domain_event_with_key(
            schema.to_string(),
            payload,
            request_id.to_be_bytes().to_vec(),
        )
        .context("submit event")?;
    kernel.tick_until_idle().context("tick to idle")
}

fn boot_kernel(
    store: Arc<FsStore>,
    assets_root: &Path,
    wasm_hash: &HashRef,
    kernel_config: KernelConfig,
    journal: Box<dyn aos_kernel::journal::Journal>,
) -> Result<Kernel<FsStore>> {
    let loaded = load_manifest_for_runtime(store.clone(), assets_root, wasm_hash)?;
    Kernel::from_loaded_manifest_with_config(store, loaded, journal, kernel_config)
        .context("boot kernel")
}

fn load_manifest_for_runtime(
    store: Arc<FsStore>,
    assets_root: &Path,
    wasm_hash: &HashRef,
) -> Result<LoadedManifest> {
    let mut loaded = manifest_loader::load_from_assets_with_imports(store, assets_root, &[])
        .context("load fixture manifest")?
        .ok_or_else(|| anyhow!("manifest missing at {}", assets_root.display()))?;

    let patched = patch_modules(&mut loaded, wasm_hash, |name, _| name == MODULE_NAME);
    if patched == 0 {
        anyhow::bail!("module '{}' missing from manifest", MODULE_NAME);
    }
    Ok(loaded)
}
