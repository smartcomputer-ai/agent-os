use std::{collections::HashMap, sync::Arc};

use aos_air_types::{
    AirNode, CapGrant, CapType, DefCap, DefPlan, EffectKind, EmptyObject, Expr, ExprConst,
    ExprOrValue, ExprRecord, ExprRef, Manifest, ManifestDefaults, NamedRef, PlanBind,
    PlanBindEffect, PlanEdge, PlanStep, PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd,
    PlanStepKind, ReducerAbi, TypeExpr, TypeRecord, ValueLiteral, ValueMap, ValueNull, ValueRecord,
    ValueText,
    builtins::builtin_schemas,
    plan_literals::{SchemaIndex, normalize_plan_literals},
};
use aos_cbor::{Hash, to_canonical_cbor};
use aos_host::fixtures::{self, START_SCHEMA, TestStore, TestWorld};
use aos_kernel::error::KernelError;
use aos_kernel::governance::ManifestPatch;
use aos_kernel::journal::{GovernanceRecord, JournalKind, JournalRecord};
use aos_kernel::shadow::{LedgerDelta, LedgerKind, ShadowHarness};
use aos_wasm_abi::ReducerOutput;
use indexmap::IndexMap;
use serde_cbor;
use serde_json::json;

mod helpers;
use helpers::simple_state_manifest;

#[test]
fn governance_flow_applies_manifest_patch() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_reducer(&store, "com.acme/Patched@1", 0xBB);
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
    let reducer_state = world
        .kernel
        .reducer_state("com.acme/Patched@1")
        .cloned()
        .expect("reducer state");
    assert_eq!(reducer_state, vec![0xBB]);
}

#[test]
fn apply_requires_approval_state() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let patch_loaded = manifest_with_reducer(&store, "com.acme/Patched@1", 0xBC);
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
fn shadow_summary_includes_predictions_and_deltas() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let loaded = shadow_plan_manifest(&store);
    let mut patch = manifest_patch_from_loaded(&loaded);
    patch.manifest.caps.push(NamedRef {
        name: "sys/http.out@1".into(),
        hash: fixtures::zero_hash(),
    });
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("shadow".into()))
        .unwrap();

    let harness = ShadowHarness {
        seed_events: vec![start_seed_event()],
    };
    let summary = world.kernel.run_shadow(proposal_id, Some(harness)).unwrap();

    assert_eq!(summary.predicted_effects.len(), 1);
    assert_eq!(summary.pending_receipts.len(), 0);
    assert_eq!(summary.plan_results.len(), 1);
    assert!(summary.ledger_deltas.iter().any(|delta| delta
        == &LedgerDelta {
            ledger: LedgerKind::Capability,
            name: "sys/http.out@1".to_string(),
            change: aos_kernel::shadow::DeltaKind::Added,
        }));
}

