use std::{collections::HashMap, sync::Arc};

use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    AirNode, DefPlan, Manifest, NamedRef,
    builtins::builtin_schemas,
    plan_literals::{SchemaIndex, normalize_plan_literals},
};
use aos_kernel::governance::ManifestPatch;
use aos_kernel::shadow::ShadowHarness;
use aos_testkit::fixtures::{self, START_SCHEMA};
use aos_testkit::{TestStore, TestWorld};
use aos_wasm_abi::ReducerOutput;
use serde_json::json;

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
    patch_nodes.extend(
        patch_loaded
            .policies
            .values()
            .cloned()
            .map(AirNode::Defpolicy),
    );
    patch_nodes.extend(patch_loaded.plans.values().cloned().map(AirNode::Defplan));
    let patch = ManifestPatch {
        manifest: patch_loaded.manifest.clone(),
        nodes: patch_nodes,
    };
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("test".into()))
        .unwrap();

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

#[test]
fn patch_hash_is_identical_for_sugar_and_canonical_plans() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let sugar_plan: DefPlan = serde_json::from_value(sample_plan_json()).expect("plan json");
    let mut canonical_plan: DefPlan =
        serde_json::from_value(sample_plan_json()).expect("plan json");
    normalize_plan_literals(
        &mut canonical_plan,
        &builtin_schema_index(),
        &HashMap::new(),
    )
    .expect("normalize canonical plan");

    let sugar_patch = plan_patch(sugar_plan);
    let canonical_patch = plan_patch(canonical_plan);

    let sugar_id = world
        .kernel
        .submit_proposal(sugar_patch, Some("sugar".into()))
        .unwrap();
    let canonical_id = world
        .kernel
        .submit_proposal(canonical_patch, Some("canonical".into()))
        .unwrap();

    let hash_sugar = world
        .kernel
        .governance()
        .proposals()
        .get(&sugar_id)
        .unwrap()
        .patch_hash
        .clone();
    let hash_canonical = world
        .kernel
        .governance()
        .proposals()
        .get(&canonical_id)
        .unwrap()
        .patch_hash
        .clone();
    assert_eq!(hash_sugar, hash_canonical);
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

fn sample_plan_json() -> serde_json::Value {
    json!({
        "$kind": "defplan",
        "name": "com.acme/SugarHttp@1",
        "input": "sys/HttpRequestParams@1",
        "steps": [
            {
                "id": "emit",
                "op": "emit_effect",
                "kind": "http.request",
                "params": {
                    "headers": {
                        "content-type": "application/json",
                        "accept": "*/*"
                    },
                    "method": "POST",
                    "url": "https://example.com",
                    "body_ref": null
                },
                "cap": "cap_http",
                "bind": { "effect_id_as": "req" }
            },
            {
                "id": "await",
                "op": "await_receipt",
                "for": { "ref": "@var:req" },
                "bind": { "as": "resp" }
            },
            { "id": "end", "op": "end" }
        ],
        "edges": [
            { "from": "emit", "to": "await" },
            { "from": "await", "to": "end" }
        ],
        "required_caps": ["cap_http"],
        "allowed_effects": ["http.request"]
    })
}

fn plan_patch(plan: DefPlan) -> ManifestPatch {
    ManifestPatch {
        manifest: Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![NamedRef {
                name: plan.name.clone(),
                hash: fixtures::zero_hash(),
            }],
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing: None,
            triggers: vec![],
        },
        nodes: vec![AirNode::Defplan(plan)],
    }
}

fn builtin_schema_index() -> SchemaIndex {
    let mut map = HashMap::new();
    for builtin in builtin_schemas() {
        map.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    SchemaIndex::new(map)
}
