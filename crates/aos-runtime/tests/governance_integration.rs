#![cfg(feature = "e2e-tests")]

use std::sync::Arc;

use aos_air_types::{
    AirNode, CapGrant, CapType, DefCap, DefSchema, ManifestDefaults, NamedRef, TypeExpr,
    TypeRecord, ValueLiteral, ValueRecord, WorkflowAbi,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_kernel::error::KernelError;
use aos_kernel::governance::ManifestPatch;
use aos_kernel::journal::{GovernanceRecord, JournalKind, JournalRecord};
use aos_kernel::shadow::ShadowHarness;
use aos_wasm_abi::WorkflowOutput;
use helpers::fixtures::{self, START_SCHEMA, TestStore, TestWorld};
use indexmap::IndexMap;

mod helpers;
use helpers::simple_state_manifest;

#[test]
fn governance_flow_applies_manifest_patch() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_workflow(&store, "com.acme/Patched@1", 0xBB);
    let patch = manifest_patch_from_loaded(&patch_loaded);
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

    world
        .submit_event_result(START_SCHEMA, &serde_json::json!({ "id": "start" }))
        .expect("submit start event");
    world.tick_n(1).unwrap();
    let workflow_state = world
        .kernel
        .workflow_state("com.acme/Patched@1")
        .expect("workflow state");
    assert_eq!(workflow_state, vec![0xBB]);
}

#[test]
fn apply_requires_approval_state() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_workflow(&store, "com.acme/Patched@1", 0xBC);
    let patch = manifest_patch_from_loaded(&patch_loaded);
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("needs approval".into()))
        .unwrap();

    let err = world.kernel.apply_proposal(proposal_id).unwrap_err();
    assert!(matches!(
        err,
        KernelError::ProposalStateInvalid { required, .. } if required == "approved"
    ));
}

#[test]
fn proposals_with_same_patch_hash_do_not_collide() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let loaded = manifest_with_workflow(&store, "com.acme/Collision@1", 0xCD);
    let patch = manifest_patch_from_loaded(&loaded);

    let first = world
        .kernel
        .submit_proposal(patch.clone(), Some("first".into()))
        .unwrap();
    let second = world
        .kernel
        .submit_proposal(patch, Some("second".into()))
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(
        world
            .kernel
            .governance()
            .proposals()
            .get(&first)
            .unwrap()
            .patch_hash,
        world
            .kernel
            .governance()
            .proposals()
            .get(&second)
            .unwrap()
            .patch_hash
    );
}

#[test]
fn reject_prevents_apply_and_records_decision() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_workflow(&store, "com.acme/Reject@1", 0xEE);
    let patch = manifest_patch_from_loaded(&patch_loaded);
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("reject-me".into()))
        .unwrap();

    world
        .kernel
        .run_shadow(proposal_id, Some(ShadowHarness::default()))
        .unwrap();
    world
        .kernel
        .reject_proposal(proposal_id, "approver")
        .unwrap();

    let err = world.kernel.apply_proposal(proposal_id).unwrap_err();
    assert!(matches!(
        err,
        KernelError::ProposalStateInvalid { required, .. } if required == "approved"
    ));

    let record = world
        .kernel
        .dump_journal()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.kind == JournalKind::Governance)
        .map(|entry| serde_cbor::from_slice::<JournalRecord>(&entry.payload).unwrap())
        .find_map(|record| match record {
            JournalRecord::Governance(GovernanceRecord::Approved(r)) => Some(r),
            _ => None,
        })
        .expect("approved/rejected record present");

    assert!(matches!(
        record.decision,
        aos_kernel::journal::ApprovalDecisionRecord::Reject
    ));
}

