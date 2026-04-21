use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aos_air_types::{
    AirNode, DefSchema, EmptyObject, HashRef, NamedRef, TypeExpr, TypePrimitive, TypePrimitiveText,
    builtins, catalog::EffectCatalog, schema_index::SchemaIndex,
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::ReceiptStatus;
use aos_kernel::Consistency;
use aos_kernel::StateReader;
use aos_kernel::Store;
use aos_kernel::effects::{EffectManager, EffectParamPreprocessor};
use aos_kernel::error::KernelError;
use aos_kernel::governance::ProposalState;
use aos_kernel::governance_effects::GovernanceParamPreprocessor;
use aos_wasm_abi::WorkflowEffect;
use serde::Deserialize;

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/helpers.rs"]
mod helpers;
use fixtures::TestWorld;
use helpers::simple_state_manifest;

#[derive(Debug, Deserialize)]
struct GovProposeReceipt {
    proposal_id: u64,
}

#[test]
fn governance_effects_apply_patch_doc_from_workflow_origin() -> Result<(), KernelError> {
    let store = fixtures::new_mem_store();
    let mut loaded = simple_state_manifest(&store);
    hydrate_schema_hashes(&store, &mut loaded)?;

    let mut effect_manager = build_effect_manager(store.clone(), &loaded)?;

    let mut world = TestWorld::with_store(store, loaded)?;
    let base_manifest = world.kernel.get_manifest(Consistency::Head)?.value;
    let base_hash = Hash::of_cbor(&AirNode::Manifest(base_manifest))
        .map_err(|err| KernelError::Manifest(err.to_string()))?
        .to_hex();
    let patch_doc = patch_doc_add_schema(base_hash, "com.acme/UpgradeSchema@1");
    let patch_doc_bytes =
        serde_json::to_vec(&patch_doc).map_err(|err| KernelError::Manifest(err.to_string()))?;

    let propose_intent = effect_manager.enqueue_workflow_effect_authorized(
        "com.acme/Simple@1",
        &WorkflowEffect::new("governance.propose", propose_params_cbor(&patch_doc_bytes)?),
    )?;
    let propose_receipt = world
        .kernel
        .handle_internal_intent(&propose_intent)?
        .expect("internal receipt");
    assert_eq!(propose_receipt.status, ReceiptStatus::Ok, "propose failed");
    let propose: GovProposeReceipt = propose_receipt.payload().expect("decode receipt");
    let proposal = world
        .kernel
        .governance()
        .proposals()
        .get(&propose.proposal_id)
        .expect("proposal stored");
    assert_eq!(proposal.state, ProposalState::Submitted);

    let shadow_intent = effect_manager.enqueue_workflow_effect_authorized(
        "com.acme/Simple@1",
        &WorkflowEffect::new(
            "governance.shadow",
            shadow_params_cbor(propose.proposal_id)?,
        ),
    )?;
    let shadow_receipt = world
        .kernel
        .handle_internal_intent(&shadow_intent)?
        .expect("internal receipt");
    assert_eq!(shadow_receipt.status, ReceiptStatus::Ok, "shadow failed");
    let proposal = world
        .kernel
        .governance()
        .proposals()
        .get(&propose.proposal_id)
        .expect("proposal stored");
    assert_eq!(proposal.state, ProposalState::Shadowed);

    let approve_intent = effect_manager.enqueue_workflow_effect_authorized(
        "com.acme/Simple@1",
        &WorkflowEffect::new(
            "governance.approve",
            approve_params_cbor(propose.proposal_id)?,
        ),
    )?;
    let approve_receipt = world
        .kernel
        .handle_internal_intent(&approve_intent)?
        .expect("internal receipt");
    assert_eq!(approve_receipt.status, ReceiptStatus::Ok, "approve failed");
    let proposal = world
        .kernel
        .governance()
        .proposals()
        .get(&propose.proposal_id)
        .expect("proposal stored");
    assert_eq!(proposal.state, ProposalState::Approved);

    let apply_intent = effect_manager.enqueue_workflow_effect_authorized(
        "com.acme/Simple@1",
        &WorkflowEffect::new("governance.apply", apply_params_cbor(propose.proposal_id)?),
    )?;
    let apply_receipt = world
        .kernel
        .handle_internal_intent(&apply_intent)?
        .expect("internal receipt");
    assert_eq!(apply_receipt.status, ReceiptStatus::Ok, "apply failed");
    let proposal = world
        .kernel
        .governance()
        .proposals()
        .get(&propose.proposal_id)
        .expect("proposal stored");
    assert_eq!(proposal.state, ProposalState::Applied);

    assert!(
        world.kernel.get_def("com.acme/UpgradeSchema@1").is_some(),
        "patched schema not found in manifest"
    );
    Ok(())
}

fn build_effect_manager(
    store: Arc<fixtures::TestStore>,
    loaded: &aos_kernel::manifest::LoadedManifest,
) -> Result<EffectManager, KernelError> {
    let mut schemas = HashMap::new();
    for builtin in builtins::builtin_schemas() {
        schemas.insert(builtin.schema.name.clone(), builtin.schema.ty.clone());
    }
    for schema in loaded.schemas.values() {
        schemas.insert(schema.name.clone(), schema.ty.clone());
    }
    let schema_index = Arc::new(SchemaIndex::new(schemas));
    let effect_catalog = Arc::new(EffectCatalog::from_defs(loaded.effects.values().cloned()));
    let param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>> = Some(Arc::new(
        GovernanceParamPreprocessor::new(store.clone(), loaded.manifest.clone()),
    ));
    let manager = EffectManager::new(
        effect_catalog,
        schema_index,
        param_preprocessor,
        None,
        None,
    );
    Ok(manager)
}

fn hydrate_schema_hashes(
    store: &Arc<fixtures::TestStore>,
    loaded: &mut aos_kernel::manifest::LoadedManifest,
) -> Result<(), KernelError> {
    for schema in loaded.schemas.values() {
        let node = AirNode::Defschema(schema.clone());
        let hash = Hash::of_cbor(&node).map_err(|err| KernelError::Manifest(err.to_string()))?;
        store
            .put_node(&node)
            .map_err(|err| KernelError::Manifest(err.to_string()))?;
        let hash_ref =
            HashRef::new(hash.to_hex()).map_err(|err| KernelError::Manifest(err.to_string()))?;
        if let Some(reference) = loaded
            .manifest
            .schemas
            .iter_mut()
            .find(|reference| reference.name == schema.name)
        {
            reference.hash = hash_ref;
        } else {
            loaded.manifest.schemas.push(NamedRef {
                name: schema.name.clone(),
                hash: hash_ref,
            });
        }
    }
    Ok(())
}

fn patch_doc_add_schema(base_manifest_hash: String, name: &str) -> serde_json::Value {
    let schema = DefSchema {
        name: name.to_string(),
        ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: EmptyObject {},
        })),
    };
    let node = serde_json::to_value(AirNode::Defschema(schema)).expect("serialize defschema node");
    serde_json::json!({
        "version": "1",
        "base_manifest_hash": base_manifest_hash,
        "patches": [
            {
                "add_def": {
                    "kind": "defschema",
                    "node": node,
                }
            }
        ]
    })
}

