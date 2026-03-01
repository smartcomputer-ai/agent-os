#![cfg(feature = "e2e-tests")]

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use aos_air_types::HashRef;
use aos_host::config::HostConfig;
use aos_host::host::WorldHost;
use aos_host::manifest_loader;
use aos_host::testhost::TestHost;
use aos_kernel::journal::{JournalKind, JournalRecord};
use aos_kernel::{Kernel, KernelConfig};
use aos_store::{FsStore, Store};
use aos_wasm_build::builder::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use serde_json::{Value, json};

const DEMIURGE_WORKFLOW: &str = "demiurge/Demiurge@1";
const SESSION_ID: &str = "22222222-2222-2222-2222-222222222222";
const TOOL_CALL_ID: &str = "call_1";

fn load_world_env(world_root: &Path) -> Result<()> {
    let env_path = world_root.join(".env");
    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).context("load .env")? {
            let (key, val) = item?;
            if std::env::var_os(&key).is_none() {
                unsafe {
                    std::env::set_var(&key, &val);
                }
            }
        }
    }
    Ok(())
}

fn tool_request_event_hash(kernel: &aos_kernel::Kernel<FsStore>, call_id: &str) -> Result<String> {
    let entries = kernel.dump_journal().context("dump journal")?;
    for entry in entries {
        if entry.kind != JournalKind::DomainEvent {
            continue;
        }
        let record: JournalRecord =
            serde_cbor::from_slice(&entry.payload).context("decode journal record")?;
        let JournalRecord::DomainEvent(domain) = record else {
            continue;
        };
        if domain.schema != "demiurge/ToolCallRequested@1" {
            continue;
        }
        let Ok(value) = serde_cbor::from_slice::<serde_json::Value>(&domain.value) else {
            continue;
        };
        let same_call = value.get("call_id").and_then(|v| v.as_str()) == Some(call_id);
        if same_call && !domain.event_hash.is_empty() {
            return Ok(domain.event_hash);
        }
    }
    anyhow::bail!("missing tool request root event hash for call_id={call_id}");
}

#[derive(Debug)]
struct TraceAssertions {
    terminal_state: &'static str,
    waiting_receipt_count: usize,
    waiting_event_count: usize,
    policy_denied: bool,
    cap_denied: bool,
    has_receipt_error: bool,
    has_receipt_timeout: bool,
    has_plan_error: bool,
}

fn analyze_trace(
    kernel: &aos_kernel::Kernel<FsStore>,
    event_hash: &str,
) -> Result<TraceAssertions> {
    let entries = kernel.dump_journal().context("dump journal")?;
    let root_seq = entries
        .iter()
        .find_map(|entry| {
            if entry.kind != JournalKind::DomainEvent {
                return None;
            }
            let record: JournalRecord = serde_cbor::from_slice(&entry.payload).ok()?;
            match record {
                JournalRecord::DomainEvent(domain) if domain.event_hash == event_hash => {
                    Some(entry.seq)
                }
                _ => None,
            }
        })
        .ok_or_else(|| anyhow::anyhow!("trace root event hash not found: {event_hash}"))?;

    let mut has_window_entries = false;
    let mut policy_denied = false;
    let mut cap_denied = false;
    let mut has_receipt_error = false;
    let mut has_receipt_timeout = false;
    let mut has_workflow_error = false;
    for entry in entries.into_iter().filter(|entry| entry.seq >= root_seq) {
        let record: JournalRecord =
            serde_cbor::from_slice(&entry.payload).context("decode trace window record")?;
        has_window_entries = true;
        match record {
            JournalRecord::PolicyDecision(policy) => {
                if matches!(
                    policy.decision,
                    aos_kernel::journal::PolicyDecisionOutcome::Deny
                ) {
                    policy_denied = true;
                }
            }
            JournalRecord::CapDecision(cap) => {
                if matches!(cap.decision, aos_kernel::journal::CapDecisionOutcome::Deny) {
                    cap_denied = true;
                }
            }
            JournalRecord::EffectReceipt(receipt) => match receipt.status {
                aos_effects::ReceiptStatus::Error => has_receipt_error = true,
                aos_effects::ReceiptStatus::Timeout => has_receipt_timeout = true,
                aos_effects::ReceiptStatus::Ok => {}
            },
            JournalRecord::Custom(custom) => {
                if custom.tag == "workflow_error" {
                    has_workflow_error = true;
                }
            }
            _ => {}
        }
    }

    let workflow_instances = kernel.workflow_instances_snapshot();
    let waiting_receipt_count = kernel.pending_workflow_receipts_snapshot().len()
        + kernel.queued_effects_snapshot().len()
        + workflow_instances
            .iter()
            .map(|instance| instance.inflight_intents.len())
            .sum::<usize>();
    let waiting_event_count = workflow_instances
        .iter()
        .filter(|instance| {
            !matches!(
                instance.status,
                aos_kernel::snapshot::WorkflowStatusSnapshot::Completed
                    | aos_kernel::snapshot::WorkflowStatusSnapshot::Failed
            )
        })
        .count();

    let terminal_state = if has_receipt_error || has_receipt_timeout || has_workflow_error {
        "failed"
    } else if waiting_receipt_count > 0 {
        "waiting_receipt"
    } else if waiting_event_count > 0 {
        "waiting_event"
    } else if has_window_entries {
        "completed"
    } else {
        "unknown"
    };

    Ok(TraceAssertions {
        terminal_state,
        waiting_receipt_count,
        waiting_event_count,
        policy_denied,
        cap_denied,
        has_receipt_error,
        has_receipt_timeout,
        has_plan_error: has_workflow_error,
    })
}