#[test]
fn applied_records_manifest_root_not_patch_hash() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_workflow(&store, "com.acme/AppliedRoot@1", 0xEF);
    let patch = manifest_patch_from_loaded(&patch_loaded);
    let proposal_id = world
        .kernel
        .submit_proposal(patch.clone(), Some("root".into()))
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

    let canonical = aos_kernel::world::canonicalize_patch(store.as_ref(), patch.clone())
        .expect("canonicalize patch");
    let manifest_bytes = to_canonical_cbor(&canonical.manifest).expect("manifest cbor");
    let expected_manifest_hash = Hash::of_bytes(&manifest_bytes).to_hex();
    let patch_hash = world
        .kernel
        .governance()
        .proposals()
        .get(&proposal_id)
        .unwrap()
        .patch_hash
        .clone();

    let applied = world
        .kernel
        .dump_journal()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.kind == JournalKind::Governance)
        .map(|entry| {
            serde_cbor::from_slice::<JournalRecord>(&entry.payload).expect("governance record")
        })
        .find_map(|record| match record {
            JournalRecord::Governance(GovernanceRecord::Applied(r)) => Some(r),
            _ => None,
        })
        .expect("applied record present");

    assert_eq!(applied.manifest_hash_new, expected_manifest_hash);
    assert_eq!(applied.patch_hash, patch_hash);
    assert_ne!(applied.manifest_hash_new, applied.patch_hash);
}

#[test]
fn shadow_upgrade_reports_followup_effect_cap_delta() {
    let store = fixtures::new_mem_store();
    let base = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), base).unwrap();

    let upgraded = manifest_with_added_cap(&store);
    let patch = manifest_patch_from_loaded(&upgraded);
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("safe-upgrade".into()))
        .unwrap();

    let summary = world
        .kernel
        .run_shadow(proposal_id, Some(ShadowHarness::default()))
        .unwrap();
    assert!(
        summary
            .ledger_deltas
            .iter()
            .any(|delta| delta.name == "com.acme/http_followup_cap@1")
    );
}

fn manifest_with_workflow(
    store: &Arc<TestStore>,
    name: &str,
    state_byte: u8,
) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        name,
        &WorkflowOutput {
            state: Some(vec![state_byte]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/PatchedState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let routing = vec![fixtures::routing_event(START_SCHEMA, &workflow.name)];
    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
    helpers::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(START_SCHEMA, vec![("id", helpers::text_type())]),
            DefSchema {
                name: "com.acme/PatchedState@1".into(),
                ty: helpers::text_type(),
            },
        ],
    );
    loaded
}

fn manifest_with_added_cap(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let mut loaded = simple_state_manifest(store);
    let followup_cap = test_http_cap("com.acme/http_followup_cap@1");

    loaded.manifest.caps.push(NamedRef {
        name: followup_cap.name.clone(),
        hash: fixtures::zero_hash(),
    });
    loaded
        .caps
        .insert(followup_cap.name.clone(), followup_cap.clone());

    let mut cap_grants = loaded
        .manifest
        .defaults
        .clone()
        .map(|defaults| defaults.cap_grants)
        .unwrap_or_default();
    cap_grants.push(CapGrant {
        name: "cap_http_followup".into(),
        cap: followup_cap.name,
        params: empty_literal(),
        expiry_ns: None,
    });
    loaded.manifest.defaults = Some(ManifestDefaults {
        policy: None,
        cap_grants,
    });

    loaded
}

fn manifest_patch_from_loaded(loaded: &aos_kernel::manifest::LoadedManifest) -> ManifestPatch {
    let mut nodes: Vec<AirNode> = loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .collect();
    nodes.extend(loaded.caps.values().cloned().map(AirNode::Defcap));
    nodes.extend(loaded.policies.values().cloned().map(AirNode::Defpolicy));
    nodes.extend(loaded.effects.values().cloned().map(AirNode::Defeffect));
    nodes.extend(loaded.schemas.values().cloned().map(AirNode::Defschema));

    ManifestPatch {
        manifest: loaded.manifest.clone(),
        nodes,
    }
}

fn test_http_cap(name: &str) -> DefCap {
    DefCap {
        name: name.to_string(),
        cap_type: CapType::http_out(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapEnforceHttpOut@1".into(),
        },
    }
}

fn empty_literal() -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::new(),
    })
}
