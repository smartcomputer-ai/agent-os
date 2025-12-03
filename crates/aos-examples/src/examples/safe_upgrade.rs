use std::path::Path;

use anyhow::{Result, anyhow};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::shadow::ShadowHarness;
use serde::{Deserialize, Serialize};
use serde_cbor;

use aos_host::adapters::mock::{MockHttpHarness, MockHttpResponse};
use crate::support::manifest_loader;
use crate::support::reducer_harness::{ExampleReducerHarness, HarnessConfig};

const REDUCER_NAME: &str = "demo/SafeUpgrade@1";
const EVENT_SCHEMA: &str = "demo/SafeUpgradeEvent@1";
const MODULE_PATH: &str = "examples/06-safe-upgrade/reducer";

#[derive(Debug, Clone, Serialize, Deserialize)]
enum UpgradeEventEnvelope {
    Start {
        url: String,
    },
    NotifyComplete {
        primary_status: i64,
        follow_status: i64,
        request_count: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpgradeStateView {
    pc: UpgradePcView,
    pending_request: Option<u64>,
    primary_status: Option<i64>,
    follow_status: Option<i64>,
    requests_observed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum UpgradePcView {
    Idle,
    Fetching,
    Completed,
}

pub fn run(example_root: &Path) -> Result<()> {
    let assets_root = example_root.join("air.v1");
    let harness = ExampleReducerHarness::prepare(HarnessConfig {
        example_root,
        assets_root: Some(assets_root.as_path()),
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;
    let mut run = harness.start()?;

    println!("→ Safe upgrade demo");
    let start_event = UpgradeEventEnvelope::Start {
        url: "https://example.com/data.json".into(),
    };
    println!("   start v1 fetch → url={}", url_for(&start_event));
    run.submit_event(&start_event)?;

    let mut http = MockHttpHarness::new();
    let mut requests = http.collect_requests(run.kernel_mut())?;
    if requests.len() != 1 {
        return Err(anyhow!(
            "expected a single HTTP intent before upgrade, saw {}",
            requests.len()
        ));
    }
    let primary = requests.pop().expect("one request");
    println!(
        "   v1 http.request {} {}",
        primary.params.method, primary.params.url
    );
    http.respond_with(
        run.kernel_mut(),
        primary,
        MockHttpResponse::json(200, "{\"demo\":true}"),
    )?;

    let state_v1: UpgradeStateView = run.read_state()?;
    println!(
        "   v1 complete: pending={:?} primary_status={:?} follow_status={:?} requests={}",
        state_v1.pending_request,
        state_v1.primary_status,
        state_v1.follow_status,
        state_v1.requests_observed
    );

    let proposal_patch = load_upgrade_patch(example_root, &harness)?;
    let proposal_id = run
        .kernel_mut()
        .submit_proposal(proposal_patch, Some("upgrade to fetch_plan@2".into()))?;
    let seed_event = serde_cbor::to_vec(&start_event)?;
    let summary = run.kernel_mut().run_shadow(
        proposal_id,
        Some(ShadowHarness {
            seed_events: vec![(EVENT_SCHEMA.to_string(), seed_event)],
        }),
    )?;

    println!(
        "   shadow: {} predicted effect(s), {} plan result(s), {} ledger delta(s)",
        summary.predicted_effects.len(),
        summary.plan_results.len(),
        summary.ledger_deltas.len()
    );
    for delta in &summary.ledger_deltas {
        println!("     delta: {:?} {:?}", delta.ledger, delta.name);
    }

    run.kernel_mut()
        .approve_proposal(proposal_id, "demo-approver")?;
    run.kernel_mut().apply_proposal(proposal_id)?;
    println!("   applied manifest hash {}", summary.manifest_hash);

    println!("   start v2 fetch → url={}", url_for(&start_event));
    run.submit_event(&start_event)?;
    let mut upgraded_requests = http.collect_requests(run.kernel_mut())?;
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
        run.kernel_mut(),
        primary_v2,
        MockHttpResponse::json(201, "{\"demo\":true,\"call\":1}"),
    )?;

    let mut followups = http.collect_requests(run.kernel_mut())?;
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
        run.kernel_mut(),
        follow,
        MockHttpResponse::json(202, "{\"demo\":true,\"call\":2}"),
    )?;

    let state_v2: UpgradeStateView = run.read_state()?;
    println!(
        "   v2 complete: pending={:?} primary_status={:?} follow_status={:?} requests={}",
        state_v2.pending_request,
        state_v2.primary_status,
        state_v2.follow_status,
        state_v2.requests_observed
    );

    run.finish()?.verify_replay()?;
    Ok(())
}

fn load_upgrade_patch(
    example_root: &Path,
    harness: &ExampleReducerHarness,
) -> Result<ManifestPatch> {
    let upgrade_root = example_root.join("air.v2");
    let mut loaded = manifest_loader::load_from_assets(harness.store(), &upgrade_root)?
        .ok_or_else(|| anyhow!("upgrade manifest missing at {}", upgrade_root.display()))?;
    harness.patch_module_hash(&mut loaded)?;
    Ok(manifest_loader::manifest_patch_from_loaded(&loaded))
}

fn url_for(event: &UpgradeEventEnvelope) -> &str {
    match event {
        UpgradeEventEnvelope::Start { url } => url.as_str(),
        UpgradeEventEnvelope::NotifyComplete { .. } => "",
    }
}
