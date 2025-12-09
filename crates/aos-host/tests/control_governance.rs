//! Control channel governance integration: propose -> shadow -> approve -> apply

use aos_host::WorldHost;
use aos_host::config::HostConfig;
use aos_host::fixtures::{self, TestWorld};
use aos_host::manifest_loader::manifest_patch_from_loaded;
use aos_host::modes::daemon::{ControlMsg, WorldDaemon};
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use tokio::sync::{broadcast, mpsc, oneshot};

#[path = "helpers.rs"]
mod helpers;
use helpers::simple_state_manifest;

#[tokio::test]
async fn control_governance_propose_shadow_apply_flow() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let kernel = Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new()))
        .expect("kernel");
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(8);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let mut daemon = WorldDaemon::new(host, control_rx, shutdown_rx, None);
    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    // Build a tiny patch: add a defschema and set manifest refs to include it.
    let base_loaded = simple_state_manifest(&store);
    let patch = manifest_patch_from_loaded(&base_loaded);
    let _new_world = TestWorld::with_store(store.clone(), simple_state_manifest(&store)).unwrap();

    // Propose
    let (resp_tx, resp_rx) = oneshot::channel();
    control_tx
        .send(ControlMsg::Propose {
            patch: aos_host::modes::daemon::GovernancePatchInput::Manifest(patch),
            description: Some("ctrl gov test".into()),
            resp: resp_tx,
        })
        .await
        .unwrap();
    let proposal_id = resp_rx.await.unwrap().expect("propose ok");

    // Shadow
    let (shadow_tx, shadow_rx) = oneshot::channel();
    control_tx
        .send(ControlMsg::Shadow {
            proposal_id,
            resp: shadow_tx,
        })
        .await
        .unwrap();
    let summary = shadow_rx.await.unwrap().expect("shadow ok");
    assert_eq!(summary.manifest_hash.len() > 0, true);

    // Approve
    let (approve_tx, approve_rx) = oneshot::channel();
    control_tx
        .send(ControlMsg::Approve {
            proposal_id,
            approver: "test-approver".into(),
            decision: aos_kernel::journal::ApprovalDecisionRecord::Approve,
            resp: approve_tx,
        })
        .await
        .unwrap();
    approve_rx.await.unwrap().expect("approve ok");

    // Apply
    let (apply_tx, apply_rx) = oneshot::channel();
    control_tx
        .send(ControlMsg::Apply {
            proposal_id,
            resp: apply_tx,
        })
        .await
        .unwrap();
    apply_rx.await.unwrap().expect("apply ok");

    // Shutdown daemon loop
    shutdown_tx.send(()).unwrap();
    let _ = daemon_handle.await.unwrap();
}
