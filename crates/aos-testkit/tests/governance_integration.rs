use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_air_types::AirNode;
use aos_kernel::governance::ManifestPatch;
use aos_kernel::shadow::ShadowHarness;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::{TestStore, TestWorld};
use aos_wasm_abi::ReducerOutput;

mod helpers;
use helpers::simple_state_manifest;

#[test]
fn governance_flow_applies_manifest_patch() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_reducer(&store, "com.acme/Patched@1", 0xBB);
    let mut patch_nodes: Vec<AirNode> = patch_loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .collect();
    patch_nodes.extend(patch_loaded.caps.values().cloned().map(AirNode::Defcap));
    patch_nodes.extend(patch_loaded.policies.values().cloned().map(AirNode::Defpolicy));
    patch_nodes.extend(patch_loaded.plans.values().cloned().map(AirNode::Defplan));
    let patch = ManifestPatch {
        manifest: patch_loaded.manifest.clone(),
        nodes: patch_nodes,
    };
    let proposal_id = world.kernel.submit_proposal(patch, Some("test".into())).unwrap();

    world
        .kernel
        .run_shadow(proposal_id, Some(ShadowHarness::default()))
        .unwrap();
    world
        .kernel
        .approve_proposal(proposal_id, "approver")
        .unwrap();
    world.kernel.apply_proposal(proposal_id).unwrap();

    world.submit_event_value(START_SCHEMA, &ExprValue::Record(Default::default()));
    world.tick_n(1).unwrap();
    let reducer_state = world
        .kernel
        .reducer_state("com.acme/Patched@1")
        .cloned()
        .expect("reducer state");
    assert_eq!(reducer_state, vec![0xBB]);
}

fn manifest_with_reducer(
    store: &Arc<TestStore>,
    name: &str,
    state_byte: u8,
) -> aos_kernel::manifest::LoadedManifest {
    let reducer = fixtures::stub_reducer_module(
        store,
        name,
        &ReducerOutput {
            state: Some(vec![state_byte]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer.name)];
    fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing)
}