#[test]
fn patch_hash_is_identical_for_sugar_and_canonical_plans() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let sugar_plan: DefPlan = serde_json::from_value(sample_plan_json()).expect("plan json");
    let mut canonical_plan: DefPlan =
        serde_json::from_value(sample_plan_json()).expect("plan json");
    let effect_catalog = aos_air_types::catalog::EffectCatalog::from_defs(
        aos_air_types::builtins::builtin_effects()
            .iter()
            .map(|e| e.effect.clone()),
    );
    normalize_plan_literals(
        &mut canonical_plan,
        &builtin_schema_index(),
        &HashMap::new(),
        &effect_catalog,
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

#[test]
fn proposals_with_same_patch_hash_do_not_collide() {
    let store = fixtures::new_mem_store();
    let manifest = simple_state_manifest(&store);
    let mut world = TestWorld::with_store(store.clone(), manifest).unwrap();

    let loaded = manifest_with_reducer(&store, "com.acme/Collision@1", 0xCD);
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

    let patch_loaded = manifest_with_reducer(&store, "com.acme/Reject@1", 0xEE);
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

    let patch_loaded = manifest_with_reducer(&store, "com.acme/AppliedRoot@1", 0xEF);
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

    let manifest_bytes = to_canonical_cbor(&patch.manifest).expect("manifest cbor");
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
fn shadow_upgrade_reports_followup_effects() {
    let store = fixtures::new_mem_store();
    let base = upgrade_manifest(&store, false);
    let mut world = TestWorld::with_store(store.clone(), base).unwrap();

    let upgraded = upgrade_manifest(&store, true);
    let patch = manifest_patch_from_loaded(&upgraded);
    let proposal_id = world
        .kernel
        .submit_proposal(patch, Some("safe-upgrade".into()))
        .unwrap();

    let summary = world
        .kernel
        .run_shadow(proposal_id, Some(ShadowHarness::default()))
        .unwrap();
    assert!(summary.predicted_effects.is_empty());
    assert!(
        summary
            .ledger_deltas
            .iter()
            .any(|delta| delta.name == "com.acme/http_followup_cap@1")
    );

    world
        .kernel
        .approve_proposal(proposal_id, "approver")
        .unwrap();
    world.kernel.apply_proposal(proposal_id).unwrap();
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
    let mut loaded = fixtures::build_loaded_manifest(vec![], vec![], vec![reducer], routing);
    helpers::insert_test_schemas(
        &mut loaded,
        vec![helpers::def_text_record_schema(
            START_SCHEMA,
            vec![("id", helpers::text_type())],
        )],
    );
    loaded
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
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            plans: vec![NamedRef {
                name: plan.name.clone(),
                hash: fixtures::zero_hash(),
            }],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
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

fn manifest_patch_from_loaded(loaded: &aos_kernel::manifest::LoadedManifest) -> ManifestPatch {
    let mut nodes: Vec<AirNode> = loaded
        .modules
        .values()
        .cloned()
        .map(AirNode::Defmodule)
        .collect();
    nodes.extend(loaded.caps.values().cloned().map(AirNode::Defcap));
    nodes.extend(loaded.policies.values().cloned().map(AirNode::Defpolicy));
    nodes.extend(loaded.plans.values().cloned().map(AirNode::Defplan));
    nodes.extend(loaded.effects.values().cloned().map(AirNode::Defeffect));
    nodes.extend(loaded.schemas.values().cloned().map(AirNode::Defschema));

    ManifestPatch {
        manifest: loaded.manifest.clone(),
        nodes,
    }
}

fn start_seed_event() -> (String, Vec<u8>) {
    let bytes = serde_cbor::to_vec(&serde_json::json!({ "id": "seed" })).expect("encode start event");
    (START_SCHEMA.to_string(), bytes)
}

fn upgrade_manifest(
    store: &Arc<TestStore>,
    followup: bool,
) -> aos_kernel::manifest::LoadedManifest {
    let plan_name = if followup {
        "com.acme/UpgradePlan@2"
    } else {
        "com.acme/UpgradePlan@1"
    };
    let plan = if followup {
        upgrade_plan_v2(plan_name)
    } else {
        upgrade_plan_v1(plan_name)
    };
    let mut reducer = fixtures::stub_reducer_module(
        store,
        "com.acme/UpgradeReducer@1",
        &ReducerOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    reducer.abi.reducer = Some(ReducerAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(START_SCHEMA),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: IndexMap::new(),
    });

    let routing = vec![fixtures::routing_event(START_SCHEMA, &reducer.name)];
    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(plan_name)],
        vec![reducer],
        routing,
    );

    let primary_cap = test_http_cap("com.acme/http_primary_cap@1");
    let followup_cap = test_http_cap("com.acme/http_followup_cap@1");
    let mut cap_refs = loaded.manifest.caps.clone();
    cap_refs.push(NamedRef {
        name: primary_cap.name.clone(),
        hash: fixtures::zero_hash(),
    });
    loaded.manifest.caps = cap_refs;
    loaded
        .caps
        .insert(primary_cap.name.clone(), primary_cap.clone());
    let mut cap_grants = loaded
        .manifest
        .defaults
        .clone()
        .map(|defaults| defaults.cap_grants)
        .unwrap_or_default();
    cap_grants.push(CapGrant {
        name: "cap_http_primary".into(),
        cap: primary_cap.name.clone(),
        params: empty_literal(),
        expiry_ns: None,
        budget: None,
    });
    if followup {
        loaded.manifest.caps.push(NamedRef {
            name: followup_cap.name.clone(),
            hash: fixtures::zero_hash(),
        });
        loaded
            .caps
            .insert(followup_cap.name.clone(), followup_cap.clone());
        cap_grants.push(CapGrant {
            name: "cap_http_followup".into(),
            cap: followup_cap.name,
            params: empty_literal(),
            expiry_ns: None,
            budget: None,
        });
    }
    loaded.manifest.defaults = Some(ManifestDefaults {
        policy: None,
        cap_grants,
    });

    helpers::insert_test_schemas(
        &mut loaded,
        vec![helpers::def_text_record_schema(
            START_SCHEMA,
            vec![("id", helpers::text_type())],
        )],
    );

    loaded
}

fn upgrade_plan_v1(name: &str) -> DefPlan {
    DefPlan {
        name: name.to_string(),
        input: fixtures::schema(START_SCHEMA),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("v1"),
                    cap: "cap_http_primary".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: fixtures::var_expr("req"),
                    bind: PlanBind {
                        var: "receipt".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "emit".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http_primary".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    }
}

fn upgrade_plan_v2(name: &str) -> DefPlan {
    let mut plan = upgrade_plan_v1(name);
    plan.name = name.to_string();
    plan.steps.insert(
        1,
        PlanStep {
            id: "emit_followup".into(),
            kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                kind: EffectKind::http_request(),
                params: http_params_literal("v2"),
                cap: "cap_http_followup".into(),
                bind: PlanBindEffect {
                    effect_id_as: "req_follow".into(),
                },
            }),
        },
    );
    plan.steps.insert(
        2,
        PlanStep {
            id: "await_followup".into(),
            kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                for_expr: fixtures::var_expr("req_follow"),
                bind: PlanBind {
                    var: "receipt_follow".into(),
                },
            }),
        },
    );
    plan.edges = vec![
        PlanEdge {
            from: "emit".into(),
            to: "emit_followup".into(),
            when: None,
        },
        PlanEdge {
            from: "emit_followup".into(),
            to: "await_followup".into(),
            when: None,
        },
        PlanEdge {
            from: "await_followup".into(),
            to: "await".into(),
            when: None,
        },
        PlanEdge {
            from: "await".into(),
            to: "end".into(),
            when: None,
        },
    ];
    plan.required_caps.push("cap_http_followup".to_string());
    plan
}

