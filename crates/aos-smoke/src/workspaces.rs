use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, ensure};
use aos_wasm_sdk::aos_variant;
use serde::{Deserialize, Serialize};

use crate::example_host::{ExampleHost, HarnessConfig};

const REDUCER_NAME: &str = "demo/WorkspaceDemo@1";
const EVENT_SCHEMA: &str = "demo/WorkspaceEvent@1";
const MODULE_PATH: &str = "crates/aos-smoke/fixtures/09-workspaces/reducer";

aos_variant! {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum WorkspaceEvent {
        Start(WorkspaceStart),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceStart {
    workspaces: Vec<String>,
    owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceStateView {
    workspaces: BTreeMap<String, WorkspaceSummaryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSummaryView {
    version: Option<u64>,
    root_hash: String,
    entry_count: u64,
    diff_count: u64,
}

pub fn run(example_root: &Path) -> Result<()> {
    let mut host = ExampleHost::prepare(HarnessConfig {
        example_root,
        assets_root: None,
        reducer_name: REDUCER_NAME,
        event_schema: EVENT_SCHEMA,
        module_crate: MODULE_PATH,
    })?;

    println!("→ Workspaces demo");
    let start = WorkspaceEvent::Start(WorkspaceStart {
        workspaces: vec!["alpha".into(), "beta".into()],
        owner: "demo".into(),
    });
    println!("     seed workspaces: alpha, beta");
    host.send_event(&start)?;
    drain_internal_effects(&mut host)?;

    let state: WorkspaceStateView = host.read_state()?;
    println!("   seeded {} workspaces", state.workspaces.len());
    ensure!(
        state.workspaces.contains_key("alpha"),
        "missing alpha workspace summary"
    );
    ensure!(
        state.workspaces.contains_key("beta"),
        "missing beta workspace summary"
    );
    for (name, summary) in &state.workspaces {
        ensure!(
            summary.root_hash.starts_with("sha256:"),
            "workspace {name} has invalid root hash {}",
            summary.root_hash
        );
        ensure!(
            summary.entry_count > 0,
            "workspace {name} should contain seeded entries"
        );
        println!(
            "     {name}: version={:?} entries={} diffs={} root={}",
            summary.version, summary.entry_count, summary.diff_count, summary.root_hash
        );
    }

    host.finish()?.verify_replay()?;
    Ok(())
}

fn drain_internal_effects(host: &mut ExampleHost) -> Result<()> {
    let mut safety = 0;
    loop {
        if host.kernel_mut().queued_effects_snapshot().is_empty() {
            break;
        }
        host.run_cycle_batch()?;
        safety += 1;
        ensure!(safety < 64, "safety trip: workspace workflow did not drain");
    }
    Ok(())
}
