use std::path::Path;

use anyhow::{Context, Result, anyhow};
use aos_air_types::HashRef;
use aos_kernel::error::KernelError;
use aos_kernel::governance::ManifestPatch;
use aos_kernel::shadow::ShadowHarness;
use aos_kernel::snapshot::WorkflowStatusSnapshot;
use aos_store::Store;
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};
use crate::util;
use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};
use aos_host::manifest_loader;

const WORKFLOW_NAME_V1: &str = "demo/SafeUpgrade@1";
const WORKFLOW_NAME_V2: &str = "demo/SafeUpgrade@2";
const EVENT_SCHEMA: &str = "demo/SafeUpgradeEvent@1";
const MODULE_PATH_V1: &str = "crates/aos-smoke/fixtures/06-safe-upgrade/workflow";
const MODULE_PATH_V2: &str = "crates/aos-smoke/fixtures/06-safe-upgrade/workflow-v2";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum UpgradeEventEnvelope {
        Start { url: String },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpgradeStateView {
    pc: UpgradePcView,
    pending_request: Option<u64>,
    primary_status: Option<i64>,
    follow_status: Option<i64>,
    requests_observed: u64,
}

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    enum UpgradePcView {
        Idle,
        Fetching,
        Completed,
    }
}

pub fn run(example_root: &Path) -> Result<()> {
    let assets_root = example_root.join("air.v1");
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: Some(assets_root.as_path()),
        workflow_name: WORKFLOW_NAME_V1,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH_V1,
    })?;

    println!("→ Safe upgrade demo");
    let start_event = UpgradeEventEnvelope::Start {
        url: "https://example.com/data.json".into(),
    };
    println!("   start v1 fetch → url={}", url_for(&start_event));
    host.send_event(&start_event)?;

    let mut http = MockHttpHarness::new();
    let mut requests = http.collect_requests(host.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected a single HTTP intent before upgrade, saw {}",
            requests.len()
        ));
    }
    let primary = requests.pop().expect("one request");
    println!(
        "   v1 http.request {} {} (holding receipt)",
        primary.params.method, primary.params.url
    );

    host.kernel_mut().create_snapshot()?;
    let snapshot_hash = host
        .kernel_mut()
        .snapshot_hash()
        .ok_or_else(|| anyhow!("snapshot hash missing after create_snapshot"))?;
    println!(
        "   snapshot created while waiting: {}",
        snapshot_hash.to_hex()
    );

    let waiting_instances = host
        .kernel_mut()
        .workflow_instances_snapshot()
        .into_iter()
        .filter(|instance| instance.status == WorkflowStatusSnapshot::Waiting)
        .count();
    if waiting_instances == 0 {
        return Err(anyhow!(
            "expected waiting workflow instance after snapshot before upgrade"
        ));
    }

    let proposal_patch = load_upgrade_patch(example_root, &host)?;
    let proposal_id = host.kernel_mut().submit_proposal(
        proposal_patch,
        Some("upgrade to SafeUpgrade workflow v2".into()),
    )?;
    let summary = host.kernel_mut().run_shadow(
        proposal_id,
        Some(ShadowHarness {
            seed_events: vec![],
        }),
    )?;

    println!(
        "   shadow: {} predicted effect(s), {} workflow instance(s), {} ledger delta(s)",
        summary.predicted_effects.len(),
        summary.workflow_instances.len(),
        summary.ledger_deltas.len()
    );

    host.kernel_mut()
        .approve_proposal(proposal_id, "demo-approver")?;
    let blocked = host.kernel_mut().apply_proposal(proposal_id);
    match blocked {
        Err(KernelError::ManifestApplyBlockedInFlight {
            plan_instances,
            waiting_events,
            pending_plan_receipts,
            pending_workflow_receipts,
            queued_effects,
            workflow_queue_pending,
        }) => {
            println!(
                "   apply blocked (strict-quiescence): workflows={} waiting={} pending_plan_receipts={} pending_workflow_receipts={} queued_effects={} workflow_queue_pending={}",
                plan_instances,
                waiting_events,
                pending_plan_receipts,
                pending_workflow_receipts,
                queued_effects,
                workflow_queue_pending
            );
        }
        Err(other) => return Err(anyhow!("expected strict-quiescence block, got: {other}")),
        Ok(()) => {
            return Err(anyhow!(
                "expected strict-quiescence apply block while receipt was pending"
            ));
        }
    }

    println!("   delivering late receipt to settle v1 workflow");
    http.respond_with(
        host.kernel_mut(),
        primary,
        MockHttpResponse::json(200, "{\"demo\":true}"),
    )?;

    let state_v1: UpgradeStateView = host.read_state()?;
    println!(
        "   v1 post-receipt: pending={:?} primary_status={:?} follow_status={:?} requests={}",
        state_v1.pending_request,
        state_v1.primary_status,
        state_v1.follow_status,
        state_v1.requests_observed
    );
    if state_v1.pc != UpgradePcView::Completed
        || state_v1.pending_request.is_some()
        || state_v1.primary_status != Some(200)
        || state_v1.follow_status.is_some()
        || state_v1.requests_observed != 1
    {
        return Err(anyhow!(
            "unexpected v1 state after late receipt continuation: {:?}",
            state_v1
        ));
    }

    host.kernel_mut().apply_proposal(proposal_id)?;
    println!("   applied manifest hash {}", summary.manifest_hash);

    println!("   start v2 fetch → url={}", url_for(&start_event));
    host.send_event(&start_event)?;
    let mut upgraded_requests = http.collect_requests(host.kernel_mut())?;
    if upgraded_requests.len() != 1 {
        return Err(anyhow!(
            "expected primary HTTP intent after upgrade, saw {}",
            upgraded_requests.len()
        ));
    }
    let primary_v2 = upgraded_requests.pop().expect("primary request");
    println!(
        "   v2 http.request {} {}",
        primary_v2.params.method, primary_v2.params.url
    );
    http.respond_with(
        host.kernel_mut(),
        primary_v2,
        MockHttpResponse::json(201, "{\"demo\":true,\"call\":1}"),
    )?;

    let mut followups = http.collect_requests(host.kernel_mut())?;
    if followups.len() != 1 {
        return Err(anyhow!(
            "expected follow-up HTTP intent after upgrade, saw {}",
            followups.len()
        ));
    }
    let follow = followups.pop().expect("follow-up request");
    println!(
        "   v2 http.request {} {}",
        follow.params.method, follow.params.url
    );
    http.respond_with(
        host.kernel_mut(),
        follow,
        MockHttpResponse::json(202, "{\"demo\":true,\"call\":2}"),
    )?;

    let state_v2_bytes = host
        .kernel_mut()
        .workflow_state(WORKFLOW_NAME_V2)
        .ok_or_else(|| anyhow!("missing state for upgraded module '{WORKFLOW_NAME_V2}'"))?;
    let state_v2: UpgradeStateView =
        serde_cbor::from_slice(&state_v2_bytes).context("decode upgraded workflow state")?;
    println!(
        "   v2 complete: pending={:?} primary_status={:?} follow_status={:?} requests={}",
        state_v2.pending_request,
        state_v2.primary_status,
        state_v2.follow_status,
        state_v2.requests_observed
    );
    if state_v2.pc != UpgradePcView::Completed
        || state_v2.pending_request.is_some()
        || state_v2.primary_status != Some(201)
        || state_v2.follow_status != Some(202)
        || state_v2.requests_observed != 2
    {
        return Err(anyhow!("unexpected v2 state after upgrade: {:?}", state_v2));
    }

    host.finish()?.verify_replay()?;
    Ok(())
}

fn load_upgrade_patch(example_root: &Path, host: &ExampleHost) -> Result<ManifestPatch> {
    let upgrade_root = example_root.join("air.v2");
    let mut loaded = manifest_loader::load_from_assets(host.store(), &upgrade_root)?
        .ok_or_else(|| anyhow!("upgrade manifest missing at {}", upgrade_root.display()))?;

    let wasm_bytes = util::compile_workflow(MODULE_PATH_V2)?;
    let wasm_hash = host
        .store()
        .put_blob(&wasm_bytes)
        .context("store v2 workflow wasm blob")?;
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).context("hash v2 workflow wasm")?;
    let patched = aos_host::util::patch_modules(&mut loaded, &wasm_hash_ref, |name, _| {
        name == WORKFLOW_NAME_V2
    });
    if patched == 0 {
        return Err(anyhow!(
            "module '{WORKFLOW_NAME_V2}' missing from v2 manifest"
        ));
    }

    Ok(manifest_loader::manifest_patch_from_loaded(&loaded))
}

fn url_for(event: &UpgradeEventEnvelope) -> &str {
    match event {
        UpgradeEventEnvelope::Start { url } => url.as_str(),
    }
}
