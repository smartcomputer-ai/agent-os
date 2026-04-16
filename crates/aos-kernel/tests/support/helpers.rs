//! Shared test helpers for integration tests.
//!
//! These helpers are workflow-era only (no plan fixtures).

#![allow(dead_code)]

use aos_air_types::{
    DefPolicy, DefSchema, EmptyObject, ManifestDefaults, NamedRef, TypeExpr, TypePrimitive,
    TypePrimitiveInt, TypePrimitiveText, TypeRecord, TypeRef, TypeVariant, WorkflowAbi,
};
use aos_effects::builtins::TimerSetParams;
#[path = "fixtures.rs"]
pub mod fixtures;

use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use fixtures::{START_SCHEMA, TestStore, zero_hash};
use indexmap::IndexMap;
use std::sync::Arc;

/// Builds a test manifest with a workflow that emits a timer effect and another workflow
/// that handles the timer receipt event.
pub fn timer_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let timer_output = WorkflowOutput {
        state: Some(vec![0x01]),
        domain_events: vec![],
        effects: vec![WorkflowEffect::new(
            aos_effects::EffectKind::TIMER_SET,
            serde_cbor::to_vec(&TimerSetParams {
                deliver_at_ns: 5,
                key: Some("retry".into()),
            })
            .unwrap(),
        )],
        ann: None,
    };
    let mut timer_emitter =
        fixtures::stub_workflow_module(store, "com.acme/TimerEmitter@1", &timer_output);

    let handler_output = WorkflowOutput {
        state: Some(vec![0xCC]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let mut timer_handler =
        fixtures::stub_workflow_module(store, "com.acme/TimerHandler@1", &handler_output);

    let timer_event_schema = "com.acme/TimerEvent@1";
    let routing = vec![
        fixtures::routing_event(fixtures::START_SCHEMA, &timer_emitter.name),
        fixtures::routing_event(fixtures::SYS_TIMER_FIRED, &timer_handler.name),
    ];
    timer_emitter.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(timer_event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });
    timer_handler.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema(START_SCHEMA),
        event: fixtures::schema(timer_event_schema),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let mut loaded = fixtures::build_loaded_manifest(vec![timer_emitter, timer_handler], routing);
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(fixtures::START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: timer_event_schema.into(),
                ty: TypeExpr::Variant(TypeVariant {
                    variant: IndexMap::from([
                        (
                            "Start".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(fixtures::START_SCHEMA),
                            }),
                        ),
                        (
                            "Fired".into(),
                            TypeExpr::Ref(TypeRef {
                                reference: fixtures::schema(fixtures::SYS_TIMER_FIRED),
                            }),
                        ),
                    ]),
                }),
            },
        ],
    );
    loaded
}

/// Builds a simple manifest with a single workflow that sets deterministic state when invoked.
pub fn simple_state_manifest(store: &Arc<TestStore>) -> aos_kernel::manifest::LoadedManifest {
    let mut workflow = fixtures::stub_workflow_module(
        store,
        "com.acme/Simple@1",
        &WorkflowOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/SimpleState@1"),
        event: fixtures::schema(START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });
    let routing = vec![fixtures::routing_event(START_SCHEMA, &workflow.name)];
    let mut loaded = fixtures::build_loaded_manifest(vec![workflow], routing);
    insert_test_schemas(
        &mut loaded,
        vec![
            def_text_record_schema(START_SCHEMA, vec![("id", text_type())]),
            DefSchema {
                name: "com.acme/SimpleState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

pub fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject {},
    }))
}

pub fn int_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Int(TypePrimitiveInt {
        int: EmptyObject {},
    }))
}

pub fn def_text_record_schema(name: &str, fields: Vec<(&str, TypeExpr)>) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Record(TypeRecord {
            record: IndexMap::from_iter(fields.into_iter().map(|(k, ty)| (k.to_string(), ty))),
        }),
    }
}

pub fn insert_test_schemas(
    loaded: &mut aos_kernel::manifest::LoadedManifest,
    schemas: Vec<DefSchema>,
) {
    for schema in schemas {
        let name = schema.name.clone();
        loaded.schemas.insert(name.clone(), schema);
        if !loaded
            .manifest
            .schemas
            .iter()
            .any(|existing| existing.name == name)
        {
            loaded.manifest.schemas.push(NamedRef {
                name,
                hash: zero_hash(),
            });
        }
    }
}

/// Attaches a policy to the manifest defaults so it becomes the runtime policy gate.
pub fn attach_default_policy(loaded: &mut aos_kernel::manifest::LoadedManifest, policy: DefPolicy) {
    loaded.manifest.policies.push(NamedRef {
        name: policy.name.clone(),
        hash: zero_hash(),
    });
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.policy = Some(policy.name.clone());
    } else {
        loaded.manifest.defaults = Some(ManifestDefaults {
            policy: Some(policy.name.clone()),
            cap_grants: vec![],
        });
    }
    loaded.policies.insert(policy.name.clone(), policy);
}