fn test_http_cap(name: &str) -> DefCap {
    DefCap {
        name: name.to_string(),
        cap_type: CapType::http_out(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
    }
}

fn empty_literal() -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::new(),
    })
}

fn shadow_plan_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let _ = store;
    let plan_name = "com.acme/ShadowPlan@1".to_string();
    let plan = DefPlan {
        name: plan_name.clone(),
        input: fixtures::schema(START_SCHEMA),
        output: Some(fixtures::schema("com.acme/ShadowOut@1")),
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: EffectKind::http_request(),
                    params: http_params_literal("shadow"),
                    cap: "cap_http".into(),
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "await".into(),
                kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                    for_expr: Expr::Ref(ExprRef {
                        reference: "@var:req".into(),
                    }),
                    bind: PlanBind { var: "resp".into() },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd {
                    result: Some(
                        Expr::Record(ExprRecord {
                            record: IndexMap::from([(
                                "value".into(),
                                Expr::Const(ExprConst::Text {
                                    text: "done".into(),
                                }),
                            )]),
                        })
                        .into(),
                    ),
                }),
            },
        ],
        edges: vec![
            PlanEdge {
                from: "emit".into(),
                to: "await".into(),
                when: None,
            },
            PlanEdge {
                from: "await".into(),
                to: "end".into(),
                when: None,
            },
        ],
        required_caps: vec!["cap_http".into()],
        allowed_effects: vec![EffectKind::http_request()],
        invariants: vec![],
    };

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![],
        vec![],
    );

    helpers::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema(
                "com.acme/ShadowOut@1",
                vec![("value", helpers::text_type())],
            ),
        ],
    );

    loaded
}

fn http_params_literal(tag: &str) -> ExprOrValue {
    ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "method".into(),
                ValueLiteral::Text(ValueText { text: "GET".into() }),
            ),
            (
                "url".into(),
                ValueLiteral::Text(ValueText {
                    text: format!("https://example.com/{tag}"),
                }),
            ),
            (
                "headers".into(),
                ValueLiteral::Map(ValueMap {
                    map: vec![], // empty header map is allowed
                }),
            ),
            (
                "body_ref".into(),
                ValueLiteral::Null(ValueNull {
                    null: EmptyObject {},
                }),
            ),
        ]),
    }))
}
