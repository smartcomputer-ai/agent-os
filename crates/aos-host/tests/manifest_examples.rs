use std::path::PathBuf;
use std::sync::Arc;

use aos_host::manifest_loader::load_from_assets;
use aos_store::FsStore;
use tempfile::TempDir;

#[test]
fn aggregator_manifest_loads_from_assets() {
    let tmp = TempDir::new().expect("tmp");
    let store = Arc::new(FsStore::open(tmp.path()).expect("store"));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/04-aggregator");
    let loaded = load_from_assets(store, &root)
        .expect("load assets")
        .expect("manifest");
    assert!(loaded.manifest.modules.iter().any(|m| m.name == "demo/Aggregator@1"));
}
