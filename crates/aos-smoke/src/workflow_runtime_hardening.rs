use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use aos_air_types::HashRef;
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};
use aos_host::manifest_loader;
use aos_host::trace::workflow_trace_summary;
use aos_host::util::{is_placeholder_hash, patch_modules};
use aos_kernel::journal::mem::MemJournal;
use aos_kernel::journal::{CapDecisionOutcome, JournalRecord, PolicyDecisionOutcome};
use aos_kernel::{Kernel, KernelConfig, LoadedManifest};
use aos_store::{FsStore, Store};
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::util;

const MODULE_NAME: &str = "demo/FlowTracker@1";
const EVENT_SCHEMA: &str = "demo/RuntimeHardeningEvent@1";
const MODULE_CRATE: &str = "crates/aos-smoke/fixtures/11-plan-runtime-hardening/reducer";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum RuntimeHardeningEvent {
        Start(StartEvent),
        Approval(ApprovalEvent),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    request_id: u64,
    urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApprovalEvent {
    request_id: u64,
    approved: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct FlowState {
    completed_count: u64,
    last_request_id: Option<u64>,
    last_worker_count: Option<u64>,
}

pub fn run(example_root: &Path) -> Result<()> {
    println!("→ Workflow Runtime Hardening demo");
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

    submit_event(
        &mut kernel,
        &RuntimeHardeningEvent::Start(StartEvent {
            request_id: 1,
            urls: vec![
                "https://example.com/req-1-a".into(),
                "https://example.com/req-1-b".into(),
            ],
        }),
    )?;
    submit_event(
        &mut kernel,
        &RuntimeHardeningEvent::Start(StartEvent {
            request_id: 2,
            urls: vec![
                "https://example.com/req-2-a".into(),
                "https://example.com/req-2-b".into(),
            ],
        }),
    )?;

    let mut http = MockHttpHarness::new();
    ensure!(
        http.collect_requests(&mut kernel)?.is_empty(),
        "workers must not run before approval"
    );

    submit_event(
        &mut kernel,
        &RuntimeHardeningEvent::Approval(ApprovalEvent {
            request_id: 2,
            approved: true,
        }),
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

    submit_event(
        &mut kernel,
        &RuntimeHardeningEvent::Approval(ApprovalEvent {
            request_id: 1,
            approved: true,
        }),
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

    let summary = workflow_trace_summary(&kernel)?;
    let journal_summary = journal_counters(&kernel)?;
    ensure!(
        summary["totals"]["effects"]["intents"].as_u64() == Some(4),
        "expected 4 total http.request intents"
    );
    ensure!(
        summary["totals"]["effects"]["receipts"]["ok"].as_u64() == Some(4),
        "expected 4 successful receipts"
    );
    ensure!(
        summary["totals"]["workflows"]["failed"].as_u64() == Some(0),
        "expected zero failed workflow instances"
    );
    ensure!(
        summary["runtime_wait"]["pending_reducer_receipts"].as_u64() == Some(0),
        "expected no pending reducer receipts"
    );
    ensure!(
        summary["runtime_wait"]["queued_effects"].as_u64() == Some(0),
        "expected no queued effects"
    );

    let artifact_dir = example_root.join(".aos").join("artifacts");
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("create artifact dir {}", artifact_dir.display()))?;
    let artifact_path = artifact_dir.join("workflow-summary.json");
    fs::write(&artifact_path, serde_json::to_vec_pretty(&summary)?)
        .with_context(|| format!("write {}", artifact_path.display()))?;

    let final_entries = kernel.dump_journal()?;
    let replay_kernel = boot_kernel(
        store,
        example_root,
        &wasm_hash_ref,
        kernel_config,
        Box::new(MemJournal::from_entries(&final_entries)),
    )?;
    let replay_state_bytes = replay_kernel
        .reducer_state(MODULE_NAME)
        .ok_or_else(|| anyhow!("missing replay reducer state"))?;
    let replay_state: FlowState =
        serde_cbor::from_slice(&replay_state_bytes).context("decode replay flow state")?;
    ensure!(
        replay_state == state,
        "replay mismatch: reducer state diverged"
    );
    ensure!(
        journal_counters(&replay_kernel)? == journal_summary,
        "replay mismatch: journal counters diverged"
    );

    println!("   crash/resume: OK (pending worker receipts recovered)");
    println!("   replay parity: OK");
    println!("   workflow summary artifact: {}", artifact_path.display());

    Ok(())
}

fn submit_event(kernel: &mut Kernel<FsStore>, value: &RuntimeHardeningEvent) -> Result<()> {
    let payload = serde_cbor::to_vec(value).context("encode event")?;
    kernel
        .submit_domain_event(EVENT_SCHEMA.to_string(), payload)
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
    let mut loaded =
        manifest_loader::load_from_assets_with_imports(store.clone(), assets_root, &[])
            .context("load fixture manifest")?
            .ok_or_else(|| anyhow!("manifest missing at {}", assets_root.display()))?;

    let patched = patch_modules(&mut loaded, wasm_hash, |name, _| name == MODULE_NAME);
    if patched == 0 {
        anyhow::bail!("module '{}' missing from manifest", MODULE_NAME);
    }

    maybe_patch_sys_module(
        assets_root,
        store,
        &mut loaded,
        "sys/CapEnforceHttpOut@1",
        "cap_enforce_http_out",
    )?;

    Ok(loaded)
}

fn maybe_patch_sys_module(
    assets_root: &Path,
    store: Arc<FsStore>,
    loaded: &mut LoadedManifest,
    module_name: &str,
    bin_name: &str,
) -> Result<()> {
    let needs_patch = loaded
        .modules
        .get(module_name)
        .map(is_placeholder_hash)
        .unwrap_or(false);
    if !needs_patch {
        return Ok(());
    }

    let cache_dir = assets_root.join(".aos").join("cache").join("modules");
    let wasm_bytes =
        util::compile_wasm_bin(crate::workspace_root(), "aos-sys", bin_name, &cache_dir)?;
    let wasm_hash = store
        .put_blob(&wasm_bytes)
        .with_context(|| format!("store {module_name} wasm blob"))?;
    let wasm_hash_ref =
        HashRef::new(wasm_hash.to_hex()).with_context(|| format!("hash {module_name}"))?;
    let patched = patch_modules(loaded, &wasm_hash_ref, |name, _| name == module_name);
    if patched == 0 {
        anyhow::bail!("module '{}' missing in manifest", module_name);
    }
    Ok(())
}

fn journal_counters(kernel: &Kernel<FsStore>) -> Result<serde_json::Value> {
    let mut effect_intents = 0u64;
    let mut receipt_ok = 0u64;
    let mut receipt_error = 0u64;
    let mut receipt_timeout = 0u64;
    let mut policy_allow = 0u64;
    let mut policy_deny = 0u64;
    let mut cap_allow = 0u64;
    let mut cap_deny = 0u64;
    let mut governance_total = 0u64;

    for entry in kernel.dump_journal()? {
        let record: JournalRecord = serde_cbor::from_slice(&entry.payload)
            .with_context(|| format!("decode journal seq {}", entry.seq))?;
        match record {
            JournalRecord::EffectIntent(_) => effect_intents += 1,
            JournalRecord::EffectReceipt(receipt) => match receipt.status {
                aos_effects::ReceiptStatus::Ok => receipt_ok += 1,
                aos_effects::ReceiptStatus::Error => receipt_error += 1,
                aos_effects::ReceiptStatus::Timeout => receipt_timeout += 1,
            },
            JournalRecord::PolicyDecision(decision) => match decision.decision {
                PolicyDecisionOutcome::Allow => policy_allow += 1,
                PolicyDecisionOutcome::Deny => policy_deny += 1,
            },
            JournalRecord::CapDecision(decision) => match decision.decision {
                CapDecisionOutcome::Allow => cap_allow += 1,
                CapDecisionOutcome::Deny => cap_deny += 1,
            },
            JournalRecord::Governance(_) => governance_total += 1,
            _ => {}
        }
    }

    Ok(json!({
        "effects": {
            "intents": effect_intents,
            "receipts": {
                "ok": receipt_ok,
                "error": receipt_error,
                "timeout": receipt_timeout,
            }
        },
        "policy_decisions": {
            "allow": policy_allow,
            "deny": policy_deny,
        },
        "cap_decisions": {
            "allow": cap_allow,
            "deny": cap_deny,
        },
        "governance_records": governance_total,
    }))
}