fn propose_params_cbor(patch_doc_bytes: &[u8]) -> Result<Vec<u8>, KernelError> {
    let mut patch_map = BTreeMap::new();
    patch_map.insert(
        serde_cbor::Value::Text("patch_doc_json".into()),
        serde_cbor::Value::Bytes(patch_doc_bytes.to_vec()),
    );
    let mut params = BTreeMap::new();
    params.insert(
        serde_cbor::Value::Text("patch".into()),
        serde_cbor::Value::Map(patch_map),
    );
    params.insert(
        serde_cbor::Value::Text("summary".into()),
        serde_cbor::Value::Null,
    );
    params.insert(
        serde_cbor::Value::Text("manifest_base".into()),
        serde_cbor::Value::Null,
    );
    params.insert(
        serde_cbor::Value::Text("description".into()),
        serde_cbor::Value::Text("manual upgrade".into()),
    );
    to_canonical_cbor(&serde_cbor::Value::Map(params))
        .map_err(|err| KernelError::Manifest(err.to_string()))
}

fn shadow_params_cbor(proposal_id: u64) -> Result<Vec<u8>, KernelError> {
    let mut params = BTreeMap::new();
    params.insert(
        serde_cbor::Value::Text("proposal_id".into()),
        serde_cbor::Value::Integer(proposal_id as i128),
    );
    to_canonical_cbor(&serde_cbor::Value::Map(params))
        .map_err(|err| KernelError::Manifest(err.to_string()))
}

fn approve_params_cbor(proposal_id: u64) -> Result<Vec<u8>, KernelError> {
    let mut decision = BTreeMap::new();
    decision.insert(
        serde_cbor::Value::Text("approve".into()),
        serde_cbor::Value::Null,
    );

    let mut params = BTreeMap::new();
    params.insert(
        serde_cbor::Value::Text("proposal_id".into()),
        serde_cbor::Value::Integer(proposal_id as i128),
    );
    params.insert(
        serde_cbor::Value::Text("decision".into()),
        serde_cbor::Value::Map(decision),
    );
    params.insert(
        serde_cbor::Value::Text("approver".into()),
        serde_cbor::Value::Text("test".into()),
    );
    params.insert(
        serde_cbor::Value::Text("reason".into()),
        serde_cbor::Value::Null,
    );
    to_canonical_cbor(&serde_cbor::Value::Map(params))
        .map_err(|err| KernelError::Manifest(err.to_string()))
}

fn apply_params_cbor(proposal_id: u64) -> Result<Vec<u8>, KernelError> {
    let mut params = BTreeMap::new();
    params.insert(
        serde_cbor::Value::Text("proposal_id".into()),
        serde_cbor::Value::Integer(proposal_id as i128),
    );
    to_canonical_cbor(&serde_cbor::Value::Map(params))
        .map_err(|err| KernelError::Manifest(err.to_string()))
}
