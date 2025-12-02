use std::fs::File;
use std::io::Write;
use std::sync::Arc;

use aos_host::testhost::TestHost;
use aos_store::MemStore;
use serde_json::json;
use tempfile::TempDir;

fn write_minimal_manifest(path: &std::path::Path) {
    let manifest = json!({
        "air_version": "1",
        "schemas": [],
        "modules": [],
        "plans": [],
        "effects": [],
        "caps": [],
        "policies": [],
        "triggers": []
    });
    let bytes = serde_cbor::to_vec(&manifest).expect("cbor encode");
    let mut file = File::create(path).expect("create manifest");
    file.write_all(&bytes).expect("write manifest");
}

#[tokio::test]
async fn reopen_and_replay_smoke() {
    let tmp = TempDir::new().unwrap();
    let manifest_path = tmp.path().join("manifest.cbor");
    write_minimal_manifest(&manifest_path);

    let store = Arc::new(MemStore::new());

    // First open: send an event, run cycle, snapshot.
    {
        let mut host = TestHost::open(store.clone(), &manifest_path).unwrap();
        host.send_event("demo/Event@1", json!({"n": 1})).unwrap();
        host.run_cycle_batch().await.unwrap();
        host.snapshot().unwrap();
    }

    // Reopen and ensure we can run another idle cycle without errors.
    {
        let mut host = TestHost::open(store.clone(), &manifest_path).unwrap();
        host.run_cycle_batch().await.unwrap();
    }
}