async fn run_until_idle(host: &mut TestHost<FsStore>, max_cycles: usize) -> Result<()> {
    for _ in 0..max_cycles {
        let outcome = host.run_cycle_batch().await?;
        if outcome.effects_dispatched == 0 && outcome.receipts_applied == 0 {
            break;
        }
    }
    Ok(())
}

fn send_session_event(
    host: &mut TestHost<FsStore>,
    step_epoch: u64,
    event_kind: Value,
) -> Result<()> {
    host.send_event(
        "aos.agent/SessionEvent@1",
        json!({
            "session_id": SESSION_ID,
            "run_id": null,
            "turn_id": null,
            "step_id": null,
            "session_epoch": 0,
            "step_epoch": step_epoch,
            "event": event_kind,
        }),
    )?;
    Ok(())
}

fn workflow_state_json(host: &TestHost<FsStore>) -> Result<Value> {
    let state_bytes = host
        .kernel()
        .workflow_state_bytes(DEMIURGE_WORKFLOW, None)
        .context("load workflow state")?
        .ok_or_else(|| anyhow::anyhow!("missing workflow state"))?;
    serde_cbor::from_slice(&state_bytes).context("decode workflow state json")
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "P2: plan-driven demiurge fixture retired; replace with workflow-native fixture"]
async fn demiurge_introspect_manifest_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir().context("tempdir")?;
    let store = Arc::new(FsStore::open(tmp.path()).context("open store")?);

    let asset_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../worlds/demiurge");
    load_world_env(&asset_root).context("load demiurge .env")?;
    let asset_root = asset_root.as_path();

    let sdk_air_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aos-agent/air");
    let import_roots = vec![sdk_air_root];
    let mut loaded =
        manifest_loader::load_from_assets_with_imports(store.clone(), asset_root, &import_roots)
            .context("load demiurge assets")?
            .context("missing demiurge manifest")?;

    let workflow_root = asset_root.join("workflow");
    let workflow_dir =
        Utf8PathBuf::from_path_buf(workflow_root.to_path_buf()).expect("utf8 workflow path");
    let mut request = BuildRequest::new(workflow_dir);
    request.config.release = false;
    let artifact = Builder::compile(request).context("compile demiurge workflow")?;
    let wasm_hash = store
        .put_blob(&artifact.wasm_bytes)
        .context("store workflow wasm")?;

    let module = loaded
        .modules
        .get_mut(DEMIURGE_WORKFLOW)
        .expect("demiurge module");
    module.wasm_hash = HashRef::new(wasm_hash.to_hex()).expect("wasm hash ref");

    let kernel_config = KernelConfig {
        allow_placeholder_secrets: true,
        ..KernelConfig::default()
    };
    let kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        Box::new(aos_kernel::journal::mem::MemJournal::new()),
        kernel_config,
    )
    .context("build kernel")?;
    let world = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());
    let mut host = TestHost::from_world_host(world);

    let prompt_bytes = std::fs::read(asset_root.join("agent-ws/prompts/packs/default.json"))
        .context("read prompt file")?;
    let prompt_hash = store
        .put_blob(&prompt_bytes)
        .context("store prompt blob")?
        .to_hex();

    let mut step_epoch = 1_u64;

    send_session_event(
        &mut host,
        step_epoch,
        json!({
            "$tag": "RunRequested",
            "$value": {
                "input_ref": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "run_overrides": {
                    "provider": "mock",
                    "model": "gpt-mock",
                    "reasoning_effort": null,
                    "max_tokens": 256,
                    "workspace_binding": null,
                    "default_prompt_pack": null,
                    "default_prompt_refs": [prompt_hash],
                    "default_tool_profile": "openai",
                    "default_tool_enable": ["host.session.open"],
                    "default_tool_disable": null,
                    "default_tool_force": null
                }
            }
        }),
    )?;
    step_epoch += 1;
    run_until_idle(&mut host, 8).await?;

    let state = workflow_state_json(&host)?;
    let active_run_config = state
        .get("active_run_config")
        .and_then(Value::as_object)
        .context("missing active_run_config")?;
    let prompt_refs = active_run_config
        .get("prompt_refs")
        .and_then(Value::as_array)
        .context("missing prompt_refs")?;
    let tool_profile = active_run_config
        .get("tool_profile")
        .and_then(Value::as_str)
        .context("missing tool_profile")?;
    assert_eq!(prompt_refs.len(), 1);
    assert_eq!(tool_profile, "openai");
    assert!(
        active_run_config
            .get("workspace_binding")
            .unwrap_or(&Value::Null)
            .is_null()
    );

    let step_id = state
        .get("active_step_id")
        .cloned()
        .context("missing active_step_id")?;
    let run_id = state.get("active_run_id").cloned().unwrap_or(Value::Null);
    let turn_id = state.get("active_turn_id").cloned().unwrap_or(Value::Null);
    let active_step_epoch = state.get("step_epoch").and_then(Value::as_u64).unwrap_or(0);

    send_session_event(
        &mut host,
        step_epoch,
        json!({
            "$tag": "ToolCallsObserved",
            "$value": {
                "intent_id": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "params_hash": null,
                "calls": [{
                  "call_id": TOOL_CALL_ID,
                  "tool_name": "host.session.open",
                  "arguments_ref": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                  "provider_call_id": null
                }]
            }
        }),
    )?;
    step_epoch += 1;

    host.send_event(
        "demiurge/ToolCallRequested@1",
        json!({
            "session_id": SESSION_ID,
            "run_id": run_id,
            "turn_id": turn_id,
            "step_id": step_id,
            "session_epoch": 0,
            "step_epoch": active_step_epoch,
            "tool_batch_id": { "step_id": state.get("active_step_id").cloned().unwrap_or(Value::Null), "batch_seq": 1 },
            "call_id": TOOL_CALL_ID,
            "finalize_batch": true,
            "params": {
                "$tag": "IntrospectManifest",
                "$value": { "consistency": "head" }
            }
        }),
    )?;

    run_until_idle(&mut host, 16).await?;

    let state = workflow_state_json(&host)?;
    let batch = state
        .get("active_tool_batch")
        .and_then(Value::as_object)
        .context("missing active_tool_batch")?;
    let call_status = batch
        .get("call_status")
        .and_then(Value::as_object)
        .and_then(|m| m.get(TOOL_CALL_ID))
        .and_then(Value::as_object)
        .and_then(|v| v.get("$tag"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_eq!(call_status, "Succeeded");
    assert!(
        batch.get("results_ref").unwrap_or(&Value::Null).is_null(),
        "expected null results_ref for plan-driven tool settlement"
    );

    let root_hash = tool_request_event_hash(host.kernel(), TOOL_CALL_ID)?;
    let trace = analyze_trace(host.kernel(), &root_hash)?;
    assert_eq!(
        trace.terminal_state, "completed",
        "trace terminal state should be completed for successful flow: {:?}",
        trace
    );
    assert_eq!(
        trace.waiting_receipt_count, 0,
        "trace should not have pending receipt waits at end: {:?}",
        trace
    );
    assert_eq!(
        trace.waiting_event_count, 0,
        "trace should not have pending event waits at end: {:?}",
        trace
    );
    assert!(
        !trace.policy_denied,
        "unexpected policy deny in trace: {:?}",
        trace
    );
    assert!(
        !trace.cap_denied,
        "unexpected capability deny in trace: {:?}",
        trace
    );
    assert!(
        !trace.has_receipt_error,
        "unexpected error receipt in trace: {:?}",
        trace
    );
    assert!(
        !trace.has_receipt_timeout,
        "unexpected timeout receipt in trace: {:?}",
        trace
    );
    assert!(
        !trace.has_plan_error,
        "unexpected plan error in trace: {:?}",
        trace
    );

    send_session_event(&mut host, step_epoch, json!({ "$tag": "StepBoundary" }))?;
    step_epoch += 1;
    send_session_event(&mut host, step_epoch, json!({ "$tag": "RunCompleted" }))?;
    run_until_idle(&mut host, 8).await?;

    Ok(())
}
