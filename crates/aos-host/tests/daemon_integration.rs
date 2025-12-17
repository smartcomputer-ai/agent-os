//! Integration test covering the daemon timer path end-to-end.

use std::sync::Arc;
use std::time::Duration;

use aos_host::config::HostConfig;
use aos_host::fixtures::{self, START_SCHEMA, TestStore};
use aos_host::modes::daemon::{ControlMsg, WorldDaemon};
use aos_host::{ExternalEvent, WorldHost};
use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use tokio::sync::{broadcast, mpsc, oneshot};

// Re-use shared helpers defined for other integration tests.
#[path = "helpers.rs"]
mod helpers;

use helpers::timer_manifest;

/// Ensure a reducer-emitted `timer.set` is scheduled, fired by the daemon,
/// and routed to the handler reducer.
#[tokio::test]
async fn daemon_fires_timer_and_routes_event() {
    // Build in-memory world with timer-emitting reducer + handler.
    let store: Arc<TestStore> = fixtures::new_mem_store();
    let manifest = timer_manifest(&store);
    let kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();
    let host = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());

    let (control_tx, control_rx) = mpsc::channel(8);
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Spawn daemon; it returns itself so we can inspect final state.
    let mut daemon = WorldDaemon::new(host, control_rx, shutdown_rx, None);
    let handle = tokio::spawn(async move { (daemon.run().await, daemon) });

    // Kick off the reducer that emits timer.set.
    let start_value = serde_cbor::to_vec(&serde_json::json!({ "id": "t1" })).unwrap();
    let (resp_tx, resp_rx) = oneshot::channel();
    control_tx
        .send(ControlMsg::EventSend {
            event: ExternalEvent::DomainEvent {
                schema: START_SCHEMA.into(),
                value: start_value,
                key: None,
            },
            resp: resp_tx,
        })
        .await
        .unwrap();
    let _ = resp_rx.await;

    // Allow the daemon loop to schedule and fire the timer.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Request shutdown to let the daemon exit cleanly.
    shutdown_tx.send(()).unwrap();

    let (result, daemon) = handle.await.unwrap();
    result.unwrap();

    // Timer handler should have been invoked, setting its stub state to 0xCC.
    let state = daemon
        .host()
        .kernel()
        .reducer_state("com.acme/TimerHandler@1");
    assert_eq!(state, Some(&vec![0xCC]));
}
