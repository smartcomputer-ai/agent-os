use std::path::PathBuf;
use std::sync::Arc;

use aos_node::{FsCas, LocalStatePaths};
use aos_runtime::manifest_loader::load_from_assets;

#[test]
fn aggregator_manifest_loads_from_assets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("crates/aos-smoke/fixtures/04-aggregator");
    let paths = LocalStatePaths::from_world_root(&root);
    let store = Arc::new(FsCas::open_with_paths(&paths).expect("store"));
    let loaded = load_from_assets(store, &root)
        .expect("load assets")
        .expect("manifest");
    assert!(
        loaded
            .manifest
            .modules
            .iter()
            .any(|m| m.name == "demo/Aggregator@1")
    );
}
